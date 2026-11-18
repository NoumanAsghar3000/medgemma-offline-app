import "./style.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { processFile, type Att } from "./attachments";
import { cameraErrorMessage, openCameraStream, stopStream, waitForFrame } from "./camera";
import { buildMessages, streamChat, type Turn } from "./chat";
import { md } from "./md";
import { buildReportPdf, isStructured, parseSections, type ReportFile } from "./report";

interface Status { state: "need_model_folder" | "loading" | "ready" | "idle" | "error"; message: string; model_dir: string; compute: string;
  model: string; quant: string; family: string; multimodal: boolean; ctx: number; note: string; idle_minutes: number;
  tps_best: number; tps_worst: number; verdict: string; streamed_gb: number; forced: boolean; model_file: string }
interface Variant { file: string; family: string; quant: string; size_gb: number; multimodal: boolean; tps_best: number; tps_worst: number; verdict: string; streamed_gb: number; auto: boolean; current: boolean }

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]!));

let st: Status | null = null;
let atts: Att[] = []; let pending = 0;
const history: Turn[] = [];
let abort: AbortController | null = null;
let busy = false;

/* ---------------- backend status + model-folder gate ---------------- */
const waiters: ((ok: boolean) => void)[] = [];
function render(s: Status) {
  st = s;
  const badge = $("badge"); const gate = $("gate"); const body = $("gate-body");
  badge.className = "badge";
  $<HTMLSelectElement>("mem").value = String(s.idle_minutes);
  showWarn(s);
  if ((s.state === "ready" || s.state === "idle") && s.model_file !== lastVariantFile) { lastVariantFile = s.model_file; void refreshVariants(); }
  if (s.state === "ready" || s.state === "error" || s.state === "need_model_folder") waiters.splice(0).forEach((w) => w(s.state === "ready"));
  if (s.state === "idle") {
    badge.textContent = "model unloaded · RAM freed"; gate.hidden = true;
  } else if (s.state === "ready") {
    badge.classList.add("ok");
    badge.textContent = `${s.family} ${s.quant} · ${s.compute.toUpperCase()}${s.multimodal ? " · vision" : " · text only"}`;
    gate.hidden = true;
  } else if (s.state === "loading") {
    badge.textContent = "loading model…";
    gate.hidden = false;
    body.innerHTML = `<div class="spin"></div><h2>Loading ${esc(s.family)}…</h2><p class="muted">${esc(s.message)}</p>` +
      (s.note ? `<p class="muted">${esc(s.note)}</p>` : "") + `<p class="muted" id="elapsed"></p>`;
    startTimer();
  } else if (s.state === "need_model_folder") {
    badge.classList.add("bad"); badge.textContent = "model files missing";
    gate.hidden = false;
    body.innerHTML = `<h2 class="red">Backend model files not found</h2>
      <p>The folder with the MedGemma model files was moved, renamed, deleted, or the drive is not connected.</p>
      ${s.model_dir ? `<p class="muted">Last known location:</p><div class="path">${esc(s.model_dir)}</div>` : ""}
      ${s.message.replace(/^Backend model files not found\.?\s*/, "").trim() ? `<p class="red">${esc(s.message.replace(/^Backend model files not found\.?\s*/, "").trim())}</p>` : ""}
      <p>Please select the folder that contains the MedGemma <code>.gguf</code> files.</p>
      <div id="gate-err" class="red"></div>
      <div class="btns"><button class="ghost" id="g-retry">Retry</button><button class="primary" id="g-pick">Browse for model folder…</button></div>`;
    $("g-pick").onclick = pickFolder; $("g-retry").onclick = () => invoke("retry");
  } else {
    badge.classList.add("bad"); badge.textContent = "backend error";
    gate.hidden = false;
    body.innerHTML = `<h2 class="red">The model could not start</h2><pre class="log">${esc(s.message)}</pre>
      <div id="gate-err" class="red"></div>
      <div class="btns"><button class="ghost" id="g-pick">Choose another model folder…</button><button class="primary" id="g-retry">Retry</button></div>`;
    $("g-pick").onclick = pickFolder; $("g-retry").onclick = () => invoke("retry");
  }
  updateSend();
}
let lastVariantFile = "";
let variantList: Variant[] = [];
const fmtTps = (a: number, b: number) => (Math.abs(a - b) < 0.05 ? `~${a.toFixed(1)}` : `~${b.toFixed(1)}-${a.toFixed(1)}`) + " tok/s";
function showWarn(s: Status) {
  const w = $("warn");
  const slow = s.state !== "need_model_folder" && s.model_file && (s.forced || s.verdict.startsWith("batch") || s.verdict === "impractical");
  if (!slow) { w.hidden = true; return; }
  w.hidden = false; w.classList.toggle("forced", s.forced);
  w.textContent = (s.forced ? "FORCED SLOW VARIANT. " : "") + `Estimated speed on this PC: ${fmtTps(s.tps_best, s.tps_worst)} (${s.verdict}). ` +
    (s.streamed_gb > 0.5 ? `About ${s.streamed_gb.toFixed(1)} GB of weights must be re-read from the drive for every token. ` : "") +
    `A 1,000-token report could take about ${fmtDuration(1000 / Math.max(s.tps_worst, 0.005))}. Choose a smaller variant in the header, or add RAM / a faster drive / a GPU.`;
}
async function refreshVariants() {
  const sel = $<HTMLSelectElement>("variant");
  try {
    const vs = await invoke<Variant[]>("list_variants"); variantList = vs;
    sel.innerHTML = `<option value="">Automatic (recommended)</option>` + vs.map((v) =>
      `<option value="${esc(v.file)}"${v.current ? " selected" : ""}>${esc(v.family.replace("MedGemma ", ""))} ${esc(v.quant)} · ${v.size_gb.toFixed(1)} GB · ${esc(fmtTps(v.tps_best, v.tps_worst))}${v.auto ? " · auto" : ""}</option>`).join("");
  } catch { sel.innerHTML = `<option value="">Automatic</option>`; }
}
const MIN_TPS = 0.5;
function fmtDuration(sec: number) { return sec < 5400 ? `${Math.max(1, Math.round(sec / 60))} minutes` : `${(sec / 3600).toFixed(1)} hours`; }
$("variant").onchange = (e) => {
  const sel = e.target as HTMLSelectElement; const file = sel.value; const v = variantList.find((x) => x.file === file);
  const revert = () => { sel.value = lastVariantFile; };
  if (v && v.tps_worst < MIN_TPS) {                       // strong, blocking warning before loading a too-slow variant
    $("force-text").textContent = `${v.family} ${v.quant} (${v.size_gb.toFixed(1)} GB) is estimated at ${fmtTps(v.tps_best, v.tps_worst)} on this PC` +
      (v.streamed_gb > 0.5 ? `, because about ${v.streamed_gb.toFixed(1)} GB of weights must be re-read from the drive for every generated token` : "") +
      `. A 1,000-token report could take about ${fmtDuration(1000 / Math.max(v.tps_worst, 0.005))}.`;
    $("force").hidden = false;
    $("force-no").onclick = () => { $("force").hidden = true; revert(); };
    $("force-yes").onclick = () => { $("force").hidden = true; void invoke("choose_variant", { file }); };
    return;
  }
  void invoke("choose_variant", { file });
};
$("mem").onchange = (e) => { void invoke("set_memory_mode", { minutes: Number((e.target as HTMLSelectElement).value) }); };
/** Reload the model after an idle unload and wait until it is ready (or failed). */
function wake(): Promise<boolean> {
  return new Promise((res) => { waiters.push(res); void invoke("retry"); });
}
async function pickFolder() {
  try { await invoke<boolean>("pick_model_folder"); }                      // Rust validates + persists + restarts backend
  catch (e) { const el = document.getElementById("gate-err"); if (el) el.textContent = String(e); }
}
let timer = 0;
function startTimer() {
  clearInterval(timer); const t0 = Date.now();
  timer = window.setInterval(() => { const e = document.getElementById("elapsed"); if (!e) return clearInterval(timer); e.textContent = `Elapsed: ${Math.floor((Date.now() - t0) / 1000)} s`; }, 1000);
}

/* ---------------- attachments ---------------- */
function renderChips() {
  const box = $("chips"); box.innerHTML = "";
  for (const a of atts) {
    const d = document.createElement("div"); d.className = "chip";
    d.innerHTML = `${a.thumb ? `<img src="${a.thumb}" alt="">` : `<div class="ico">📄</div>`}<div><span>${esc(a.name)}</span><small class="${a.warn ? "err" : ""}">${esc(a.warn || a.label)}</small></div><button title="Remove">✕</button>`;
    d.querySelector("button")!.onclick = () => { atts = atts.filter((x) => x.id !== a.id); renderChips(); };
    box.appendChild(d);
  }
  if (pending) { const d = document.createElement("div"); d.className = "chip busy"; d.innerHTML = `<div class="ico">⏳</div><span>Reading ${pending} file(s)…</span>`; box.appendChild(d); }
  updateSend();
}
async function addFiles(files: File[]) {
  pending += files.length; renderChips();
  for (const f of files) { const a = await processFile(f); pending--; if (a.images.length || a.text) atts.push(a); else { a.thumb = ""; atts.push(a); } renderChips(); }
}

/* ---------------- + menu, file picker, camera ---------------- */
const menu = $("menu");
$("plus").onclick = (e) => { e.stopPropagation(); menu.hidden = !menu.hidden; };
document.addEventListener("click", () => (menu.hidden = true));
$("m-file").onclick = () => $<HTMLInputElement>("file").click();
$<HTMLInputElement>("file").onchange = (e) => { const i = e.target as HTMLInputElement; addFiles([...(i.files ?? [])]); i.value = ""; };
document.addEventListener("paste", (e) => { const f = [...(e.clipboardData?.files ?? [])]; if (f.length) addFiles(f); });
window.addEventListener("dragover", (e) => e.preventDefault());
window.addEventListener("drop", (e) => { e.preventDefault(); addFiles([...(e.dataTransfer?.files ?? [])]); });

let stream: MediaStream | null = null;
let camSeq = 0;                                   // bumped on every open/close so a late permission answer cannot revive a closed dialog
function closeCameraStream() { stopStream(stream); stream = null; }
async function openCamera() {
  closeCameraStream(); const seq = ++camSeq;
  const shot = $<HTMLButtonElement>("cam-shot"); shot.disabled = true;
  $("cam").hidden = false; $("cam-err").textContent = "";
  try {
    const s = await openCameraStream();          // constraint ladder: rear camera preferred, then any camera
    if (seq !== camSeq) { stopStream(s); return; }
    stream = s;
    s.getVideoTracks().forEach((t) => t.addEventListener("ended", () => { if (stream === s) { $("cam-err").textContent = "The camera was disconnected."; shot.disabled = true; } }));
    const v = $<HTMLVideoElement>("video"); v.srcObject = s;
    if (await waitForFrame(v, 15000)) { if (seq === camSeq) shot.disabled = false; }
    else if (seq === camSeq) $("cam-err").textContent = "The camera started but is not delivering images. Close other apps that use the camera and try again.";
  } catch (e) { if (seq === camSeq) $("cam-err").textContent = cameraErrorMessage(e); }
}
function closeCamera() { camSeq++; closeCameraStream(); $<HTMLVideoElement>("video").srcObject = null; $("cam").hidden = true; }
$("m-cam").onclick = openCamera;
$("cam-cancel").onclick = closeCamera;
document.addEventListener("keydown", (e) => { if (e.key === "Escape" && !$("cam").hidden) closeCamera(); });
$("cam-shot").onclick = () => {
  const v = $<HTMLVideoElement>("video"); if (!v.videoWidth) return;
  const c = document.createElement("canvas"); c.width = v.videoWidth; c.height = v.videoHeight;
  c.getContext("2d")!.drawImage(v, 0, 0);
  c.toBlob((b) => { if (b) addFiles([new File([b], `camera-${new Date().toISOString().replace(/[:.]/g, "-")}.jpg`, { type: "image/jpeg" })]); closeCamera(); }, "image/jpeg", 0.92);
};

/* ---------------- chat ---------------- */
const input = $<HTMLTextAreaElement>("input"); const sendBtn = $<HTMLButtonElement>("send");
function updateSend() { sendBtn.disabled = !busy && ((st?.state !== "ready" && st?.state !== "idle") || pending > 0 || (!input.value.trim() && !atts.some((a) => a.images.length || a.text))); sendBtn.textContent = busy ? "■" : "➤"; }
input.addEventListener("input", () => { input.style.height = "auto"; input.style.height = Math.min(input.scrollHeight, 180) + "px"; updateSend(); });
input.addEventListener("keydown", (e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); send(); } });
sendBtn.onclick = () => (busy ? abort?.abort() : send());

function addMsg(role: "user" | "assistant", html: string): HTMLElement {
  $("empty")?.remove();
  const m = document.createElement("div"); m.className = `msg ${role}`;
  m.innerHTML = `${role === "assistant" ? `<div class="avatar" aria-hidden="true">+</div>` : ""}<div class="bubble">${html}</div>`;
  $("messages").appendChild(m); $("messages").scrollTop = 1e9; return m.querySelector(".bubble") as HTMLElement;
}

/** Findings / Impression / Recommendations as clinical cards when the answer is structured, plain markdown otherwise. */
function renderAnswer(el: HTMLElement, text: string) {
  if (!isStructured(text)) { el.innerHTML = md(text); return; }
  const s = parseSections(text);
  const card = (cls: string, title: string, body: string) => (body.trim() ? `<section class="sec ${cls}"><h3>${title}</h3>${md(body)}</section>` : "");
  el.innerHTML = (s.summary.trim() ? md(s.summary) : "") + card("find", "Findings", s.findings) + card("imp", "Impression", s.impression) + card("rec", "Recommendations", s.recommendations);
}
const answers = new Map<HTMLElement, { question: string; answer: string; files: ReportFile[]; model: string }>();

async function send() {
  if (!st || busy || (st.state !== "ready" && st.state !== "idle")) return;
  const text = input.value.trim(); const sent = atts; if (!text && !sent.length) return;
  if (sent.some((a) => !a.images.length && !a.text)) { addMsg("assistant", `<span class="err">Some attachments could not be read (see the red notes on the chips). Remove them or fix the files and try again.</span>`); return; }
  const nImg = sent.reduce((n, a) => n + a.images.length, 0);
  if (nImg > 8) { addMsg("assistant", `<span class="err">Too many images (${nImg}). Please send at most 8 images / scanned pages per message.</span>`); return; }
  if (st.state === "idle") {                                   // model was unloaded to free RAM: bring it back
    const note = addMsg("assistant", `<span class="muted">Reloading the model from disk…</span>`);
    busy = true; updateSend();
    const ok = await wake(); busy = false; updateSend();
    if (!ok) { note.innerHTML = `<span class="err">The model could not be reloaded. See the message on screen.</span>`; return; }
    note.parentElement?.remove();
  }
  if (!st!.multimodal && nImg) { addMsg("assistant", `<span class="err">The loaded model is text-only, so images / scanned pages cannot be analysed. Add the 4B mmproj file to the model folder.</span>`); }
  const thumbs = sent.flatMap((a) => a.images).map((u) => `<img src="${u}" alt="">`).join("");
  const names = sent.filter((a) => !a.images.length).map((a) => `<span class="tag">📄 ${esc(a.name)}</span>`).join(" ");
  addMsg("user", `${thumbs ? `<div class="thumbs">${thumbs}</div>` : ""}${names}${names ? "<br>" : ""}${esc(text)}`);
  input.value = ""; input.style.height = "auto"; atts = []; renderChips();

  const out = addMsg("assistant", `<span class="muted">Analysing… (large images/PDFs can take a while on CPU)</span>`);
  const msgs = buildMessages(history, text, sent, st!.ctx);
  let acc = ""; let speed = ""; let nTok = 0; let tFirst = 0; busy = true; abort = new AbortController(); updateSend();
  try {
    await streamChat(msgs, (d) => { acc += d; nTok++; if (!tFirst) tFirst = performance.now(); const dt = (performance.now() - tFirst) / 1000; if (dt > 1.5) $("live").textContent = `⚡ ${(nTok / dt).toFixed(2)} tok/s (live)`; renderAnswer(out, acc); $("messages").scrollTop = 1e9; }, abort.signal,
      (t) => { speed = `⚡ ${t.tps.toFixed(2)} tok/s measured · ${t.tokens} tokens`; });
  } catch (e) {
    if ((e as Error).name !== "AbortError") out.innerHTML += `<p class="err">${esc(String(e instanceof Error ? e.message : e))}</p>`;
  } finally {
    busy = false; abort = null; updateSend(); $("live").textContent = "";
    const doc = sent.filter((a) => a.text).map((a) => `[file: ${a.name}]`).join(" ");
    history.push({ role: "user", text: `${text} ${doc}`.trim() }, { role: "assistant", text: acc });
    if (acc.trim()) {
      const files: ReportFile[] = sent.map((a) => ({ name: a.name, kind: a.label, thumb: a.images[0] }));
      const model = st ? `${st.family} ${st.quant}${st.multimodal ? "" : " (text-only)"}` : "MedGemma";
      answers.set(out, { question: text, answer: acc, files, model });
      const bar = document.createElement("div"); bar.className = "actions";
      bar.innerHTML = `<button>📄 Download report (PDF)</button><button class="copy">Copy</button>${speed ? `<span class="speed">${esc(speed)}</span>` : ""}`;
      const [bReport, bCopy] = bar.querySelectorAll("button");
      bReport.onclick = () => openCase(out);
      bCopy.onclick = async () => {
        try { await navigator.clipboard.writeText(acc); } catch { const t = document.createElement("textarea"); t.value = acc; document.body.appendChild(t); t.select(); document.execCommand("copy"); t.remove(); }
        bCopy.textContent = "Copied"; setTimeout(() => (bCopy.textContent = "Copy"), 1500);
      };
      out.appendChild(bar);
    }
  }
}

/* ---------------- report dialog ---------------- */
let reportFor: HTMLElement | null = null;
function openCase(el: HTMLElement) { reportFor = el; $("case-msg").textContent = ""; $("case").hidden = false; $<HTMLInputElement>("c-name").focus(); }
$("case-cancel").onclick = () => ($("case").hidden = true);
$("case-save").onclick = async () => {
  const rec = reportFor && answers.get(reportFor); if (!rec) return;
  const msg = $("case-msg"); msg.className = "muted"; msg.textContent = "Building PDF…";
  try {
    const b64 = await buildReportPdf({
      caseInfo: { patientName: $<HTMLInputElement>("c-name").value, patientId: $<HTMLInputElement>("c-id").value, ageSex: $<HTMLInputElement>("c-age").value,
        clinician: $<HTMLInputElement>("c-doc").value, notes: $<HTMLTextAreaElement>("c-notes").value },
      question: rec.question, answer: rec.answer, files: rec.files, model: rec.model, when: new Date(),
    });
    const stamp = new Date().toISOString().slice(0, 16).replace(/[-:T]/g, "");
    const saved = await invoke<string | null>("save_report", { filename: `MedicalReport_${stamp}.pdf`, dataB64: b64 });
    if (saved) { msg.className = "ok"; msg.textContent = `Saved: ${saved}`; setTimeout(() => ($("case").hidden = true), 1400); } else msg.textContent = "";
  } catch (e) { msg.className = "red"; msg.textContent = `Could not create the report: ${e instanceof Error ? e.message : e}`; }
};

/* ---------------- boot ---------------- */
/* ---------------- starters, new case, theme, lightbox ---------------- */
const emptyHTML = $("empty").outerHTML;
$("messages").addEventListener("click", (e) => {
  const t = e.target as HTMLElement;
  const st = t.closest<HTMLElement>(".starter");
  if (st) { input.value = st.dataset.prompt ?? ""; input.dispatchEvent(new Event("input")); input.focus(); return; }
  if (t instanceof HTMLImageElement && t.closest(".thumbs")) { ($("lightbox-img") as HTMLImageElement).src = t.src; $("lightbox").hidden = false; }
});
$("lightbox").onclick = () => ($("lightbox").hidden = true);
$("newcase").onclick = () => {
  abort?.abort(); history.length = 0; atts = []; renderChips(); answers.clear();
  $("messages").innerHTML = emptyHTML; $("live").textContent = ""; input.value = ""; input.style.height = "auto"; updateSend();
};
function applyTheme(t: string | null) { if (t) document.documentElement.dataset.theme = t; else delete document.documentElement.dataset.theme; }
try { applyTheme(localStorage.getItem("mg-theme")); } catch { /* storage unavailable: follow the system theme */ }
$("theme").onclick = () => {
  const dark = document.documentElement.dataset.theme ? document.documentElement.dataset.theme === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
  const next = dark ? "light" : "dark"; applyTheme(next);
  try { localStorage.setItem("mg-theme", next); } catch { /* ignore */ }
};

await listen<Status>("status", (e) => render(e.payload));
render(await invoke<Status>("get_status"));   // state may already have been published before the UI loaded
renderChips();
