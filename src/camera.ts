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
}

const NO_FRAMES = "The camera started but is not delivering images. Close other apps that use the camera and try again.";
const DISCONNECTED = "The camera was disconnected.";

/**
 * One open camera dialog. Guarantees: Capture is enabled ONLY while real frames are arriving; a permission answer that arrives after the
 * dialog was closed (or re-opened) is discarded and its stream stopped; every stream is stopped on close; an unplugged camera disables Capture.
 */
export class CameraSession {
  private seq = 0;
  private stream: MediaStream | null = null;
  private shown = false;

  constructor(
    private readonly ui: CameraUI,
    private readonly deps: CameraDeps = { open: () => openCameraStream(), waitFrame: waitForFrame, frameTimeoutMs: 15000 },
  ) {}

  /** True while the dialog is on screen. */
  get visible(): boolean { return this.shown; }

  async open(): Promise<void> {
    this.release();
    const seq = ++this.seq;
    this.ui.setCaptureEnabled(false);
    this.ui.setError("");
    this.ui.show();
    this.shown = true;
    try {
      const s = await this.deps.open();
      if (seq !== this.seq) { stopStream(s); return; }          // closed or re-opened while the permission prompt was up
      this.stream = s;
      for (const t of s.getVideoTracks()) {
        t.addEventListener("ended", () => {
          if (seq !== this.seq || this.stream !== s) return;
          this.ui.setError(DISCONNECTED);
          this.ui.setCaptureEnabled(false);
        });
      }
      this.ui.showStream(s);
      const ok = await this.deps.waitFrame(this.ui.video, this.deps.frameTimeoutMs);
      if (seq !== this.seq) return;
      if (ok) this.ui.setCaptureEnabled(true); else this.ui.setError(NO_FRAMES);
    } catch (e) {
      if (seq === this.seq) this.ui.setError(cameraErrorMessage(e));
    }
  }

  close(): void {
    this.seq++;
    this.release();
    this.ui.showStream(null);
    this.ui.setCaptureEnabled(false);
    this.ui.hide();
    this.shown = false;
  }

  private release(): void {
    stopStream(this.stream);
    this.stream = null;
  }
}
