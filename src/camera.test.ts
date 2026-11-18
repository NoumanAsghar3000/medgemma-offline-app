import { test } from "node:test";
import assert from "node:assert/strict";
import { CAMERA_LADDER, cameraErrorMessage, errName, openCameraStream, waitForFrame } from "./camera.ts";

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
