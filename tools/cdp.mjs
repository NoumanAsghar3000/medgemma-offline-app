// Minimal, hardened Chrome-DevTools-Protocol client used by the real-WebView2 end-to-end test (tools/e2e_webview.mjs).
// Everything that can hang has a timeout, everything that can be refused is retried, and a closed socket rejects all pending commands.
import WebSocket from "ws";

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

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
