// A realistic SIMULATION of Windows / WebView2 camera behaviour (no browser, no hardware), used by the tests and by tools/doctor.mjs.
// It reproduces what matters: a permission prompt that answers late or never, denial, a busy device, a device that disappears, a privacy
// shutter (mute), a rejected rear-camera constraint and - above all - whether every opened camera was STOPPED (the camera light).
import type { CameraUI } from "./camera";

const domErr = (name: string) => Object.assign(new Error(`simulated ${name}`), { name });

export class SimTrack extends EventTarget {
  readyState: "live" | "ended" = "live";
  muted = false;
  enabled = true;
  stopCalls = 0;
  constructor(readonly label: string, private readonly onGone: (t: SimTrack) => void) { super(); }
  /** MediaStreamTrack.stop(): releases the device, does NOT fire "ended" (as in real browsers). */
  stop(): void { this.stopCalls++; if (this.readyState === "live") { this.readyState = "ended"; this.onGone(this); } }
  /** The device vanished (USB unplugged / taken by another app): "ended" fires. */
  unplug(): void { if (this.readyState === "ended") return; this.readyState = "ended"; this.onGone(this); this.dispatchEvent(new Event("ended")); }
  setMuted(m: boolean): void { if (this.muted === m) return; this.muted = m; this.dispatchEvent(new Event(m ? "mute" : "unmute")); }
}

export interface SimOptions {
  cameras?: string[];
  /** granted | denied | prompt (stays pending until answerPrompt()). */
  permission?: "granted" | "denied" | "prompt";
  /** Delay before getUserMedia settles (ms); a function lets a test randomise it. */
  answerAfterMs?: number | (() => number);
  /** Another app holds the camera. */
  busy?: boolean;
  /** Reject any facingMode constraint with OverconstrainedError (a laptop webcam asked for the rear camera "exactly"). */
  rejectFacingMode?: boolean;
}

export class SimDevices {
  cameras: string[]; permission: "granted" | "denied" | "prompt"; answerAfterMs: number | (() => number); busy: boolean; rejectFacingMode: boolean;
  readonly calls: MediaStreamConstraints[] = [];
  readonly tracks: SimTrack[] = [];
  private readonly live = new Set<SimTrack>();
  private readonly prompts: Array<(r: "granted" | "denied") => void> = [];

  constructor(o: SimOptions = {}) {
    this.cameras = o.cameras ?? ["Integrated Camera"]; this.permission = o.permission ?? "granted"; this.answerAfterMs = o.answerAfterMs ?? 5;
    this.busy = o.busy ?? false; this.rejectFacingMode = o.rejectFacingMode ?? false;
  }

  /** Cameras currently switched on: the camera LIGHT. Must be 0 whenever no dialog is using the camera. */
  get liveCount(): number { return this.live.size; }

  /** The user answers the Windows permission prompt. */
  answerPrompt(r: "granted" | "denied"): void { for (const p of this.prompts.splice(0)) p(r); }
  unplugAll(): void { for (const t of [...this.live]) t.unplug(); }

  readonly getUserMedia = (c: MediaStreamConstraints): Promise<MediaStream> => new Promise((resolve, reject) => {
    this.calls.push(c);
    const delay = typeof this.answerAfterMs === "function" ? this.answerAfterMs() : this.answerAfterMs;
    const settle = (decision: "granted" | "denied") => {
      if (this.cameras.length === 0) return reject(domErr("NotFoundError"));
      if (decision === "denied") return reject(domErr("NotAllowedError"));
      if (this.busy) return reject(domErr("NotReadableError"));
      const video = typeof c.video === "object" ? c.video : undefined;
      if (this.rejectFacingMode && video && "facingMode" in video) return reject(domErr("OverconstrainedError"));
      const t = new SimTrack(this.cameras[0], (x) => this.live.delete(x));
      this.live.add(t); this.tracks.push(t);
      resolve({ id: `sim-${this.tracks.length}`, getTracks: () => [t], getVideoTracks: () => [t] } as unknown as MediaStream);
    };
    if (this.permission === "prompt") this.prompts.push(settle);
    else setTimeout(() => settle(this.permission as "granted" | "denied"), delay);
  });
}

/** The <video> element: delivers frames a little after a live, un-muted stream is attached. */
export class SimVideo {
  videoWidth = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  constructor(public frameDelayMs = 8, public neverFrames = false) {}
  attach(stream: MediaStream | null): void {
    if (this.timer) clearTimeout(this.timer);
    this.videoWidth = 0;
    if (!stream || this.neverFrames) return;
    const first = stream.getVideoTracks()[0];
    this.timer = setTimeout(() => { if (first && first.readyState === "live") this.videoWidth = 640; }, this.frameDelayMs);
  }
}

export interface SimUIState { visible: boolean; error: string; capture: boolean; attached: MediaStream | null; captureHistory: boolean[]; captureWithoutFrames: number }

/** The dialog. `captureWithoutFrames` counts the forbidden moment: Capture enabled while no frame has arrived. */
export function simUI(video: SimVideo): { ui: CameraUI; state: SimUIState } {
  const state: SimUIState = { visible: false, error: "", capture: false, attached: null, captureHistory: [], captureWithoutFrames: 0 };
  const ui: CameraUI = {
    show() { state.visible = true; },
    hide() { state.visible = false; },
    setError(m) { state.error = m; },
    setCaptureEnabled(on) { state.capture = on; state.captureHistory.push(on); if (on && video.videoWidth === 0) state.captureWithoutFrames++; },
    showStream(s) { state.attached = s; video.attach(s); },
    get video() { return video; },
  };
  return { ui, state };
}

/** Small deterministic random generator so storm tests are reproducible. */
export function lcg(seed: number): () => number {
  let x = seed >>> 0 || 1;
  return () => { x = (Math.imul(x, 1664525) + 1013904223) >>> 0; return x / 0x100000000; };
}
export const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
