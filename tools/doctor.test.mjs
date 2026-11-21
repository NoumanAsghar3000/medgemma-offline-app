// Tests the doctor: HEALTH parsing and verdicts (against the REAL mg-probe binary when it is built), camera interpretation, and the whole
// doctor run against a MOCK PAGE that executes the REAL, bundled CameraSession + the simulated Windows camera behind a mock DevTools server.
import { test, after } from "node:test";
import assert from "node:assert/strict";
import { spawn, execFileSync, spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";
import { buildSync } from "esbuild";
import { WebSocketServer } from "ws";
import { CAMERA_PROBE_JS, checkModelHealth, formatReport, interpretBackend, interpretCamera, parseHealth, runDoctor } from "./doctor_lib.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..");
const PROBE = [path.join(repo, "target/debug/mg-probe"), path.join(repo, "target/release/mg-probe")].find(existsSync);
const DATA = path.join(repo, "crates/mg-core/tests/data");
const cleanups = []; after(() => { for (const c of cleanups) try { c(); } catch {} });
const tmp = (p) => mkdtempSync(path.join(os.tmpdir(), `mg_doctor_${p}_`));
const byName = (r, n) => r.checks.find((c) => c.name.startsWith(n));

/* ---------------------------------------------------------------- HEALTH block: parsing + verdicts ---------------------------------------------------------------- */

const SAMPLE = `junk before
--- HEALTH (machine readable) ---
HEALTH.verdict=DEGRADED
HEALTH.files=2
HEALTH.family.27B=no-projector
HEALTH.family.4B=ready
HEALTH.file.0.name=alpha.gguf
HEALTH.file.0.kind=weights
HEALTH.file.0.family=MedGemma 27B
HEALTH.file.0.quant=F16
HEALTH.file.0.quant_source=tensors
HEALTH.file.0.size=51000
HEALTH.file.0.expected_min_size=60000
HEALTH.file.0.header=read
HEALTH.file.0.state=bad
HEALTH.file.0.detail=the file is INCOMPLETE (a=b)
HEALTH.file.1.name=beta.gguf
HEALTH.file.1.kind=weights
HEALTH.file.1.family=MedGemma 4B
HEALTH.file.1.quant=Q8_0
HEALTH.file.1.quant_source=file_type
HEALTH.file.1.size=4100
HEALTH.file.1.expected_min_size=-
HEALTH.file.1.header=read
HEALTH.file.1.state=ok
--- END HEALTH ---
junk after`;

test("parseHealth reads the block, including '=' inside values and a missing expected size", () => {
  const h = parseHealth(SAMPLE);
  assert.equal(h.verdict, "DEGRADED"); assert.equal(h.files.length, 2); assert.equal(h.family27, "no-projector");
  assert.equal(h.files[0].detail, "the file is INCOMPLETE (a=b)"); assert.equal(h.files[0].expectedMinSize, 60000); assert.equal(h.files[1].expectedMinSize, null);
  assert.equal(parseHealth("no block here"), null); assert.equal(parseHealth(undefined), null);
});

test("checkModelHealth: OK / DEGRADED / BROKEN / EMPTY / old probe -> PASS / WARN / FAIL / FAIL / FAIL with the reason", () => {
  const mk = (verdict, extra = "") => parseHealth(`--- HEALTH (machine readable) ---\nHEALTH.verdict=${verdict}\nHEALTH.files=0\nHEALTH.family.27B=ready\nHEALTH.family.4B=ready${extra}\n--- END HEALTH ---`);
  assert.equal(checkModelHealth(mk("OK")).status, "PASS");
  assert.equal(checkModelHealth(mk("DEGRADED")).status, "WARN");
  assert.equal(checkModelHealth(mk("BROKEN")).status, "FAIL");
  const e = checkModelHealth(mk("EMPTY")); assert.equal(e.status, "FAIL"); assert.match(e.detail, /no \.gguf files/);
  const old = checkModelHealth(null); assert.equal(old.status, "FAIL"); assert.match(old.detail, /old build/);
  const d = checkModelHealth(parseHealth(SAMPLE)); assert.match(d.detail, /alpha\.gguf: the file is INCOMPLETE/);
});

const folder = (files) => { const d = tmp("models"); for (const [name, fixture] of files) copyFileSync(path.join(DATA, fixture), path.join(d, name)); return d; };
const HEALTHY = [["a-final.gguf", "hdr-27b-q8.gguf"], ["a-vision.gguf", "hdr-clip-5376.gguf"], ["b-final.gguf", "hdr-4b-q4km.gguf"], ["b-vision.gguf", "hdr-clip-2560.gguf"]];

test("REAL mg-probe doctor: healthy folder -> OK, files identified by content under meaningless names", { skip: PROBE ? false : "mg-probe is not built (cargo build -p mg-probe)" }, async () => {
  const r = await runDoctor({ probeExe: PROBE, modelsDir: folder(HEALTHY) });
  const c = byName(r, "Model header health");
  assert.equal(c.status, "PASS", c.detail); assert.match(c.detail, /27B: ready, 4B: ready/); assert.match(c.detail, /a-final\.gguf = MedGemma 27B/); assert.equal(r.ok, true);
});

test("REAL mg-probe doctor: 27B without its projector -> WARN naming the problem; only projectors -> FAIL; empty -> FAIL", { skip: PROBE ? false : "mg-probe is not built" }, async () => {
  const warn = byName(await runDoctor({ probeExe: PROBE, modelsDir: folder([HEALTHY[0], HEALTHY[2], HEALTHY[3]]) }), "Model header health");
  assert.equal(warn.status, "WARN"); assert.match(warn.detail, /27B: no-projector/);
  const broken = byName(await runDoctor({ probeExe: PROBE, modelsDir: folder([HEALTHY[1], HEALTHY[3]]) }), "Model header health");
  assert.equal(broken.status, "FAIL"); assert.match(broken.detail, /no usable model/);
  const empty = byName(await runDoctor({ probeExe: PROBE, modelsDir: tmp("empty") }), "Model header health");
  assert.equal(empty.status, "FAIL"); assert.match(empty.detail, /no \.gguf files/);
});

test("the doctor CLI prints a report and exits 1 on a failure, 0 otherwise", { skip: PROBE ? false : "mg-probe is not built" }, () => {
  const ok = spawnSync(process.execPath, [path.join(here, "doctor.mjs"), "--models", folder(HEALTHY), "--probe", PROBE, "--work", tmp("work")], { encoding: "utf8" });
  assert.equal(ok.status, 0, ok.stdout + ok.stderr); assert.match(ok.stdout, /=== DOCTOR: ALL CHECKS PASSED ===/); assert.match(ok.stdout, /\[PASS\] Model header health/); assert.match(ok.stdout, /\[skip\] Browser args accepted/);
  const bad = spawnSync(process.execPath, [path.join(here, "doctor.mjs"), "--models", tmp("empty2"), "--probe", PROBE, "--work", tmp("work")], { encoding: "utf8" });
  assert.equal(bad.status, 1); assert.match(bad.stdout, /=== DOCTOR: 1 PROBLEM\(S\) - Model header health ===/);
  const missing = spawnSync(process.execPath, [path.join(here, "doctor.mjs"), "--models", tmp("e3"), "--probe", "/nonexistent/mg-probe", "--work", tmp("work")], { encoding: "utf8" });
  assert.equal(missing.status, 1); assert.match(missing.stdout, /mg-probe was not found/);
});

/* ---------------------------------------------------------------- interpretation tables ---------------------------------------------------------------- */

test("interpretCamera: every real-world situation gets its own plain verdict", () => {
  const diag = (o = {}) => ({ camera: { leaked: 0, verdict: "OK", visible: false, streamsAcquired: 2, streamsStopped: 2, ...o }, backend: null });
  const base = { hasMediaDevices: true, videoInputs: 1, permission: "granted", diag: diag(), error: null };
  assert.equal(interpretCamera(base).status, "PASS");
  assert.match(interpretCamera({ ...base, hasMediaDevices: false }).detail, /getUserMedia does not exist/);
  assert.match(interpretCamera({ ...base, videoInputs: 0 }).detail, /NO video input device/);
  assert.match(interpretCamera({ ...base, permission: "denied" }).detail, /permission is DENIED/);
  assert.equal(interpretCamera({ ...base, diag: null }).status, "WARN");
  const leak = interpretCamera({ ...base, diag: diag({ leaked: 2, verdict: "LEAK: 2 camera stream(s) were opened and never stopped" }) }); assert.equal(leak.status, "FAIL"); assert.match(leak.detail, /CAMERA LEAK/);
  assert.equal(interpretCamera({ ...base, diag: diag({ visible: true, verdict: "waiting for the camera (permission prompt?)" }) }).status, "WARN");
  assert.equal(interpretCamera(undefined).status, "FAIL"); assert.equal(interpretCamera("nope").status, "FAIL");
});

test("interpretBackend: ready / waiting / loading / error", () => {
  const b = (state, extra = {}) => interpretBackend({ diag: { backend: { state, family: "MedGemma 4B", quant: "Q8_0", compute: "cpu", multimodal: true, ...extra } } });
  assert.equal(b("ready").status, "PASS"); assert.match(b("ready").detail, /MedGemma 4B Q8_0 on CPU with vision/);
  assert.equal(b("need_model_folder").status, "WARN"); assert.equal(b("loading").status, "WARN"); assert.equal(b("error", { note: "out of memory" }).status, "FAIL");
  assert.equal(interpretBackend({}).status, "SKIP");
});

/* ---------------------------------------------------------------- the mock PAGE: real CameraSession + simulated camera, behind a DevTools server ---------------------------------------------------------------- */

const BUNDLE = buildSync({ stdin: { contents: `export * from "./src/camera.ts"; export * from "./src/camera.sim.ts";`, resolveDir: repo, loader: "ts" },
  bundle: true, format: "iife", globalName: "MG", write: false, target: "es2020", logLevel: "silent" }).outputFiles[0].text;

function startPage(scenario = {}, { leaky = false } = {}) {
  const ctx = vm.createContext({ setTimeout, clearTimeout, EventTarget, Event, console });
  vm.runInContext(BUNDLE, ctx);
  vm.runInContext(`
    var window = globalThis;
    var els = {}; var mk = (id, extra) => (els[id] = Object.assign({ disabled: false, hidden: false, innerText: '', click() {} }, extra));
    var video = new MG.SimVideo(8); var sim = new MG.SimDevices(${JSON.stringify(scenario)});
    mk('plus'); mk('cam', { hidden: true }); mk('cam-shot', { disabled: true }); mk('cam-err'); mk('m-cam'); mk('cam-cancel'); mk('file');
    var session = new MG.CameraSession({
      show() { els['cam'].hidden = false; }, hide() { els['cam'].hidden = true; }, setError(m) { els['cam-err'].innerText = m; },
      setCaptureEnabled(on) { els['cam-shot'].disabled = !on; }, showStream(s) { video.attach(s); }, get video() { return video; },
    }, { open: () => MG.openCameraStream(sim.getUserMedia), waitFrame: MG.waitForFrame, frameTimeoutMs: 300, openTimeoutMs: 800 });
    ${leaky ? "MG.CameraSession.prototype.release = function () {};   // a build that forgets to stop the camera" : ""}
    els['m-cam'].click = () => { session.open(); }; els['cam-cancel'].click = () => session.close();
    var document = { getElementById: (id) => els[id] || null, querySelector: (sel) => els[sel.replace(/^#/, '')] || null };
    var navigator = { mediaDevices: { getUserMedia: sim.getUserMedia, enumerateDevices: async () => sim.cameras.map(() => ({ kind: 'videoinput' })) },
                      permissions: { query: async () => ({ state: sim.permission === 'denied' ? 'denied' : 'granted' }) } };
    window.__mgDiagnostics = () => ({ camera: session.diagnose(), backend: { state: 'ready', family: 'MedGemma 4B', quant: 'Q8_0', compute: 'cpu', multimodal: true, note: '' } });
  `, ctx);
  const srv = http.createServer((req, res) => { if (req.url === "/json") { res.setHeader("content-type", "application/json"); res.end(JSON.stringify([{ type: "page", webSocketDebuggerUrl: `ws://127.0.0.1:${srv.address().port}/devtools/page/1` }])); } else { res.statusCode = 404; res.end(); } });
  const wss = new WebSocketServer({ noServer: true });
  srv.on("upgrade", (req, sock, head) => wss.handleUpgrade(req, sock, head, (ws) => ws.on("message", async (m) => {
    const { id, method, params } = JSON.parse(m);
    if (method !== "Runtime.evaluate") return ws.send(JSON.stringify({ id, result: {} }));
    try { const value = await vm.runInContext(params.expression, ctx); ws.send(JSON.stringify({ id, result: { result: { value } } })); }
    catch (e) { ws.send(JSON.stringify({ id, result: { exceptionDetails: { text: "boom", exception: { description: String(e.message) } } } })); }
  })));
  return new Promise((resolve) => srv.listen(0, "127.0.0.1", () => {
    const stop = () => { for (const c of wss.clients) c.terminate(); wss.close(); srv.closeAllConnections?.(); srv.close(); };
    cleanups.push(stop);
    resolve({ port: srv.address().port, sim: ctx.sim, session: ctx.session, stop });
  }));
}
const FLAGS = (port) => `4120 "C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\Application\\131.0\\msedgewebview2.exe" --embedded-browser-webview=1 --remote-debugging-port=${port} --use-fake-device-for-media-stream --use-fake-ui-for-media-stream`;
const doctorOn = (page, extra = {}) => runDoctor({ attachPort: page.port, waitSeconds: 5, exerciseCamera: true, readCommandLines: () => FLAGS(page.port), ...extra });

test("the page probe expression itself runs and reports devices, permission and the app's diagnostics", async () => {
  const page = await startPage();
  const cdpMod = await import("./cdp.mjs"); const cdp = cdpMod.createCdp(); await cdp.connect(`ws://127.0.0.1:${page.port}/devtools/page/1`);
  const raw = await cdp.ev(CAMERA_PROBE_JS); cdp.close();
  assert.deepEqual([raw.hasMediaDevices, raw.videoInputs, raw.permission, raw.diag.camera.leaked, raw.diag.backend.state], [true, 1, "granted", 0, "ready"]);
});

test("DOCTOR on a healthy app: every check passes and the camera is released afterwards", async () => {
  const page = await startPage();
  const r = await doctorOn(page);
  for (const n of ["Browser args accepted", "Debug port listening", "Camera readiness", "Backend status", "Camera open/close cycle"]) assert.equal(byName(r, n).status, "PASS", `${n}: ${byName(r, n).detail}`);
  assert.equal(r.headline, "DOCTOR: ALL CHECKS PASSED"); assert.equal(r.ok, true); assert.equal(page.sim.liveCount, 0, "the doctor left the camera on");
  assert.match(byName(r, "Camera open/close cycle").detail, /released on close/);
});

test("DOCTOR: permission denied -> camera readiness AND the real dialog cycle fail, with the app's own message", async () => {
  const page = await startPage({ permission: "denied" });
  const r = await doctorOn(page);
  assert.equal(byName(r, "Camera readiness").status, "FAIL"); assert.match(byName(r, "Camera readiness").detail, /permission is DENIED/);
  const cycle = byName(r, "Camera open/close cycle"); assert.equal(cycle.status, "FAIL"); assert.match(cycle.detail, /permission was denied/i);
  assert.equal(r.ok, false); assert.match(r.headline, /PROBLEM\(S\).*Camera/); assert.equal(page.sim.liveCount, 0);
});

test("DOCTOR: no camera connected -> 'NO video input device'", async () => {
  const r = await doctorOn(await startPage({ cameras: [] }));
  assert.match(byName(r, "Camera readiness").detail, /NO video input device/); assert.match(byName(r, "Camera open/close cycle").detail, /No camera was found/);
});

test("DOCTOR: camera busy (another app) -> readiness passes, the real cycle names the cause", async () => {
  const page = await startPage({ busy: true });
  const r = await doctorOn(page);
  assert.equal(byName(r, "Camera readiness").status, "PASS");
  assert.match(byName(r, "Camera open/close cycle").detail, /in use by another app/); assert.equal(page.sim.liveCount, 0);
});

test("DOCTOR: a build that forgets to stop the camera is caught -> 'STILL HELD ... camera light stays on' (and the camera really is on)", async () => {
  const page = await startPage({}, { leaky: true });
  const r = await doctorOn(page);
  const cycle = byName(r, "Camera open/close cycle");
  assert.equal(cycle.status, "FAIL"); assert.match(cycle.detail, /STILL HELD/);
  assert.equal(page.sim.liveCount, 1, "the simulated camera light really is still on: the doctor's verdict is true");
});

test("DOCTOR: a rear-camera constraint the webcam rejects still works (ladder fallback), and the report is readable", async () => {
  const r = await doctorOn(await startPage({ rejectFacingMode: true }));
  assert.equal(byName(r, "Camera open/close cycle").status, "PASS");
  const text = formatReport(r); assert.match(text, /\[PASS\] Camera readiness/); assert.match(text, /=== DOCTOR: ALL CHECKS PASSED ===/);
});

/* ---------------------------------------------------------------- launching the app ---------------------------------------------------------------- */

const STUB = (wsUrl) => `
import http from "node:http"; import fs from "node:fs";
const { WebSocketServer } = await import(${JSON.stringify(wsUrl)});
fs.writeFileSync(process.env.STUB_PID, String(process.pid));
const m = /--remote-debugging-port=(\\d+)/.exec(process.env.MG_TEST_BROWSER_ARGS || ""); const port = m ? Number(m[1]) : 0;
if (process.env.STUB_MODE !== "ok") { setInterval(() => {}, 1000); }
else {
  const wss = new WebSocketServer({ noServer: true });
  const srv = http.createServer((q, res) => { res.setHeader("content-type", "application/json"); res.end(JSON.stringify([{ type: "page", webSocketDebuggerUrl: "ws://127.0.0.1:" + port + "/p" }])); });
  srv.on("upgrade", (q, s, h) => wss.handleUpgrade(q, s, h, (ws) => ws.on("message", (x) => { const { id } = JSON.parse(x); ws.send(JSON.stringify({ id, result: {} })); })));
  srv.listen(port, "127.0.0.1");
}`;
function stubApp(mode) {
  const dir = tmp("stub"); const pidFile = path.join(dir, "pid");
  writeFileSync(path.join(dir, "stub.mjs"), STUB(import.meta.resolve("ws")));
  const exe = path.join(dir, "app.sh"); writeFileSync(exe, `#!/bin/sh\nexport STUB_MODE=${mode} STUB_PID=${pidFile}\nexec "${process.execPath}" "${path.join(dir, "stub.mjs")}"\n`); chmodSync(exe, 0o755);
  return { exe, dir, alive: () => { try { process.kill(Number(readFileSync(pidFile, "utf8")), 0); return true; } catch { return false; } } };
}
const skipWin = process.platform === "win32" ? "stub app is a shell wrapper" : false;

test("DOCTOR launches the app, finds the port, reads the page, and KILLS the app afterwards", { skip: skipWin }, async () => {
  const s = stubApp("ok");
  const r = await runDoctor({ appExe: s.exe, workDir: s.dir, waitSeconds: 8, readCommandLines: () => "" });
  assert.equal(byName(r, "Debug port listening").status, "PASS"); assert.equal(byName(r, "Browser args accepted").status, "PASS", "a listening port proves the args were applied");
  assert.equal(byName(r, "Camera readiness").status, "FAIL", "this stub page has no camera support: the doctor says so instead of crashing");
  await new Promise((r2) => setTimeout(r2, 300)); assert.equal(s.alive(), false, "the doctor left the app running");
});

test("DOCTOR: args not applied -> 'WEBVIEW2 ARGS NOT APPLIED' in the Browser-args check, port check FAILS, camera SKIPPED, app killed", { skip: skipWin }, async () => {
  const s = stubApp("never");
  const lines = `4120 "C:\\\\EdgeWebView\\\\msedgewebview2.exe" --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`;
  const r = await runDoctor({ appExe: s.exe, workDir: s.dir, waitSeconds: 3, readCommandLines: () => lines });
  assert.equal(byName(r, "Browser args accepted").status, "FAIL"); assert.match(byName(r, "Browser args accepted").detail, /WEBVIEW2 ARGS NOT APPLIED/);
  assert.equal(byName(r, "Debug port listening").status, "FAIL"); assert.equal(byName(r, "Camera readiness").status, "SKIP");
  assert.equal(r.startup.headline, "WEBVIEW2 ARGS NOT APPLIED"); assert.match(formatReport(r), /Startup verdict: WEBVIEW2 ARGS NOT APPLIED/); assert.match(formatReport(r), /create_main_window\(\)/);
  await new Promise((r2) => setTimeout(r2, 300)); assert.equal(s.alive(), false);
});

test("DOCTOR: flag present but nothing listens -> 'DEBUG PORT NOT DETECTED'; no process -> 'WEBVIEW2 DID NOT START'", { skip: skipWin }, async () => {
  const s1 = stubApp("never");
  const r1 = await runDoctor({ appExe: s1.exe, workDir: s1.dir, waitSeconds: 2, readCommandLines: () => `4120 "x\\\\msedgewebview2.exe" --remote-debugging-port=9333 --remote-debugging-port=${9333}` });
  assert.ok(["DEBUG PORT NOT DETECTED", "WEBVIEW2 ARGS NOT APPLIED"].includes(r1.startup.headline), r1.startup.headline);   // the real port differs from 9333 when 9333 is busy
  const s2 = stubApp("never");
  const r2 = await runDoctor({ appExe: s2.exe, workDir: s2.dir, waitSeconds: 2, readCommandLines: () => "" });
  assert.equal(r2.startup.headline, "WEBVIEW2 DID NOT START");
});

test("DOCTOR with nothing to inspect skips everything and still gives a verdict", async () => {
  const r = await runDoctor({});
  assert.ok(r.checks.every((c) => c.status === "SKIP")); assert.equal(r.headline, "DOCTOR: ALL CHECKS PASSED"); assert.equal(r.ok, true);
});
