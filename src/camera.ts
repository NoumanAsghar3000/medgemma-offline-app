// Camera capture helpers (pure logic, unit-tested): getUserMedia with a constraint ladder, actionable error messages, readiness polling.

export type GetUserMedia = (c: MediaStreamConstraints) => Promise<MediaStream>;

/** Tried in order. A laptop webcam has no rear camera, so "environment" is only a preference; every step is more permissive than the last. */
export const CAMERA_LADDER: MediaStreamConstraints[] = [
  { video: { facingMode: { ideal: "environment" }, width: { ideal: 1600 } }, audio: false },
  { video: { width: { ideal: 1600 } }, audio: false },
  { video: true, audio: false },
];

export function errName(e: unknown): string {
  if (typeof e === "object" && e !== null && "name" in e) return String((e as { name: unknown }).name);
  return "";
}

/** Only constraint problems are worth retrying with looser constraints; permission / missing / busy camera are final. */
const RETRYABLE = new Set(["OverconstrainedError", "ConstraintNotSatisfiedError", "TypeError"]);

export function cameraErrorMessage(e: unknown): string {
  switch (errName(e)) {
    case "NotAllowedError":
    case "PermissionDeniedError":
    case "SecurityError":
      return "Camera permission was denied. In Windows Settings > Privacy & security > Camera, allow camera access and let desktop apps use the camera, then try again.";
    case "NotFoundError":
    case "DevicesNotFoundError":
      return "No camera was found on this computer. Connect a camera and try again.";
    case "NotReadableError":
    case "TrackStartError":
    case "AbortError":
      return "The camera is in use by another app or could not be started. Close the other app and try again.";
    case "OverconstrainedError":
    case "ConstraintNotSatisfiedError":
      return "The camera does not support the requested settings.";
    case "CameraTimeout":
      return "Timed out waiting for the camera. If Windows asked for permission, answer the prompt and try again.";
    case "NoMediaDevices":
      return "The camera API is not available in this window.";
    default:
      return "Camera unavailable: " + (e instanceof Error ? e.message : String(e));
  }
}

export async function openCameraStream(gum?: GetUserMedia): Promise<MediaStream> {
  const md = typeof navigator !== "undefined" ? navigator.mediaDevices : undefined;
  const fn: GetUserMedia | undefined = gum ?? (md && typeof md.getUserMedia === "function" ? (c) => md.getUserMedia(c) : undefined);
  if (!fn) throw Object.assign(new Error("The camera API is not available in this window."), { name: "NoMediaDevices" });
  let last: unknown;
  for (const c of CAMERA_LADDER) {
    try {
      return await fn(c);
    } catch (e) {
      last = e;
      if (!RETRYABLE.has(errName(e))) throw e;
    }
  }
  throw last;
}

/** Resolves true as soon as the video element delivers real frames, false after `ms`. */
export async function waitForFrame(video: { videoWidth: number }, ms: number, step = 100): Promise<boolean> {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (video.videoWidth > 0) return true;
    await new Promise((r) => setTimeout(r, step));
  }
  return video.videoWidth > 0;
}

export function stopStream(s: MediaStream | null): void {
  if (!s) return;
  s.getTracks().forEach((t) => t.stop());
}

/** What the session needs from the page; main.ts maps it onto the dialog's elements, tests use a fake. */
export interface CameraUI {
  show(): void;
  hide(): void;
  setError(message: string): void;
  setCaptureEnabled(enabled: boolean): void;
  showStream(stream: MediaStream | null): void;
  readonly video: { videoWidth: number };
}

export interface CameraDeps {
  open: () => Promise<MediaStream>;
  waitFrame: (video: { videoWidth: number }, ms: number) => Promise<boolean>;
  frameTimeoutMs: number;
  /** How long to wait for getUserMedia (a permission prompt nobody answers must not leave the dialog hanging). Default 60 s; 0 = no limit. */
  openTimeoutMs?: number;
  /** Receives every diagnostic line as it happens (e.g. console.debug). The same lines are kept in CameraSession.diagnose().events. */
  onDiag?: (line: string) => void;
}

const NO_FRAMES = "The camera started but is not delivering images. Close other apps that use the camera and try again.";
const DISCONNECTED = "The camera was disconnected.";
const MUTED = "The camera is blocked or paused. Check the camera's privacy shutter and Windows Settings > Privacy & security > Camera.";

export interface TrackDiag { label: string; readyState: string; muted: boolean; enabled: boolean }
export interface CameraDiagnostics {
  /** Number of open() calls so far; the newest one owns the dialog. */
  generation: number;
  visible: boolean;
  /** True while real frames are arriving. */
  live: boolean;
  hasStream: boolean;
  tracks: TrackDiag[];
  lastError: string;
  streamsAcquired: number;
  streamsStopped: number;
  /** Streams that were acquired, are not stopped and are not the current one. MUST be 0: anything else is a camera-light leak. */
  leaked: number;
  /** "OK" or a one-line plain-language verdict naming the problem. */
  verdict: string;
  events: string[];
}

const MAX_EVENTS = 200;

/**
 * One open camera dialog. Guarantees:
 *  - Capture is enabled ONLY while real frames are arriving (not while opening, not after an error, not while the camera is muted/unplugged);
 *  - a permission answer that arrives after the dialog was closed, re-opened or timed out is discarded and its stream stopped;
 *  - every stream that is not in use is stopped (no leaked camera light): on close, on re-open, on error, on "no frames", on unplug;
 *  - any number of rapid open() calls leave exactly one live stream;
 *  - everything that happens is recorded: diagnose() says what state the camera is in and why.
 */
export class CameraSession {
  private seq = 0;
  private stream: MediaStream | null = null;
  private shown = false;
  private live = false;
  private lastError = "";
  private acquired = 0;
  private stopped = 0;
  private events: string[] = [];
  private readonly done = new WeakSet<MediaStream>();      // streams already stopped: stopping is idempotent, so the leak arithmetic stays exact

  constructor(
    private readonly ui: CameraUI,
    private readonly deps: CameraDeps = { open: () => openCameraStream(), waitFrame: waitForFrame, frameTimeoutMs: 15000, openTimeoutMs: 60000 },
  ) {}

  /** True while the dialog is on screen. */
  get visible(): boolean { return this.shown; }

  async open(): Promise<void> {
    this.release("re-open");
    const seq = ++this.seq;
    this.live = false;
    this.note(seq, "open requested");
    this.ui.setCaptureEnabled(false);
    this.ui.setError("");
    this.ui.show();
    this.shown = true;
    let s: MediaStream | null = null;
    try {
      s = await this.acquire(seq);
      this.acquired++;
      if (seq !== this.seq) { this.note(seq, `permission answer arrived after this request was superseded (now #${this.seq}): stream discarded and stopped`); this.stopTracked(s); return; }
      this.stream = s;
      const tracks = s.getVideoTracks();
      this.note(seq, `camera granted: ${tracks.length} video track(s)${tracks[0] ? ` "${tracks[0].label || "unnamed"}"` : ""}`);
      for (const t of tracks) {
        t.addEventListener("ended", () => { if (this.current(seq, s)) { this.note(seq, "track ENDED (camera unplugged or taken by another app)"); this.fail(seq, DISCONNECTED); } });
        t.addEventListener("mute", () => { if (this.current(seq, s)) { this.note(seq, "track MUTED (privacy shutter / Windows privacy switch / another app)"); this.ui.setCaptureEnabled(false); this.lastError = MUTED; this.ui.setError(MUTED); } });
        t.addEventListener("unmute", () => { if (this.current(seq, s) && this.live) { this.note(seq, "track unmuted: frames resume"); this.lastError = ""; this.ui.setError(""); this.ui.setCaptureEnabled(true); } });
      }
      if (tracks.length === 0) return this.fail(seq, DISCONNECTED, "the stream has no video track");
      if (tracks.some((t) => t.readyState === "ended")) return this.fail(seq, DISCONNECTED, "the track was already ended when it arrived (unplugged while the prompt was open)");
      this.ui.showStream(s);
      const ok = await this.deps.waitFrame(this.ui.video, this.deps.frameTimeoutMs);
      if (seq !== this.seq) { this.note(seq, "frame wait finished but this request was superseded"); return; }
      if (!ok) return this.fail(seq, NO_FRAMES, `no frame within ${this.deps.frameTimeoutMs} ms`);
      this.live = true;
      this.note(seq, "frames arriving: Capture enabled");
      this.ui.setCaptureEnabled(true);
    } catch (e) {
      if (seq !== this.seq) { this.note(seq, `error after being superseded (${describe(e)}): ignored`); this.stopTracked(s); return; }
      this.fail(seq, cameraErrorMessage(e), `${describe(e)}`);
    }
  }

  close(): void {
    this.seq++;
    this.live = false;
    this.note(this.seq, "close requested");
    this.release("close");
    this.detach();
    this.ui.setCaptureEnabled(false);
    this.ui.hide();
    this.shown = false;
  }

  /** Snapshot for support and for tools/doctor.mjs. Contains no images and no patient data. */
  diagnose(): CameraDiagnostics {
    const tracks: TrackDiag[] = this.stream ? this.stream.getVideoTracks().map((t) => ({ label: t.label, readyState: t.readyState, muted: t.muted, enabled: t.enabled })) : [];
    const leaked = this.acquired - this.stopped - (this.stream ? 1 : 0);
    let verdict = "OK";
    if (leaked !== 0) verdict = `LEAK: ${leaked} camera stream(s) were opened and never stopped`;
    else if (this.lastError) verdict = this.lastError;
    else if (this.shown && !this.live) verdict = this.stream ? "camera open but no frames yet" : "waiting for the camera (permission prompt?)";
    return { generation: this.seq, visible: this.shown, live: this.live, hasStream: this.stream !== null, tracks, lastError: this.lastError,
      streamsAcquired: this.acquired, streamsStopped: this.stopped, leaked, verdict, events: [...this.events] };
  }

  /** getUserMedia with a deadline; a stream that arrives after the deadline is stopped, never kept. */
  private acquire(seq: number): Promise<MediaStream> {
    const limit = this.deps.openTimeoutMs ?? 60000;
    const opening = this.deps.open();
    if (!limit) return opening;
    return new Promise<MediaStream>((resolve, reject) => {
      let settled = false;
      const timer = setTimeout(() => {
        settled = true;
        this.note(seq, `no answer from the camera/permission prompt within ${limit} ms`);
        reject(Object.assign(new Error("timed out waiting for the camera"), { name: "CameraTimeout" }));
      }, limit);
      opening.then(
        (st) => { clearTimeout(timer); if (settled) { this.acquired++; this.note(seq, "a stream arrived after the deadline: stopped"); this.stopTracked(st); } else { settled = true; resolve(st); } },
        (e) => { clearTimeout(timer); if (!settled) { settled = true; reject(e); } },
      );
    });
  }

  private current(seq: number, s: MediaStream | null): boolean { return seq === this.seq && this.stream === s; }

  /** Error state: message shown, Capture disabled, camera released (light off), dialog stays up so the message can be read. */
  private fail(seq: number, message: string, why = ""): void {
    this.live = false;
    this.lastError = message;
    this.note(seq, `FAILED: ${message}${why ? ` [${why}]` : ""}`);
    this.release("error");
    this.detach();
    this.ui.setCaptureEnabled(false);
    this.ui.setError(message);
  }

  /** Detaching is best-effort: a missing/broken video element must never stop the error path (or close()) from finishing. */
  private detach(): void {
    try { this.ui.showStream(null); } catch { /* element gone */ }
  }

  private release(reason: string): void {
    if (!this.stream) return;
    this.note(this.seq, `releasing the camera (${reason})`);
    this.stopTracked(this.stream);
    this.stream = null;
  }

  private stopTracked(s: MediaStream | null): void {
    if (!s || this.done.has(s)) return;
    this.done.add(s);
    stopStream(s);
    this.stopped++;
  }

  private note(seq: number, msg: string): void {
    const line = `[camera #${seq}] ${msg}`;
    this.events.push(line);
    if (this.events.length > MAX_EVENTS) this.events.shift();
    try { this.deps.onDiag?.(line); } catch { /* a broken logger must never break the camera */ }
  }
}

function describe(e: unknown): string {
  const n = errName(e);
  return n ? `${n}${e instanceof Error && e.message ? `: ${e.message}` : ""}` : String(e);
}
