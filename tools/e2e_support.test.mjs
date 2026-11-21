import { test } from "node:test";
import assert from "node:assert/strict";
import net from "node:net";
import { analyzeBrowserArgs, buildBrowserArgs, DEFAULT_FEATURES, finalVerdict, parseDevToolsActivePort, pickFreePort } from "./e2e_support.mjs";

test("browser args carry wry's default, the debug port, and the fake camera flags", () => {
  const a = buildBrowserArgs(9444);
  for (const f of [DEFAULT_FEATURES, "--remote-debugging-port=9444", "--remote-allow-origins=*", "--use-fake-device-for-media-stream", "--use-fake-ui-for-media-stream"]) assert.ok(a.includes(f), f);
});

test("pickFreePort skips ports that are in use", async () => {
  const start = 31000 + Math.floor(Math.random() * 2000);
  const hog = net.createServer(); await new Promise((r) => hog.listen(start, "127.0.0.1", r));
  try { assert.equal(await pickFreePort(start, 10), start + 1); } finally { await new Promise((r) => hog.close(r)); }
  assert.equal(await pickFreePort(start, 10), start, "free again once released");
});

test("DevToolsActivePort parsing", () => {
  assert.equal(parseDevToolsActivePort("9333\n/devtools/browser/abc"), 9333);
  assert.equal(parseDevToolsActivePort("41234\r\n/devtools/browser/abc\r\n"), 41234);
  for (const bad of ["", "abc", "0\n", "70000\nx", null, undefined, "-5"]) assert.equal(parseDevToolsActivePort(bad), null, String(bad));
});

const WV = (extra) => `4120 "C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\Application\\130.0\\msedgewebview2.exe" --embedded-browser-webview=1 ${extra}`;

test("verdict: no WebView2 process at all", () => {
  const v = analyzeBrowserArgs("5000 C:\\app\\medgemma-offline.exe", 9333);
  assert.equal(v.code, "NO_WEBVIEW2"); assert.match(v.message, /never started/);
});

test("verdict: WebView2 runs but the browser args were NOT applied (the exact failure to diagnose)", () => {
  const v = analyzeBrowserArgs([WV("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection"), WV("--type=renderer")].join("\n"), 9333);
  assert.equal(v.code, "ARGS_NOT_ACCEPTED"); assert.equal(v.webviewProcesses, 2); assert.equal(v.hasPort, false);
  assert.match(v.message, /MG_TEST_BROWSER_ARGS/); assert.match(v.message, /NOT applied/);
});

test("verdict: the flag reached the browser process but nothing listens", () => {
  const v = analyzeBrowserArgs(WV("--remote-debugging-port=9333 --use-fake-device-for-media-stream --use-fake-ui-for-media-stream"), 9333);
  assert.equal(v.code, "FLAG_PRESENT_NO_LISTENER"); assert.equal(v.hasFakeDevice, true); assert.equal(v.hasFakeUi, true); assert.match(v.message, /RemoteDebuggingAllowed/);
});

test("a flag for a DIFFERENT port does not count", () => {
  assert.equal(analyzeBrowserArgs(WV("--remote-debugging-port=9444"), 9333).code, "ARGS_NOT_ACCEPTED");
});

test("empty / missing input is handled", () => {
  assert.equal(analyzeBrowserArgs("", 9333).code, "NO_WEBVIEW2");
  assert.equal(analyzeBrowserArgs(undefined, 9333).code, "NO_WEBVIEW2");
});

// ---------------------------------------------------------------------------------------------------------------------
// finalVerdict: the one plain sentence a human reads first
// ---------------------------------------------------------------------------------------------------------------------
const refused = { state: "refused", detail: "nothing is listening on 127.0.0.1:9333", tally: { refused: 75 } };
const NOARGS = analyzeBrowserArgs(WV("--disable-features=msWebOOUI"), 9333);
const FLAGGED = analyzeBrowserArgs(WV("--remote-debugging-port=9333 --use-fake-device-for-media-stream --use-fake-ui-for-media-stream"), 9333);
const NOPROC = analyzeBrowserArgs("5000 C:\\app\\medgemma-offline.exe", 9333);

test("verdict headlines name the failure in plain words", () => {
  const v = (o) => finalVerdict({ port: 9333, ...o });
  assert.equal(v({ probe: { state: "ok" } }).headline, "DEBUG PORT OK");
  assert.equal(v({ probe: refused, analysis: NOARGS }).headline, "WEBVIEW2 ARGS NOT APPLIED");
  assert.equal(v({ probe: refused, analysis: NOPROC }).headline, "WEBVIEW2 DID NOT START");
  assert.equal(v({ probe: refused, analysis: FLAGGED }).headline, "DEBUG PORT NOT DETECTED");
  assert.equal(v({ probe: refused }).headline, "DEBUG PORT NOT DETECTED", "without command lines (not Windows) the port itself is what is reported");
  assert.equal(v({ probe: refused, analysis: NOARGS, appExited: true, appExitCode: 3 }).headline, "APP EXITED BEFORE THE DEBUG PORT OPENED");
  assert.equal(v({ probe: { state: "not-json", detail: "x" } }).headline, "DEBUG PORT BELONGS TO ANOTHER PROGRAM");
  assert.equal(v({ probe: { state: "http-error", detail: "HTTP 503" } }).code, "PORT_TAKEN");
  assert.equal(v({ probe: { state: "no-page-target", detail: "targets: browser" } }).headline, "DEBUG PORT OPEN, BUT NO PAGE");
});

test("every non-OK verdict carries an explanation and concrete next steps", () => {
  for (const probe of [refused, { state: "timeout", detail: "no answer from http://127.0.0.1:9333/json within 3000 ms" }, { state: "error", detail: "EACCES: permission denied" },
                       { state: "no-page-target", detail: "DevTools is listening but exposes no page (targets: browser)" }, { state: "not-json", detail: "another program listens on this port" }]) {
    for (const analysis of [undefined, NOARGS, NOPROC, FLAGGED]) {
      const r = finalVerdict({ probe, analysis, port: 9333 });
      assert.ok(r.headline && r.explanation.length > 10 && r.nextSteps.length >= 1 && r.code !== "OK", JSON.stringify({ probe, analysis: analysis?.code, r }));
    }
  }
  assert.deepEqual(finalVerdict({ probe: { state: "ok" }, port: 1 }).nextSteps, []);
});

test("ARGS_NOT_ACCEPTED points at the exact hook; FLAG_PRESENT warns when the fake-camera flags are missing", () => {
  assert.match(finalVerdict({ probe: refused, analysis: NOARGS, port: 9333 }).nextSteps.join(" "), /create_main_window\(\).*MG_TEST_BROWSER_ARGS/);
  const onlyPort = analyzeBrowserArgs(WV("--remote-debugging-port=9333"), 9333);
  assert.match(finalVerdict({ probe: refused, analysis: onlyPort, port: 9333 }).explanation, /fake-camera flags are missing/);
  assert.doesNotMatch(finalVerdict({ probe: refused, analysis: FLAGGED, port: 9333 }).explanation, /fake-camera flags are missing/);
});

test("the explanation includes what the probe saw (tally), so 'never listened' is distinguishable from 'listened then died'", () => {
  const e = finalVerdict({ probe: { state: "refused", detail: "d", tally: { ok: 0, refused: 74, timeout: 1 } }, analysis: FLAGGED, port: 9333 }).explanation;
  assert.match(e, /"refused":74/); assert.match(e, /"timeout":1/);
});

test("realistic Windows command lines: quoted paths, many processes, port 0, similar-looking flags", () => {
  const lines = [
    '4120 "C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\Application\\131.0.2903.51\\msedgewebview2.exe" --embedded-browser-webview=1 --webview-exe-name=medgemma-offline.exe --user-data-dir="D:\\a\\_temp\\e2e-work\\webview2-data\\EBWebView" --remote-debugging-port=9333 --remote-allow-origins=* --use-fake-device-for-media-stream --use-fake-ui-for-media-stream',
    '4200 "C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\Application\\131.0.2903.51\\msedgewebview2.exe" --type=gpu-process',
    '5000 D:\\a\\_temp\\MedGemmaInstalled\\medgemma-offline.exe',
  ].join("\r\n");
  const a = analyzeBrowserArgs(lines, 9333);
  assert.deepEqual([a.code, a.webviewProcesses, a.hasPort, a.hasFakeDevice, a.hasFakeUi], ["FLAG_PRESENT_NO_LISTENER", 2, true, true, true]);
  assert.equal(analyzeBrowserArgs(lines, 933).hasPort, false, "a prefix of the port number must not match");
  assert.equal(analyzeBrowserArgs(lines.replace("--remote-debugging-port=9333", "--remote-debugging-port=93331"), 9333).hasPort, false, "a longer port number must not match");
});

test("FUZZ: arbitrary text never throws and always yields a coded verdict", () => {
  let x = 12345; const rnd = () => { x = (Math.imul(x, 1664525) + 1013904223) >>> 0; return x / 2 ** 32; };
  const bits = ["msedgewebview2.exe", "--remote-debugging-port=9333", "--remote-debugging-port=", "\r\n", "  ", "medgemma-offline.exe", "--use-fake-device-for-media-stream", "\u0000", "é漢字", "--", "9333"];
  for (let i = 0; i < 3000; i++) {
    let text = ""; for (let k = Math.floor(rnd() * 12); k > 0; k--) text += bits[Math.floor(rnd() * bits.length)] + (rnd() < 0.5 ? " " : "");
    const a = analyzeBrowserArgs(text, 9333);
    assert.ok(["NO_WEBVIEW2", "ARGS_NOT_ACCEPTED", "FLAG_PRESENT_NO_LISTENER"].includes(a.code));
    const v = finalVerdict({ probe: refused, analysis: a, port: 9333 });
    assert.ok(v.headline.length > 0 && v.nextSteps.length > 0);
  }
});
