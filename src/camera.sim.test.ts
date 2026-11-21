import { test } from "node:test";
import assert from "node:assert/strict";
import { CameraSession, openCameraStream, waitForFrame } from "./camera.ts";
import { SimDevices, SimVideo, lcg, simUI, sleep, type SimOptions } from "./camera.sim.ts";

/** A session wired to the simulated devices through the REAL openCameraStream (constraint ladder included) and the REAL waitForFrame. */
function rig(o: SimOptions = {}, video = new SimVideo(8), over: { frameTimeoutMs?: number; openTimeoutMs?: number } = {}) {
  const sim = new SimDevices(o);
  const { ui, state } = simUI(video);
  const lines: string[] = [];
  const session = new CameraSession(ui, { open: () => openCameraStream(sim.getUserMedia), waitFrame: waitForFrame, frameTimeoutMs: over.frameTimeoutMs ?? 300, openTimeoutMs: over.openTimeoutMs ?? 500, onDiag: (l) => lines.push(l) });
  return { sim, ui, state, session, video, lines };
}
const settle = () => sleep(60);

test("happy path: Capture is enabled only after the first frame, the diagnostics say OK", async () => {
  const r = rig();
  await r.session.open();
  assert.equal(r.state.visible, true); assert.equal(r.state.capture, true); assert.equal(r.state.captureWithoutFrames, 0);
  const d = r.session.diagnose();
  assert.equal(d.verdict, "OK"); assert.equal(d.live, true); assert.equal(d.leaked, 0); assert.equal(d.tracks[0].label, "Integrated Camera");
  assert.ok(d.events.some((e) => /camera granted: 1 video track/.test(e)) && d.events.some((e) => /Capture enabled/.test(e)));
  r.session.close(); assert.equal(r.sim.liveCount, 0, "camera light off after close");
});

test("late permission: the user closes the dialog while the Windows prompt is open, then clicks Allow -> nothing stays on", async () => {
  const r = rig({ permission: "prompt" });
  const opening = r.session.open();
  await sleep(20);
  assert.equal(r.state.capture, false); assert.match(r.session.diagnose().verdict, /waiting for the camera/);
  r.session.close();                                  // Cancel / Escape
  r.sim.answerPrompt("granted"); await opening; await settle();
  assert.equal(r.sim.liveCount, 0, "the late stream must be stopped");
  assert.equal(r.state.visible, false); assert.equal(r.state.captureWithoutFrames, 0);
  const d = r.session.diagnose(); assert.equal(d.leaked, 0); assert.ok(d.events.some((e) => /superseded.*discarded and stopped/.test(e)), d.events.join("\n"));
});

test("a prompt nobody answers: timeout message, and the stream that arrives much later is stopped", async () => {
  const r = rig({ permission: "prompt" }, new SimVideo(8), { openTimeoutMs: 80 });
  await r.session.open();
  assert.match(r.state.error, /Timed out waiting for the camera/); assert.equal(r.state.capture, false);
  r.sim.answerPrompt("granted"); await settle();
  assert.equal(r.sim.liveCount, 0); assert.equal(r.session.diagnose().leaked, 0);
  assert.ok(r.session.diagnose().events.some((e) => /arrived after the deadline: stopped/.test(e)));
});

test("permission denied: actionable message, never any stream, verdict says so", async () => {
  const r = rig({ permission: "denied" });
  await r.session.open();
  assert.match(r.state.error, /permission was denied/i); assert.equal(r.state.capture, false); assert.equal(r.sim.tracks.length, 0);
  assert.equal(r.sim.calls.length, 1, "a denial is final: the ladder must not retry");
  assert.match(r.session.diagnose().verdict, /permission/i); assert.equal(r.state.visible, true, "dialog stays so the message can be read");
});

test("no camera / camera busy: precise messages, final (no retries)", async () => {
  const none = rig({ cameras: [] }); await none.session.open();
  assert.match(none.state.error, /No camera was found/); assert.equal(none.sim.calls.length, 1);
  const busy = rig({ busy: true }); await busy.session.open();
  assert.match(busy.state.error, /in use by another app/); assert.equal(busy.sim.calls.length, 1); assert.equal(busy.sim.liveCount, 0);
});

test("rear-camera constraint rejected: the ladder falls back and the camera works", async () => {
  const r = rig({ rejectFacingMode: true });
  await r.session.open();
  assert.equal(r.state.capture, true); assert.equal(r.sim.calls.length, 2, "facingMode attempt rejected, plain video accepted");
  assert.equal(r.session.diagnose().verdict, "OK"); r.session.close(); assert.equal(r.sim.liveCount, 0);
});

test("the camera is unplugged while live: Capture off, camera released, message and verdict", async () => {
  const r = rig();
  await r.session.open(); assert.equal(r.state.capture, true);
  r.sim.unplugAll();
  assert.equal(r.state.capture, false); assert.match(r.state.error, /disconnected/); assert.equal(r.sim.liveCount, 0);
  const d = r.session.diagnose(); assert.match(d.verdict, /disconnected/); assert.equal(d.hasStream, false); assert.ok(d.events.some((e) => /track ENDED/.test(e)));
  await r.session.open();                                   // plugging it back in and re-opening works
  assert.equal(r.state.capture, true); assert.equal(r.state.error, "");
});

test("unplugged during the permission prompt: reported at once, no stream left", async () => {
  const r = rig({ permission: "prompt" });
  const opening = r.session.open(); await sleep(10);
  r.sim.answerPrompt("granted"); r.sim.unplugAll();         // answered, and the device is gone in the same instant
  await opening; await settle();
  assert.equal(r.sim.liveCount, 0); assert.equal(r.state.capture, false);
});

test("privacy shutter: mute disables Capture, unmute restores it; no frames are 'captured' while muted", async () => {
  const r = rig();
  await r.session.open();
  r.sim.tracks[0].setMuted(true);
  assert.equal(r.state.capture, false); assert.match(r.state.error, /blocked or paused/); assert.match(r.session.diagnose().verdict, /blocked or paused/);
  r.sim.tracks[0].setMuted(false);
  assert.equal(r.state.capture, true); assert.equal(r.state.error, ""); assert.equal(r.session.diagnose().verdict, "OK");
});

test("a camera that opens but never produces frames is released and reported", async () => {
  const r = rig({}, new SimVideo(8, true), { frameTimeoutMs: 80 });
  await r.session.open();
  assert.match(r.state.error, /not delivering images/); assert.equal(r.state.capture, false); assert.equal(r.sim.liveCount, 0);
  assert.ok(r.session.diagnose().events.some((e) => /no frame within 80 ms/.test(e)));
});

test("multiple open attempts: only the newest survives and the camera light never shows two streams", async () => {
  const r = rig({ answerAfterMs: (() => { const q = [60, 5, 30, 1]; return () => q.shift() ?? 5; })() });
  const calls = [r.session.open(), r.session.open(), r.session.open(), r.session.open()];
  for (let i = 0; i < 10; i++) { await sleep(8); assert.ok(r.sim.liveCount <= 1, `two cameras on at once (step ${i})`); }
  await Promise.all(calls); await settle();
  assert.equal(r.sim.liveCount, 1); assert.equal(r.state.capture, true); assert.equal(r.session.diagnose().leaked, 0);
  assert.equal(r.session.diagnose().generation, 4);
  r.session.close(); assert.equal(r.sim.liveCount, 0);
});

test("STORM: random open / close / unplug / mute sequences never leak a stream or enable Capture without frames", async () => {
  for (const seed of [1, 2, 3, 7, 42, 99]) {
    const rnd = lcg(seed);
    const r = rig({ answerAfterMs: () => Math.floor(rnd() * 25) }, new SimVideo(6), { frameTimeoutMs: 120, openTimeoutMs: 400 });
    const pending: Promise<void>[] = [];
    for (let step = 0; step < 60; step++) {
      const x = rnd();
      if (x < 0.40) pending.push(r.session.open());
      else if (x < 0.60) r.session.close();
      else if (x < 0.72) r.sim.unplugAll();
      else if (x < 0.82) { const t = r.sim.tracks[r.sim.tracks.length - 1]; if (t && t.readyState === "live") t.setMuted(rnd() < 0.5); }
      await sleep(Math.floor(rnd() * 14));
      assert.ok(r.sim.liveCount <= 1, `seed ${seed} step ${step}: ${r.sim.liveCount} cameras on at once`);
    }
    await Promise.all(pending); await settle();
    r.session.close(); await settle();
    const d = r.session.diagnose();
    assert.equal(r.sim.liveCount, 0, `seed ${seed}: camera light still on`);
    assert.equal(d.leaked, 0, `seed ${seed}: ${d.verdict}`);
    assert.equal(d.streamsAcquired, d.streamsStopped, `seed ${seed}: acquired ${d.streamsAcquired} stopped ${d.streamsStopped}`);
    assert.equal(r.state.captureWithoutFrames, 0, `seed ${seed}: Capture was enabled while no frame had arrived`);
    assert.equal(r.state.visible, false);
  }
});

test("diagnostics are emitted as they happen, are bounded, and never contain image data", async () => {
  const r = rig({ permission: "denied" });
  await r.session.open(); await r.session.open();
  assert.ok(r.lines.length >= 4); assert.ok(r.lines.every((l) => l.startsWith("[camera #")));
  assert.ok(r.lines.some((l) => /FAILED: Camera permission was denied.*NotAllowedError/.test(l)), r.lines.join("\n"));
  for (let i = 0; i < 500; i++) { r.session.close(); }
  assert.ok(r.session.diagnose().events.length <= 200, "the event log is capped");
  assert.ok(!JSON.stringify(r.session.diagnose()).includes("data:image"));
});

test("a throwing logger never breaks the camera", async () => {
  const sim = new SimDevices(); const video = new SimVideo(5); const { ui, state } = simUI(video);
  const s = new CameraSession(ui, { open: () => openCameraStream(sim.getUserMedia), waitFrame: waitForFrame, frameTimeoutMs: 200, onDiag: () => { throw new Error("logger broke"); } });
  await s.open(); assert.equal(state.capture, true); s.close(); assert.equal(sim.liveCount, 0);
});
