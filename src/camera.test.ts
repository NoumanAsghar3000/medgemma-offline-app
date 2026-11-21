import { test } from "node:test";
import assert from "node:assert/strict";
import { CAMERA_LADDER, CameraSession, cameraErrorMessage, errName, openCameraStream, waitForFrame, type CameraUI } from "./camera.ts";

const err = (name: string) => Object.assign(new Error(name), { name });
const fakeStream = { id: "s" } as unknown as MediaStream;

test("every getUserMedia failure maps to an actionable message", () => {
  assert.match(cameraErrorMessage(err("NotAllowedError")), /permission/i);
  assert.match(cameraErrorMessage(err("SecurityError")), /permission/i);
  assert.match(cameraErrorMessage(err("NotFoundError")), /no camera/i);
  assert.match(cameraErrorMessage(err("NotReadableError")), /in use by another app/i);
  assert.match(cameraErrorMessage(err("OverconstrainedError")), /does not support/i);
  assert.match(cameraErrorMessage(err("NoMediaDevices")), /not available/i);
  assert.match(cameraErrorMessage(new Error("boom")), /Camera unavailable: boom/);
  assert.equal(errName("x"), "");
});

test("falls back through the constraint ladder on constraint errors only", async () => {
  const seen: MediaStreamConstraints[] = [];
  const s = await openCameraStream(async (c) => {
    seen.push(c);
    if (seen.length < 3) throw err("OverconstrainedError");
    return fakeStream;
  });
  assert.equal(s, fakeStream);
  assert.deepEqual(seen, CAMERA_LADDER);
});

test("permission denied, missing and busy cameras are final (no pointless retries)", async () => {
  for (const name of ["NotAllowedError", "NotFoundError", "NotReadableError"]) {
    let calls = 0;
    await assert.rejects(openCameraStream(async () => { calls++; throw err(name); }), { name });
    assert.equal(calls, 1, name);
  }
});

test("gives up with the last error when every ladder step fails", async () => {
  let calls = 0;
  await assert.rejects(openCameraStream(async () => { calls++; throw err("OverconstrainedError"); }), { name: "OverconstrainedError" });
  assert.equal(calls, CAMERA_LADDER.length);
});

test("waitForFrame resolves as soon as frames arrive, and reports false on timeout", async () => {
  const v = { videoWidth: 0 };
  setTimeout(() => { v.videoWidth = 640; }, 150);
  assert.equal(await waitForFrame(v, 3000, 20), true);
  assert.equal(await waitForFrame({ videoWidth: 0 }, 120, 20), false);
});

// ---------------------------------------------------------------------------------------------------------------------
// CameraSession: the dialog logic, driven with fakes (no browser needed)
// ---------------------------------------------------------------------------------------------------------------------
class FakeTrack extends EventTarget { stopped = false; readyState = "live"; stop() { this.stopped = true; this.readyState = "ended"; } }
function makeStream() {
  const track = new FakeTrack();
  const stream = { getTracks: () => [track], getVideoTracks: () => [track] } as unknown as MediaStream;
  return { stream, track };
}
function fakeUI() {
  const log: string[] = [];
  const ui = {
    visible: false, error: "", capture: false, attached: null as MediaStream | null, video: { videoWidth: 0 },
    show() { this.visible = true; log.push("show"); },
    hide() { this.visible = false; log.push("hide"); },
    setError(m: string) { this.error = m; if (m) log.push("error:" + m.slice(0, 20)); },
    setCaptureEnabled(on: boolean) { this.capture = on; log.push("capture:" + on); },
    showStream(s: MediaStream | null) { this.attached = s; },
  };
  return { ui: ui as typeof ui & CameraUI, log };
}
const frames = async () => true;
const noFrames = async () => false;
const deferred = <T,>() => { let resolve!: (v: T) => void; let reject!: (e: unknown) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };

test("Capture is disabled while opening and enabled only once frames arrive", async () => {
  const { ui } = fakeUI(); const { stream } = makeStream(); const gate = deferred<boolean>();
  const s = new CameraSession(ui, { open: async () => stream, waitFrame: () => gate.promise, frameTimeoutMs: 1 });
  const opening = s.open();
  await new Promise((r) => setTimeout(r, 5));
  assert.equal(ui.visible, true); assert.equal(ui.capture, false, "no frames yet -> Capture must stay disabled"); assert.equal(ui.attached, stream);
  gate.resolve(true); await opening;
  assert.equal(ui.capture, true); assert.equal(ui.error, ""); assert.equal(s.visible, true);
});

test("a failed open shows the actionable message and keeps Capture disabled", async () => {
  for (const [name, rx] of [["NotAllowedError", /permission/i], ["NotFoundError", /no camera/i], ["NotReadableError", /in use/i]] as const) {
    const { ui } = fakeUI();
    const s = new CameraSession(ui, { open: async () => { throw err(name); }, waitFrame: frames, frameTimeoutMs: 1 });
    await s.open();
    assert.match(ui.error, rx, name); assert.equal(ui.capture, false, name); assert.equal(ui.visible, true, "dialog stays up so the user can read it");
  }
});

test("a camera that opens but never delivers frames is reported, Capture stays disabled", async () => {
  const { ui } = fakeUI(); const { stream } = makeStream();
  await new CameraSession(ui, { open: async () => stream, waitFrame: noFrames, frameTimeoutMs: 1 }).open();
  assert.match(ui.error, /not delivering images/); assert.equal(ui.capture, false);
});

test("closing while the permission prompt is open discards the late stream and stops it", async () => {
  const { ui } = fakeUI(); const { stream, track } = makeStream(); const permission = deferred<MediaStream>();
  const s = new CameraSession(ui, { open: () => permission.promise, waitFrame: frames, frameTimeoutMs: 1 });
  const opening = s.open();
  s.close();                                   // user pressed Cancel / Escape
  permission.resolve(stream); await opening;   // ... and only now Windows answers "allowed"
  assert.equal(track.stopped, true, "the late stream must be stopped (camera light off)");
  assert.equal(ui.visible, false); assert.equal(ui.attached, null); assert.equal(ui.capture, false); assert.equal(s.visible, false);
});

test("re-opening while a previous open is pending: only the newest wins", async () => {
  const { ui } = fakeUI(); const first = makeStream(); const second = makeStream(); const p1 = deferred<MediaStream>(); const p2 = deferred<MediaStream>();
  const queue = [p1, p2];
  const s = new CameraSession(ui, { open: () => queue.shift()!.promise, waitFrame: frames, frameTimeoutMs: 1 });
  const a = s.open(); const b = s.open();
  p2.resolve(second.stream); await b;
  p1.resolve(first.stream); await a;
  assert.equal(first.track.stopped, true); assert.equal(second.track.stopped, false); assert.equal(ui.attached, second.stream); assert.equal(ui.capture, true);
});

test("close() stops every track, detaches the stream and hides the dialog; a second open starts clean", async () => {
  const { ui } = fakeUI(); const a = makeStream(); const b = makeStream(); const streams = [a, b];
  const s = new CameraSession(ui, { open: async () => streams.shift()!.stream, waitFrame: frames, frameTimeoutMs: 1 });
  await s.open(); assert.equal(ui.capture, true);
  s.close(); assert.equal(a.track.stopped, true); assert.equal(ui.attached, null); assert.equal(ui.visible, false); assert.equal(ui.capture, false);
  await s.open(); assert.equal(ui.visible, true); assert.equal(ui.capture, true); assert.equal(ui.attached, b.stream); assert.equal(ui.error, "");
});

test("an unplugged camera disables Capture and says so", async () => {
  const { ui } = fakeUI(); const { stream, track } = makeStream();
  await new CameraSession(ui, { open: async () => stream, waitFrame: frames, frameTimeoutMs: 1 }).open();
  assert.equal(ui.capture, true);
  track.dispatchEvent(new Event("ended"));
  assert.equal(ui.capture, false); assert.match(ui.error, /disconnected/);
});

test("an 'ended' event from an old, closed session is ignored", async () => {
  const { ui } = fakeUI(); const { stream, track } = makeStream();
  const s = new CameraSession(ui, { open: async () => stream, waitFrame: frames, frameTimeoutMs: 1 });
  await s.open(); s.close(); ui.error = "";
  track.dispatchEvent(new Event("ended"));
  assert.equal(ui.error, "");
});

test("a camera unplugged while the permission prompt was open is reported at once and the stream is stopped", async () => {
  const { ui } = fakeUI(); const { stream, track } = makeStream(); track.readyState = "ended";
  await new CameraSession(ui, { open: async () => stream, waitFrame: frames, frameTimeoutMs: 1 }).open();
  assert.match(ui.error, /disconnected/); assert.equal(ui.capture, false); assert.equal(track.stopped, true);
});

test("a camera that never delivers frames is RELEASED (camera light off), not left running behind the error", async () => {
  const { ui } = fakeUI(); const { stream, track } = makeStream();
  await new CameraSession(ui, { open: async () => stream, waitFrame: noFrames, frameTimeoutMs: 1 }).open();
  assert.match(ui.error, /not delivering images/); assert.equal(track.stopped, true); assert.equal(ui.attached, null);
});

test("unplugging the camera releases it as well as disabling Capture", async () => {
  const { ui } = fakeUI(); const { stream, track } = makeStream();
  await new CameraSession(ui, { open: async () => stream, waitFrame: frames, frameTimeoutMs: 1 }).open();
  track.dispatchEvent(new Event("ended"));
  assert.equal(track.stopped, true); assert.equal(ui.capture, false); assert.equal(ui.attached, null);
});

test("a muted camera (privacy shutter / Windows privacy switch) disables Capture and re-enables it when frames return", async () => {
  const { ui } = fakeUI(); const { stream, track } = makeStream();
  await new CameraSession(ui, { open: async () => stream, waitFrame: frames, frameTimeoutMs: 1 }).open();
  track.dispatchEvent(new Event("mute"));
  assert.equal(ui.capture, false); assert.match(ui.error, /blocked or paused/);
  track.dispatchEvent(new Event("unmute"));
  assert.equal(ui.capture, true); assert.equal(ui.error, "");
});

test("a mute that happens before the first frame does not enable Capture on unmute", async () => {
  const { ui } = fakeUI(); const { stream, track } = makeStream(); const gate = deferred<boolean>();
  const opening = new CameraSession(ui, { open: async () => stream, waitFrame: () => gate.promise, frameTimeoutMs: 1 }).open();
  await new Promise((r) => setTimeout(r, 5));
  track.dispatchEvent(new Event("mute")); track.dispatchEvent(new Event("unmute"));
  assert.equal(ui.capture, false, "no frame has arrived yet");
  gate.resolve(true); await opening; assert.equal(ui.capture, true);
});

test("a permission prompt nobody answers times out with a clear message; a late answer is stopped, not kept", async () => {
  const { ui } = fakeUI(); const { stream, track } = makeStream(); const permission = deferred<MediaStream>();
  await new CameraSession(ui, { open: () => permission.promise, waitFrame: frames, frameTimeoutMs: 1, openTimeoutMs: 30 }).open();
  assert.match(ui.error, /Timed out waiting for the camera/); assert.equal(ui.capture, false);
  permission.resolve(stream); await new Promise((r) => setTimeout(r, 5));
  assert.equal(track.stopped, true, "the stream that arrived after the deadline must be stopped");
});

test("if attaching the stream to the page throws, the stream is stopped and the error shown", async () => {
  const { ui } = fakeUI(); const { stream, track } = makeStream();
  ui.showStream = () => { throw new Error("video element missing"); };
  await new CameraSession(ui, { open: async () => stream, waitFrame: frames, frameTimeoutMs: 1 }).open();
  assert.match(ui.error, /video element missing/); assert.equal(track.stopped, true); assert.equal(ui.capture, false);
});

test("many rapid open() calls in any order leave exactly ONE live stream", async () => {
  for (const order of [[0, 1, 2, 3, 4, 5], [5, 4, 3, 2, 1, 0], [2, 5, 0, 4, 1, 3]]) {
    const { ui } = fakeUI(); const made = order.map(() => makeStream()); const gates = order.map(() => deferred<MediaStream>()); let n = 0;
    const s = new CameraSession(ui, { open: () => gates[n++].promise, waitFrame: frames, frameTimeoutMs: 1 });
    const calls = order.map(() => s.open());
    for (const i of order) gates[i].resolve(made[i].stream);                       // the permission answers arrive in a scrambled order
    await Promise.all(calls);
    const live = made.filter((m) => !m.track.stopped);
    assert.equal(live.length, 1, `order ${order}: ${live.length} streams still running`);
    assert.equal(ui.attached, live[0].stream, "the newest open() owns the stream");
    s.close(); assert.equal(made.filter((m) => !m.track.stopped).length, 0, "close() stops everything");
  }
});

test("close() is idempotent and safe before any open()", () => {
  const { ui } = fakeUI(); const s = new CameraSession(ui, { open: async () => makeStream().stream, waitFrame: frames, frameTimeoutMs: 1 });
  s.close(); s.close(); assert.equal(ui.visible, false); assert.equal(s.visible, false);
});
