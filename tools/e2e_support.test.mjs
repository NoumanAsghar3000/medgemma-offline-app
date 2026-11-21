import { test } from "node:test";
import assert from "node:assert/strict";
import net from "node:net";
import { analyzeBrowserArgs, buildBrowserArgs, DEFAULT_FEATURES, parseDevToolsActivePort, pickFreePort } from "./e2e_support.mjs";

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
