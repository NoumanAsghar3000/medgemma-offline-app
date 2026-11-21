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
const serve = mode === "ok" || mode === "portfile" || (mode === "second" && n >= 2);
if (!serve) { setInterval(() => {}, 1000); }
else {
  const port = mode === "portfile" ? reqPort + 7 : reqPort;
  const wss = new WebSocketServer({ noServer: true });
  const srv = http.createServer((req, res) => { if (req.url === "/json") { res.setHeader("content-type", "application/json"); res.end(JSON.stringify([{ type: "page", webSocketDebuggerUrl: "ws://127.0.0.1:" + port + "/devtools/page/1" }])); } else { res.statusCode = 404; res.end(); } });
  srv.on("upgrade", (req, sock, head) => wss.handleUpgrade(req, sock, head, (ws) => ws.on("message", (x) => { const { id } = JSON.parse(x); ws.send(JSON.stringify({ id, result: {} })); })));
  srv.listen(port, "127.0.0.1");
  if (mode === "portfile") { const d = path.join(process.env.WEBVIEW2_USER_DATA_FOLDER, "EBWebView"); fs.mkdirSync(d, { recursive: true }); fs.writeFileSync(path.join(d, "DevToolsActivePort"), port + "\\n/devtools/browser/x"); }
}
`;

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
