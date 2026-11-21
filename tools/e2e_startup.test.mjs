// Runs the REAL tools/e2e_webview.mjs against stub "apps" that imitate WebView2's debug endpoint in different ways, to prove the startup logic:
// endpoint found, app restarted once on a fresh data folder, WebView2's own DevToolsActivePort followed, and a clear verdict when it never comes up.
// Linux/macOS only (the stub is a shell wrapper); on Windows CI the real app takes its place.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const wsUrl = import.meta.resolve("ws");

const STUB = `
import http from "node:http"; import fs from "node:fs"; import path from "node:path";
const { WebSocketServer } = await import(${JSON.stringify(wsUrl)});
const mode = process.env.STUB_MODE;
const m = /--remote-debugging-port=(\\d+)/.exec(process.env.MG_TEST_BROWSER_ARGS || ""); const reqPort = m ? Number(m[1]) : 0;
let n = 1; const cf = process.env.STUB_COUNTER;
if (cf) { n = (fs.existsSync(cf) ? Number(fs.readFileSync(cf, "utf8")) : 0) + 1; fs.writeFileSync(cf, String(n)); }
if (mode === "exit") process.exit(7);
if (mode === "flagged" && process.env.STUB_CMDLINES) {
  fs.writeFileSync(process.env.STUB_CMDLINES, '4120 "C:\\\\Program Files (x86)\\\\Microsoft\\\\EdgeWebView\\\\Application\\\\131.0\\\\msedgewebview2.exe" --embedded-browser-webview=1 ' + (process.env.MG_TEST_BROWSER_ARGS || ""));
}
const serve = mode === "ok" || mode === "portfile" || mode === "taken" || mode === "nopage" || (mode === "second" && n >= 2);
if (!serve) { setInterval(() => {}, 1000); }
else {
  const port = mode === "portfile" ? reqPort + 7 : reqPort;
  const wss = new WebSocketServer({ noServer: true });
  const srv = http.createServer((req, res) => { if (mode === "taken") { res.end("<html>some other program</html>"); return; } if (mode === "nopage" && req.url === "/json") { res.setHeader("content-type", "application/json"); res.end(JSON.stringify([{ type: "browser" }])); return; } if (req.url === "/json") { res.setHeader("content-type", "application/json"); res.end(JSON.stringify([{ type: "page", webSocketDebuggerUrl: "ws://127.0.0.1:" + port + "/devtools/page/1" }])); } else { res.statusCode = 404; res.end(); } });
  srv.on("upgrade", (req, sock, head) => wss.handleUpgrade(req, sock, head, (ws) => ws.on("message", (x) => { const { id } = JSON.parse(x); ws.send(JSON.stringify({ id, result: {} })); })));
  srv.listen(port, "127.0.0.1");
  if (mode === "portfile") { const d = path.join(process.env.WEBVIEW2_USER_DATA_FOLDER, "EBWebView"); fs.mkdirSync(d, { recursive: true }); fs.writeFileSync(path.join(d, "DevToolsActivePort"), port + "\\n/devtools/browser/x"); }
}
`;

function cmdlines(dir, text) { const f = path.join(dir, "cmdlines.txt"); writeFileSync(f, text); return f; }
const WVP = (flags) => `4120 "C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\Application\\131.0\\msedgewebview2.exe" --embedded-browser-webview=1 ${flags}`;

function setup(mode) {
  const dir = mkdtempSync(path.join(os.tmpdir(), "mg_e2e_stub_"));
  const stub = path.join(dir, "stub.mjs"); writeFileSync(stub, STUB);
  const wrapper = path.join(dir, "app.sh"); writeFileSync(wrapper, `#!/bin/sh\nexec "${process.execPath}" "${stub}"\n`); chmodSync(wrapper, 0o755);
  mkdirSync(path.join(dir, "models"), { recursive: true });
  return { dir, wrapper, env: { ...process.env, APP_EXE: wrapper, GOOD_MODELS: path.join(dir, "models"), WORK_DIR: path.join(dir, "work"), STUB_MODE: mode,
    STUB_COUNTER: path.join(dir, "count.txt"), E2E_ENDPOINT_WAIT_1: "3", E2E_ENDPOINT_WAIT_2: "3", E2E_RETRY_PAUSE_MS: "200" } };
}

/** Runs the e2e script until `until(stdout)` is true or it exits (or 60 s), then kills its whole process group. */
function runE2E(env, until) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [path.join(here, "e2e_webview.mjs")], { env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
    let out = ""; let done = false;
    const finish = (code) => { if (done) return; done = true; clearTimeout(timer); try { process.kill(-child.pid, "SIGKILL"); } catch {} resolve({ out, code }); };
    const onData = (d) => { out += d; if (until && until(out)) finish(null); };
    child.stdout.on("data", onData); child.stderr.on("data", onData);
    child.on("exit", (c) => finish(c));
    const timer = setTimeout(() => finish("timeout"), 60000);
  });
}

const skip = process.platform === "win32" ? "stub app is a shell wrapper" : false;

test("the debug endpoint is found on the first start", { skip }, async () => {
  const s = setup("ok");
  const r = await runE2E(s.env, (o) => o.includes("debug endpoint up"));
  assert.match(r.out, /PASS  WebView2 accepted the browser arguments \(debug endpoint up\).*attempt 1/, r.out);
  assert.match(r.out, /launching the app \(attempt 1/);
});

test("the app receives the browser arguments (debug port + fake camera) through MG_TEST_BROWSER_ARGS", { skip }, async () => {
  const s = setup("ok");
  const r = await runE2E(s.env, (o) => o.includes("debug endpoint up"));
  assert.match(r.out, /debug port (\d+)/); const port = /debug port (\d+)/.exec(r.out)[1];
  assert.ok(Number(port) >= 9333, "a free port at or above 9333 is chosen");
});

test("when the first start exposes nothing, the app is restarted ONCE on a fresh user-data folder and attempt 2 succeeds", { skip }, async () => {
  const s = setup("second");
  const r = await runE2E(s.env, (o) => o.includes("debug endpoint up"));
  assert.match(r.out, /restarting the app once with a fresh user-data folder/, r.out);
  assert.match(r.out, /launching the app \(attempt 2, .*webview2-data-2/, r.out);
  assert.match(r.out, /PASS  WebView2 accepted the browser arguments \(debug endpoint up\).*attempt 2/, r.out);
  assert.equal(readFileSync(path.join(s.dir, "count.txt"), "utf8"), "2");
});

test("WebView2's own DevToolsActivePort is followed when it listens on a different port than requested", { skip }, async () => {
  const s = setup("portfile");
  const r = await runE2E(s.env, (o) => o.includes("debug endpoint up"));
  assert.match(r.out, /WebView2 reports its debug port as \d+ \(expected \d+\): following it/, r.out);
  assert.match(r.out, /PASS  WebView2 accepted the browser arguments/, r.out);
});

test("when the endpoint never comes up: both attempts, full diagnostics, a plain VERDICT, a failing check and exit code 1", { skip }, async () => {
  const s = setup("never");
  const r = await runE2E(s.env, null);
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /--- DIAG attempt 1/); assert.match(r.out, /--- DIAG attempt 2/);
  assert.match(r.out, /--- VERDICT \[(NO_WEBVIEW2|ARGS_NOT_ACCEPTED|FLAG_PRESENT_NO_LISTENER)\]/);
  assert.match(r.out, /FAIL  WebView2 accepted the browser arguments and exposed the debug endpoint/);
  assert.match(r.out, /debugging endpoint never came up \[/);
  const results = JSON.parse(readFileSync(path.join(s.dir, "work", "e2e-results.json"), "utf8"));
  assert.ok(results.some((x) => !x.ok && x.required && /accepted the browser arguments/.test(x.name)), "failure is recorded for the report");
  assert.ok(existsSync(path.join(s.dir, "work", "app.log")));
});

/** Runs a failing startup with simulated Windows process command lines and returns { out, banner, verdict }. */
async function failing(mode, cmdText) {
  const s = setup(mode);
  if (cmdText !== undefined) s.env.E2E_CMDLINES_FILE = cmdlines(s.dir, cmdText);
  if (mode === "flagged") { s.env.E2E_CMDLINES_FILE = path.join(s.dir, "flagged-cmdlines.txt"); s.env.STUB_CMDLINES = s.env.E2E_CMDLINES_FILE; }
  const r = await runE2E(s.env, null);
  const banner = /=== E2E FINAL VERDICT: (.*) ===/.exec(r.out)?.[1];
  const verdict = JSON.parse(readFileSync(path.join(s.dir, "work", "e2e-verdict.json"), "utf8"));
  return { ...r, banner, verdict };
}

test("VERDICT: WebView2 runs but the browser args were NOT applied -> 'WEBVIEW2 ARGS NOT APPLIED'", { skip }, async () => {
  const r = await failing("never", WVP("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection") + "\n" + WVP("--type=renderer"));
  assert.equal(r.code, 1); assert.equal(r.banner, "WEBVIEW2 ARGS NOT APPLIED", r.out);
  assert.equal(r.verdict.code, "ARGS_NOT_ACCEPTED"); assert.ok(r.verdict.failedChecks.some((n) => /accepted the browser arguments/.test(n)), JSON.stringify(r.verdict.failedChecks));
  assert.match(r.out, /create_main_window\(\)/, "the next steps name the exact hook");
});

test("VERDICT: the flag reached the browser process (real port in its command line) but nothing listens -> 'DEBUG PORT NOT DETECTED'", { skip }, async () => {
  const r = await failing("flagged");
  assert.equal(r.banner, "DEBUG PORT NOT DETECTED", r.out); assert.equal(r.verdict.code, "FLAG_PRESENT_NO_LISTENER");
  assert.match(r.verdict.explanation, /reached the browser process but nothing listens/); assert.match(r.out, /fake-camera flags in the browser process: device=true ui=true/);
});

test("VERDICT: no WebView2 process at all -> 'WEBVIEW2 DID NOT START'", { skip }, async () => {
  const r = await failing("never", "5000 C:\\app\\medgemma-offline.exe");
  assert.equal(r.banner, "WEBVIEW2 DID NOT START"); assert.equal(r.verdict.code, "NO_WEBVIEW2");
});

test("VERDICT: the app exits before the port opens -> 'APP EXITED BEFORE THE DEBUG PORT OPENED' with the exit code", { skip }, async () => {
  const r = await failing("exit", "");
  assert.equal(r.banner, "APP EXITED BEFORE THE DEBUG PORT OPENED"); assert.equal(r.verdict.code, "APP_EXITED"); assert.match(r.verdict.explanation, /exit code 7/);
});

test("VERDICT: another program owns the port -> 'DEBUG PORT BELONGS TO ANOTHER PROGRAM'", { skip }, async () => {
  const r = await failing("taken", "");
  assert.equal(r.banner, "DEBUG PORT BELONGS TO ANOTHER PROGRAM"); assert.equal(r.verdict.code, "PORT_TAKEN"); assert.equal(r.verdict.probe.state, "not-json");
});

test("VERDICT: DevTools listens but there is no page -> 'DEBUG PORT OPEN, BUT NO PAGE'", { skip }, async () => {
  const r = await failing("nopage", "");
  assert.equal(r.banner, "DEBUG PORT OPEN, BUT NO PAGE"); assert.equal(r.verdict.code, "NO_PAGE"); assert.equal(r.verdict.probe.state, "no-page-target");
});

test("the verdict file records the probe tally, the attempts and the port for the report", { skip }, async () => {
  const r = await failing("never", "5000 C:\\app\\medgemma-offline.exe");
  assert.equal(r.verdict.attempts, 2); assert.ok(r.verdict.port >= 9333); assert.ok(r.verdict.probe.tally.refused >= 1, JSON.stringify(r.verdict.probe));
  assert.ok(r.out.includes("--- debug port probe (attempt 1") && r.out.includes("--- debug port probe (attempt 2"));
});
