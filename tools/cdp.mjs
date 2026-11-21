// Minimal, hardened Chrome-DevTools-Protocol client used by the real-WebView2 end-to-end test (tools/e2e_webview.mjs).
// Everything that can hang has a timeout, everything that can be refused is retried, and a closed socket rejects all pending commands.
import WebSocket from "ws";

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Says exactly what is (not) on the DevTools port. state:
 *   ok              a DevTools endpoint with a page target answered
 *   refused         nothing listens on the port (the debug flag never took effect, or the app is not running)
 *   timeout         the port accepts but never answers (filtered / hung process)
 *   http-error      something answers HTTP but not with 200
 *   not-json        something listens that is not a DevTools endpoint (the port belongs to another program)
 *   no-page-target  DevTools listens but exposes no page (window not created yet / crashed)
 *   error           anything else (detail has the system error)
 */
export async function probeEndpoint(port, { timeoutMs = 3000, host = "127.0.0.1" } = {}) {
  const url = `http://${host}:${port}/json`;
  let res;
  try { res = await fetch(url, { signal: AbortSignal.timeout(timeoutMs) }); }
  catch (e) {
    const code = e?.cause?.code || e?.code || "";
    if (e?.name === "TimeoutError" || e?.name === "AbortError") return { state: "timeout", detail: `no answer from ${url} within ${timeoutMs} ms (port filtered or process hung)` };
    if (code === "ECONNREFUSED") return { state: "refused", detail: `nothing is listening on ${host}:${port}` };
    return { state: "error", detail: `${code || e?.name}: ${e?.message}` };
  }
  if (!res.ok) return { state: "http-error", detail: `HTTP ${res.status} from ${url}` };
  let list;
  try { list = await res.json(); } catch { return { state: "not-json", detail: `${url} did not return JSON: another program listens on this port` }; }
  if (!Array.isArray(list)) return { state: "not-json", detail: `${url} returned JSON that is not a DevTools target list` };
  const page = list.find((t) => t && t.type === "page" && t.webSocketDebuggerUrl);
  if (!page) return { state: "no-page-target", detail: `DevTools is listening but exposes no page (targets: ${list.map((t) => t?.type).join(", ") || "none"})`, targets: list.map((t) => t?.type) };
  return { state: "ok", detail: "page target found", wsUrl: page.webSocketDebuggerUrl };
}

/**
 * Polls probeEndpoint until it is ok, `seconds` pass, or the app dies. The result carries `tally` (how often each state was seen), which is
 * what tells "never listened" (all refused) from "listened but broken" (some other state) in the final verdict.
 */
export async function waitForEndpoint(getPort, seconds, { isAlive = () => true, pollMs = 1000, onTick } = {}) {
  const tally = {}; let last = { state: "refused", detail: "not probed yet" };
  for (let i = 0; i < seconds; i++) {
    if (!isAlive()) return { ...last, tally, appDied: true };
    if (onTick) await onTick(i);
    last = await probeEndpoint(getPort());
    tally[last.state] = (tally[last.state] || 0) + 1;
    if (last.state === "ok") return { ...last, tally };
    await sleep(pollMs);
  }
  return { ...last, tally };
}

export function createCdp({ timeoutMs = 45000 } = {}) {
  let ws = null; let nextId = 1; const pending = new Map();

  function send(method, params = {}) {
    return new Promise((res, rej) => {
      if (!ws || ws.readyState !== WebSocket.OPEN) return rej(new Error(`DevTools not connected (${method})`));
      const id = nextId++;
      const timer = setTimeout(() => { pending.delete(id); rej(new Error(`DevTools command timed out: ${method}`)); }, timeoutMs);
      pending.set(id, { res: (v) => { clearTimeout(timer); res(v); }, rej: (e) => { clearTimeout(timer); rej(e); } });
      ws.send(JSON.stringify({ id, method, params }));
    });
  }

  /** The socket can refuse the first attempts while WebView2 is still starting. */
  async function connect(url, { attempts = 5, delayMs = 1500 } = {}) {
    let last;
    for (let i = 0; i < attempts; i++) {
      try {
        const w = new WebSocket(url);
        await new Promise((r, j) => { w.once("open", r); w.once("error", j); });
        w.on("error", () => {});
        w.on("close", () => { for (const [id, p] of pending) { pending.delete(id); p.rej(new Error("DevTools connection closed")); } });
        w.on("message", (m) => {
          let d; try { d = JSON.parse(m); } catch { return; }
          if (d.id && pending.has(d.id)) { const p = pending.get(d.id); pending.delete(d.id); d.error ? p.rej(new Error(d.error.message)) : p.res(d.result); }
        });
        ws = w;
        return;
      } catch (e) { last = e; await sleep(delayMs); }
    }
    throw last;
  }

  async function ev(expr) {
    const r = await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  }

  /** abortExpr: when it evaluates truthy the wait fails IMMEDIATELY with that text (e.g. the page's own camera error message). */
  async function waitFor(expr, ms, label, abortExpr, pollMs = 700) {
    const t0 = Date.now(); let last;
    while (Date.now() - t0 < ms) {
      try { last = await ev(expr); if (last) return last; } catch (e) { last = String(e); }
      if (abortExpr) { let a = ""; try { a = await ev(abortExpr); } catch { /* page busy: keep waiting */ } if (a) throw new Error(`${label} failed: ${String(a).slice(0, 200)}`); }
      await sleep(pollMs);
    }
    throw new Error(`timeout waiting for ${label || expr} (last: ${String(last).slice(0, 120)})`);
  }

  /** Waits until the element exists AND is enabled, so a click can never be swallowed by a disabled button. */
  async function click(sel, ms = 20000, pollMs = 700) {
    const q = JSON.stringify(sel);
    await waitFor(`(()=>{const e=document.querySelector(${q}); return !!e && !e.disabled})()`, ms, `clickable ${sel}`, undefined, pollMs);
    return ev(`document.querySelector(${q}).click(), true`);
  }

  const text = (sel) => ev(`(document.querySelector(${JSON.stringify(sel)})||{}).innerText||''`);

  async function setFiles(files) {
    await waitFor(`!!document.getElementById('file')`, 15000, "file input");
    const { root } = await send("DOM.getDocument");
    const { nodeId } = await send("DOM.querySelector", { nodeId: root.nodeId, selector: "#file" });
    await send("DOM.setFileInputFiles", { files, nodeId });
    await ev(`document.getElementById('file').dispatchEvent(new Event('change',{bubbles:true})), true`);
  }

  function close() { try { ws?.close(); } catch { /* already closed */ } }

  return { send, connect, ev, waitFor, click, text, setFiles, close };
}
