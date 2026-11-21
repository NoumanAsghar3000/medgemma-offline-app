// Tests the DevTools client against a MOCK DevTools WebSocket server whose Runtime.evaluate runs in a vm with a fake DOM.
import { test, after } from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { WebSocketServer } from "ws";
import http from "node:http";
import net from "node:net";
import { createCdp, probeEndpoint, sleep, waitForEndpoint } from "./cdp.mjs";

function freePort() { return 20000 + Math.floor(Math.random() * 20000); }

// every server and client is registered so that a FAILING test can never leave an open handle behind (which would hang the run)
const servers = []; const clients = [];
const newCdp = (opts) => { const c = createCdp(opts); clients.push(c); return c; };
after(() => { for (const c of clients) c.close(); for (const m of servers) { for (const sock of m.wss.clients) sock.terminate(); m.wss.close(); } });

/** Starts a mock CDP server. `dom` is the fake document seen by Runtime.evaluate. `hang` = methods that are never answered. */
function mockServer({ port, dom, hang = [] }) {
  const wss = new WebSocketServer({ port });
  servers.push({ wss });
  const ctx = vm.createContext({ document: dom, Event: class { constructor(t) { this.type = t; } } });
  wss.on("connection", (sock) => {
    sock.on("message", async (m) => {
      const { id, method, params } = JSON.parse(m);
      if (hang.includes(method)) return;
      if (method === "Runtime.evaluate") {
        try { const value = await vm.runInContext(params.expression, ctx); sock.send(JSON.stringify({ id, result: { result: { value } } })); }
        catch (e) { sock.send(JSON.stringify({ id, result: { exceptionDetails: { text: "boom", exception: { description: String(e.message) } } } })); }
      } else if (method === "DOM.getDocument") { sock.send(JSON.stringify({ id, result: { root: { nodeId: 1 } } }));
      } else if (method === "DOM.querySelector") { sock.send(JSON.stringify({ id, result: { nodeId: 2 } }));
      } else { sock.send(JSON.stringify({ id, result: {} })); }
    });
  });
  return { wss, url: `ws://127.0.0.1:${port}`, ctx };
}
const stop = (m) => new Promise((r) => { for (const c of m.wss.clients) c.terminate(); m.wss.close(() => r()); });

function fakeDom() {
  const els = {};
  const dom = {
    els,
    querySelector: (sel) => els[sel] ?? null,
    getElementById: (id) => els["#" + id] ?? null,
  };
  return dom;
}

test("connect retries while the endpoint is still starting", async () => {
  const port = freePort(); const cdp = newCdp();
  const connecting = cdp.connect(`ws://127.0.0.1:${port}`, { attempts: 6, delayMs: 500 });
  await sleep(1300);                                   // the first attempts are refused: nothing is listening yet
  const m = mockServer({ port, dom: fakeDom() });
  await connecting;
  assert.equal(await cdp.ev("1+1"), 2);
  cdp.close(); await stop(m);
});

test("connect gives up with the real error when nothing ever listens", async () => {
  const cdp = newCdp();
  await assert.rejects(cdp.connect(`ws://127.0.0.1:${freePort()}`, { attempts: 2, delayMs: 50 }), /ECONNREFUSED|connect/);
});

test("a command the server never answers times out instead of hanging", async () => {
  const port = freePort(); const m = mockServer({ port, dom: fakeDom(), hang: ["Page.enable"] }); const cdp = newCdp({ timeoutMs: 300 });
  await cdp.connect(m.url);
  await assert.rejects(cdp.send("Page.enable"), /timed out: Page\.enable/);
  assert.equal(await cdp.ev("40+2"), 42, "the connection stays usable after a timeout");
  cdp.close(); await stop(m);
});

test("a closed socket rejects every pending command at once", async () => {
  const port = freePort(); const m = mockServer({ port, dom: fakeDom(), hang: ["Page.enable"] }); const cdp = newCdp({ timeoutMs: 20000 });
  await cdp.connect(m.url);
  const p = cdp.send("Page.enable");
  setTimeout(() => { for (const c of m.wss.clients) c.terminate(); }, 100);   // the app crashed
  await assert.rejects(p, /connection closed/);
  cdp.close(); await stop(m);
});

test("page exceptions surface as errors", async () => {
  const port = freePort(); const m = mockServer({ port, dom: fakeDom() }); const cdp = newCdp();
  await cdp.connect(m.url);
  await assert.rejects(cdp.ev("nope.nothing"), /nope is not defined/);
  cdp.close(); await stop(m);
});

test("click waits for a DISABLED button to become enabled, then clicks exactly once", async () => {
  const port = freePort(); const dom = fakeDom(); let clicks = 0;
  dom.els["#cam-shot"] = { disabled: true, click() { clicks++; } };
  const m = mockServer({ port, dom }); const cdp = newCdp();
  await cdp.connect(m.url);
  setTimeout(() => { dom.els["#cam-shot"].disabled = false; }, 600);         // frames arrive 600 ms later
  const t0 = Date.now();
  await cdp.click("#cam-shot", 5000, 100);
  assert.equal(clicks, 1); assert.ok(Date.now() - t0 >= 550, "must not click before the button is enabled");
  cdp.close(); await stop(m);
});

test("click on an element that never appears fails with a clear label", async () => {
  const port = freePort(); const m = mockServer({ port, dom: fakeDom() }); const cdp = newCdp();
  await cdp.connect(m.url);
  await assert.rejects(cdp.click("#missing", 400, 100), /timeout waiting for clickable #missing/);
  cdp.close(); await stop(m);
});

test("waitFor fails FAST with the page's own error text instead of waiting for the full timeout", async () => {
  const port = freePort(); const dom = fakeDom();
  dom.els["#cam-err"] = { innerText: "" }; dom.els["#video"] = { videoWidth: 0 };
  const m = mockServer({ port, dom }); const cdp = newCdp();
  await cdp.connect(m.url);
  setTimeout(() => { dom.els["#cam-err"].innerText = "No camera was found on this computer."; }, 300);
  const t0 = Date.now();
  await assert.rejects(
    cdp.waitFor(`document.getElementById('video').videoWidth>0`, 25000, "camera frames", `document.getElementById('cam-err').innerText.trim()`, 100),
    /camera frames failed: No camera was found/,
  );
  assert.ok(Date.now() - t0 < 3000, "must not run into the 25 s timeout");
  cdp.close(); await stop(m);
});

test("waitFor succeeds as soon as the condition turns true", async () => {
  const port = freePort(); const dom = fakeDom(); dom.els["#video"] = { videoWidth: 0 };
  const m = mockServer({ port, dom }); const cdp = newCdp();
  await cdp.connect(m.url);
  setTimeout(() => { dom.els["#video"].videoWidth = 640; }, 250);
  assert.equal(await cdp.waitFor(`document.getElementById('video').videoWidth>0`, 5000, "frames", undefined, 100), true);
  cdp.close(); await stop(m);
});

test("setFiles drives the DevTools file-input protocol and fires 'change'", async () => {
  const port = freePort(); const dom = fakeDom(); const seen = [];
  dom.els["#file"] = { dispatchEvent(e) { seen.push(e.type); } };
  const m = mockServer({ port, dom }); const cdp = newCdp();
  await cdp.connect(m.url);
  await cdp.setFiles(["/tmp/a.docx"]);
  assert.deepEqual(seen, ["change"]);
  cdp.close(); await stop(m);
});

// ---------------------------------------------------------------------------------------------------------------------
// probeEndpoint / waitForEndpoint: every way a debug port can be "not there" is told apart
// ---------------------------------------------------------------------------------------------------------------------
function httpServer(handler) {
  return new Promise((resolve) => { const srv = http.createServer(handler); srv.listen(0, "127.0.0.1", () => { servers.push({ wss: { clients: [], close: () => srv.close() } }); resolve({ srv, port: srv.address().port }); }); });
}
const closed = (srv) => new Promise((r) => { srv.closeAllConnections?.(); srv.close(r); });

test("probe: ok when a DevTools page target answers", async () => {
  const { srv, port } = await httpServer((req, res) => { res.setHeader("content-type", "application/json"); res.end(JSON.stringify([{ type: "page", webSocketDebuggerUrl: "ws://x/y" }])); });
  const p = await probeEndpoint(port); assert.equal(p.state, "ok"); assert.equal(p.wsUrl, "ws://x/y"); await closed(srv);
});

test("probe: refused when nothing listens (the debug flag never took effect)", async () => {
  const free = await new Promise((r) => { const s = net.createServer(); s.listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => r(p)); }); });
  const p = await probeEndpoint(free); assert.equal(p.state, "refused"); assert.match(p.detail, /nothing is listening/);
});

test("probe: timeout when the port accepts but never answers", async () => {
  const { srv, port } = await httpServer(() => { /* never answer */ });
  const p = await probeEndpoint(port, { timeoutMs: 200 }); assert.equal(p.state, "timeout"); assert.match(p.detail, /within 200 ms/); await closed(srv);
});

test("probe: http-error, not-json and non-list JSON are told apart (another program owns the port)", async () => {
  const cases = [[(req, res) => { res.statusCode = 503; res.end("busy"); }, "http-error", /HTTP 503/], [(req, res) => { res.end("<html>router admin page</html>"); }, "not-json", /another program/],
                 [(req, res) => { res.setHeader("content-type", "application/json"); res.end('{"hello":"world"}'); }, "not-json", /not a DevTools target list/]];
  for (const [handler, state, rx] of cases) { const { srv, port } = await httpServer(handler); const p = await probeEndpoint(port); assert.equal(p.state, state); assert.match(p.detail, rx); await closed(srv); }
});

test("probe: no-page-target when DevTools listens but the window does not exist", async () => {
  const { srv, port } = await httpServer((req, res) => { res.setHeader("content-type", "application/json"); res.end(JSON.stringify([{ type: "service_worker" }, { type: "browser" }])); });
  const p = await probeEndpoint(port); assert.equal(p.state, "no-page-target"); assert.deepEqual(p.targets, ["service_worker", "browser"]); assert.match(p.detail, /service_worker, browser/); await closed(srv);
});

test("waitForEndpoint tallies what it saw: all 'refused' means the port never opened", async () => {
  const free = await new Promise((r) => { const s = net.createServer(); s.listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => r(p)); }); });
  const r = await waitForEndpoint(() => free, 3, { pollMs: 50 });
  assert.equal(r.state, "refused"); assert.deepEqual(r.tally, { refused: 3 });
});

test("waitForEndpoint succeeds when the endpoint appears late, and reports the earlier refusals", async () => {
  const free = await new Promise((r) => { const s = net.createServer(); s.listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => r(p)); }); });
  let late;
  setTimeout(() => { late = http.createServer((q, res) => { res.setHeader("content-type", "application/json"); res.end(JSON.stringify([{ type: "page", webSocketDebuggerUrl: "ws://late" }])); }); late.listen(free, "127.0.0.1"); }, 300);
  const r = await waitForEndpoint(() => free, 20, { pollMs: 100 });
  assert.equal(r.state, "ok"); assert.ok(r.tally.refused >= 2 && r.tally.ok === 1, JSON.stringify(r.tally)); await closed(late);
});

test("waitForEndpoint stops at once when the app dies, and says so", async () => {
  let alive = true; setTimeout(() => { alive = false; }, 250);
  const t0 = Date.now();
  const r = await waitForEndpoint(() => 9, 60, { isAlive: () => alive, pollMs: 100 });
  assert.equal(r.appDied, true); assert.ok(Date.now() - t0 < 5000);
});

test("waitForEndpoint follows a port that changes while waiting", async () => {
  const { srv, port } = await httpServer((q, res) => { res.setHeader("content-type", "application/json"); res.end(JSON.stringify([{ type: "page", webSocketDebuggerUrl: "ws://moved" }])); });
  let current = 1; setTimeout(() => { current = port; }, 250);
  const r = await waitForEndpoint(() => current, 20, { pollMs: 100 }); assert.equal(r.state, "ok"); assert.equal(r.wsUrl, "ws://moved"); await closed(srv);
});

test("concurrent commands are matched to their own replies; events and garbage frames are ignored", async () => {
  const port = freePort(); const dom = fakeDom(); dom.els["#a"] = { disabled: false };
  const wss = new WebSocketServer({ port }); servers.push({ wss });
  wss.on("connection", (sock) => sock.on("message", (m) => {
    const { id, method, params } = JSON.parse(m);
    sock.send("this is not json"); sock.send(JSON.stringify({ method: "Page.frameNavigated", params: {} }));          // noise before the answer
    const delay = method === "Runtime.evaluate" ? Number(String(params.expression).match(/\d+/)?.[0] ?? 0) : 0;
    setTimeout(() => sock.send(JSON.stringify({ id, result: { result: { value: params.expression } } })), delay);
  }));
  const cdp = newCdp(); await cdp.connect(`ws://127.0.0.1:${port}`);
  const answers = await Promise.all([cdp.ev("'slow 120'"), cdp.ev("'fast 1'"), cdp.ev("'mid 40'")]);
  assert.deepEqual(answers, ["'slow 120'", "'fast 1'", "'mid 40'"]);
  cdp.close();
});
