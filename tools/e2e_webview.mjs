// End-to-end test of the INSTALLED app inside the REAL WebView2, driven over the Chrome DevTools Protocol.
// Runs on the Windows CI runner (see .github/workflows/build.yml). Required env:
//   APP_EXE        path of the installed medgemma-offline.exe (its install.json points at a MISSING model folder)
//   GOOD_MODELS    folder with a valid model (tiny Gemma-3 test model) used for the re-link step
//   WORK_DIR       scratch folder for generated attachments and the saved PDF
// The app is started with MG_TEST_PICK_FOLDER / MG_TEST_SAVE_PATH so the two NATIVE dialogs (folder picker, Save)
// - which a headless test cannot click - are replaced by those paths. Everything else is the real UI code path.
import { spawn, execSync } from "node:child_process";
import { mkdirSync, writeFileSync, readFileSync, existsSync } from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import WebSocket from "ws";
import { PDFDocument, StandardFonts } from "pdf-lib";
import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";

async function pdfText(file) {   // real text extraction (pdf-lib compresses content streams, so grepping bytes proves nothing)
  const doc = await pdfjs.getDocument({ data: new Uint8Array(readFileSync(file)), verbosity: 0 }).promise;
  let t = ""; for (let p = 1; p <= doc.numPages; p++) { const c = await (await doc.getPage(p)).getTextContent(); t += c.items.map((i) => i.str).join(" ") + "\n"; }
  return { pages: doc.numPages, text: t };
}

const { APP_EXE, GOOD_MODELS, WORK_DIR } = process.env;
if (!APP_EXE || !GOOD_MODELS || !WORK_DIR) { console.error("APP_EXE, GOOD_MODELS, WORK_DIR required"); process.exit(2); }
mkdirSync(WORK_DIR, { recursive: true });
const PORT = 9333, PDF_OUT = path.join(WORK_DIR, "saved-report.pdf");
const results = [];
const check = (name, ok, detail = "", required = true) => { results.push({ name, ok: !!ok, detail: String(detail).slice(0, 300), required }); console.log(`${ok ? "PASS" : (required ? "FAIL" : "SOFT-FAIL")}  ${name}${detail ? "  - " + String(detail).slice(0, 200) : ""}`); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---- start the app with a debuggable WebView2 and a FAKE camera
const env = { ...process.env, MG_TEST_PICK_FOLDER: GOOD_MODELS, MG_TEST_SAVE_PATH: PDF_OUT,
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT} --use-fake-device-for-media-stream --use-fake-ui-for-media-stream` };
const app = spawn(APP_EXE, [], { env, stdio: "ignore", detached: false });
let appExited = false; app.on("exit", () => (appExited = true));

async function targetWs() {
  for (let i = 0; i < 120 && !appExited; i++) {
    try { const l = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json(); const t = l.find((x) => x.type === "page" && x.webSocketDebuggerUrl); if (t) return t.webSocketDebuggerUrl; } catch { /* not up yet */ }
    await sleep(1000);
  }
  throw new Error("WebView2 debugging endpoint never came up (app exited: " + appExited + ")");
}
let ws, nextId = 1; const pending = new Map();
function send(method, params = {}) { return new Promise((res, rej) => { const id = nextId++; pending.set(id, { res, rej }); ws.send(JSON.stringify({ id, method, params })); }); }
async function ev(expr) {
  const r = await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
  return r.result.value;
}
async function waitFor(expr, ms, label) {
  const t0 = Date.now(); let last;
  while (Date.now() - t0 < ms) { try { last = await ev(expr); if (last) return last; } catch (e) { last = String(e); } await sleep(700); }
  throw new Error(`timeout waiting for ${label || expr} (last: ${String(last).slice(0, 120)})`);
}
const click = (sel) => ev(`document.querySelector(${JSON.stringify(sel)}).click(), true`);
const text = (sel) => ev(`(document.querySelector(${JSON.stringify(sel)})||{}).innerText||''`);

function png(w, h) {   // tiny valid RGB PNG (no deps)
  const raw = Buffer.alloc((w * 3 + 1) * h); for (let y = 0; y < h; y++) { raw[y * (w * 3 + 1)] = 0; for (let x = 0; x < w; x++) { const o = y * (w * 3 + 1) + 1 + x * 3; raw[o] = (x * 4) & 255; raw[o + 1] = (y * 4) & 255; raw[o + 2] = 128; } }
  const crcT = Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
  const crc = (b) => { let c = 0xffffffff; for (const x of b) c = crcT[(c ^ x) & 255] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
  const chunk = (t, d) => { const l = Buffer.alloc(4); l.writeUInt32BE(d.length); const td = Buffer.concat([Buffer.from(t), d]); const c = Buffer.alloc(4); c.writeUInt32BE(crc(td)); return Buffer.concat([l, td, c]); };
  const ihdr = Buffer.alloc(13); ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr[8] = 8; ihdr[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", ihdr), chunk("IDAT", zlib.deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

try {
  const url = await targetWs();
  ws = new WebSocket(url); await new Promise((r, j) => { ws.on("open", r); ws.on("error", j); });
  ws.on("message", (m) => { const d = JSON.parse(m); if (d.id && pending.has(d.id)) { const p = pending.get(d.id); pending.delete(d.id); d.error ? p.rej(new Error(d.error.message)) : p.res(d.result); } });
  await send("Runtime.enable"); await send("DOM.enable"); await send("Page.enable");
  check("real WebView2 UI loaded and scripted over DevTools", true, url);

  // 1) model folder is MISSING -> the dedicated re-link dialog must appear
  await waitFor(`(document.getElementById('gate-body')||{}).innerText?.includes('Backend model files not found')`, 90000, "re-link dialog");
  const gateText = await text("#gate-body");
  check("missing folder -> 'Backend model files not found' dialog shown", gateText.includes("Backend model files not found"), gateText.replace(/\s+/g, " "));
  check("dialog offers the folder browser button", await ev(`!!document.getElementById('g-pick') && !document.getElementById('gate').hidden`));

  // 2) re-link (native picker replaced by MG_TEST_PICK_FOLDER) -> model loads, chat unlocks
  await click("#g-pick");
  await waitFor(`document.getElementById('gate').hidden && /MedGemma/.test(document.getElementById('badge').innerText)`, 180000, "model loaded after re-link");
  check("re-link: path persisted, model planned and loaded, chat enabled", true, await text("#badge"));
  check("%APPDATA% config.json persisted the re-linked folder", (() => { try { const j = JSON.parse(readFileSync(path.join(process.env.APPDATA, "com.nouman.medgemma.offline", "config.json"), "utf8").replace(/^\uFEFF/, "")); return path.resolve(j.model_dir).toLowerCase() === path.resolve(GOOD_MODELS).toLowerCase(); } catch { return false; } })());

  // 3) attachments through the real file input (DOM.setFileInputFiles), incl. a generated PDF, CSV, TXT, PNG
  const csv = path.join(WORK_DIR, "labs.csv"); writeFileSync(csv, "age,glucose\n" + Array.from({ length: 800 }, (_, i) => `${20 + (i % 60)},${80 + (i % 90)}`).join("\n"));
  const txt = path.join(WORK_DIR, "history.txt"); writeFileSync(txt, "54-year-old woman, cough for two weeks, mild fever. No known allergies.");
  const pdfPath = path.join(WORK_DIR, "discharge.pdf"); { const d = await PDFDocument.create(); const f = await d.embedFont(StandardFonts.Helvetica); const pg = d.addPage(); pg.drawText("Discharge summary: patient treated for community acquired pneumonia with amoxicillin for seven days; follow-up chest radiograph advised in six weeks.", { x: 30, y: 700, size: 9, font: f, maxWidth: 520 }); writeFileSync(pdfPath, await d.save()); }
  const imgPath = path.join(WORK_DIR, "xray.png"); writeFileSync(imgPath, png(64, 64));
  const { root } = await send("DOM.getDocument"); const { nodeId } = await send("DOM.querySelector", { nodeId: root.nodeId, selector: "#file" });
  await send("DOM.setFileInputFiles", { files: [csv, txt, pdfPath, imgPath], nodeId });
  await ev(`document.getElementById('file').dispatchEvent(new Event('change',{bubbles:true})), true`);
  await waitFor(`document.querySelectorAll('#chips .chip').length>=4 && !document.querySelector('#chips .chip.busy')`, 60000, "4 attachment chips");
  const chips = await text("#chips");
  check("CSV, TXT, PDF (pdf.js) and PNG attach in the real WebView2", ["labs.csv", "history.txt", "discharge.pdf", "xray.png"].every((n) => chips.includes(n)), chips.replace(/\s+/g, " "));
  check("PDF was read (text extracted) and image decoded to a thumbnail", await ev(`!!document.querySelector('#chips img') && !/could not|unsupported/i.test(document.getElementById('chips').innerText)`));
  // 4) camera (fake device) - REQUIRED: getUserMedia -> live preview -> capture -> attached as an image chip
  await click("#plus"); await click("#m-cam");
  await waitFor(`!document.getElementById('cam').hidden`, 5000, "camera dialog");
  await waitFor(`document.getElementById('video').videoWidth>0`, 25000, "camera frames");
  await click("#cam-shot");
  await waitFor(`document.getElementById('chips').innerText.includes('camera-')`, 20000, "camera chip");
  check("camera: getUserMedia in WebView2 -> capture -> attached as image", true);

  // 5a) ONE request carrying text + CSV + TXT + PDF + PNG + camera photo. The tiny CI model has no vision projector, so the REAL
  //     llama-server must answer "image input is not supported ... mmproj": that proves the image parts arrived in the request.
  await ev(`(()=>{const i=document.getElementById('input'); i.value='Please assess this case.'; i.dispatchEvent(new Event('input',{bubbles:true})); return true})()`);
  await click("#send");
  await waitFor(`/image input is not supported/i.test(document.querySelector('.msg.assistant:last-child .bubble')?.innerText||'')`, 120000, "server reply about image input");
  check("text + CSV + TXT + PDF + PNG + camera photo sent in ONE request; the real server received the image parts", true, await ev(`document.querySelector('.msg.assistant:last-child .bubble').innerText.replace(/\s+/g,' ')`));
  await waitFor(`document.getElementById('send').textContent.trim() !== '\u25A0'`, 30000, "send button back to idle");

  // 5b) text-only request (documents only) -> successful streamed answer
  const r2 = await send("DOM.getDocument"); const q2 = await send("DOM.querySelector", { nodeId: r2.root.nodeId, selector: "#file" });
  await send("DOM.setFileInputFiles", { files: [csv, txt, pdfPath], nodeId: q2.nodeId });
  await ev(`document.getElementById('file').dispatchEvent(new Event('change',{bubbles:true})), true`);
  await waitFor(`document.querySelectorAll('#chips .chip').length>=3 && !document.querySelector('#chips .chip.busy')`, 60000, "chips again");
  await ev(`[...document.querySelectorAll('#chips .chip')].filter(c=>/camera-|xray/.test(c.innerText)).forEach(c=>c.querySelector('button').click()), true`);
  await ev(`(()=>{const i=document.getElementById('input'); i.value='Summarise the documents.'; i.dispatchEvent(new Event('input',{bubbles:true})); return true})()`);
  await click("#send");
  await waitFor(`!!document.querySelector('.msg.assistant .actions button')`, 240000, "assistant answer");
  const ans = await ev(`document.querySelector('.msg.assistant .actions').closest('.bubble').innerText`);
  check("documents (CSV/TXT/PDF) answered by the local server (streamed)", ans.trim().length > 5, ans.replace(/\s+/g, " "));
  const actions = await ev(`document.querySelector('.msg.assistant .actions').innerText`);
  check("measured tokens/second shown after the answer", /tok\/s measured/.test(actions), actions);

  // 6) PDF report: form -> offline PDF -> save (native dialog replaced by MG_TEST_SAVE_PATH)
  await click(".msg.assistant .actions button");
  await waitFor(`!document.getElementById('case').hidden`, 5000, "case dialog");
  await ev(`(()=>{const s=(id,v)=>{const e=document.getElementById(id); e.value=v; e.dispatchEvent(new Event('input',{bubbles:true}))}; s('c-name','CI Test Patient'); s('c-id','MRN-0001'); s('c-age','54 / F'); s('c-doc','Dr CI'); return true})()`);
  await click("#case-save");
  await waitFor(`/Saved:/.test(document.getElementById('case-msg').innerText)`, 60000, "report saved");
  const pdf = existsSync(PDF_OUT) ? readFileSync(PDF_OUT) : Buffer.alloc(0);
  check("report PDF generated offline and written to disk", pdf.subarray(0, 5).toString() === "%PDF-" && pdf.length > 3000, `${pdf.length} bytes`);
  try { const d = await PDFDocument.load(pdf); check("saved PDF is a valid document", d.getPageCount() >= 1, `${d.getPageCount()} page(s), title='${d.getTitle()}'`); } catch (e) { check("saved PDF is a valid document", false, e.message); }
  try {
    const { text: pt } = await pdfText(PDF_OUT);
    for (const k of ["MEDICAL ANALYSIS REPORT", "CI Test Patient", "MRN-0001", "FINDINGS", "IMPRESSION", "RECOMMENDATIONS", "labs.csv", "history.txt", "discharge.pdf", "MedGemma", "decision support"])
      check(`PDF contains: ${k}`, pt.includes(k), "");
    check("PDF shows a timestamp", /20\d\d-\d\d-\d\d \d\d:\d\d UTC/.test(pt), "");
  } catch (e) { check("PDF text extraction", false, e.message); }

  // 7) offline: no request left the loopback (the page itself never reached for the network)
  const external = await ev(`performance.getEntriesByType('resource').map(r=>r.name).filter(n=>!/^(http:\\/\\/tauri\\.localhost|tauri:|blob:|data:|http:\\/\\/ipc\\.localhost)/.test(n))`);
  check("no resource was fetched from outside the app (WebView2 performance log)", external.length === 0, external.join(", "));
} catch (e) {
  check("e2e run", false, e.stack || e.message);
} finally {
  try { ws?.close(); } catch {}
  try { execSync("taskkill /F /T /IM llama-server.exe", { stdio: "ignore" }); } catch {}
  try { app.kill(); execSync(`taskkill /F /T /PID ${app.pid}`, { stdio: "ignore" }); } catch {}
}
writeFileSync(path.join(WORK_DIR, "e2e-results.json"), JSON.stringify(results, null, 2));
const failed = results.filter((r) => !r.ok && r.required);
console.log(`\n${results.filter((r) => r.ok).length}/${results.length} checks passed; required failures: ${failed.length}`);
process.exit(failed.length ? 1 : 0);
