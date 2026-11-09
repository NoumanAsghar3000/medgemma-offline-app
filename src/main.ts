import "./style.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { processFile, type Att } from "./attachments";
import { buildMessages, streamChat, type Turn } from "./chat";
import { md } from "./md";

interface Status { state: "need_model_folder" | "loading" | "ready" | "error"; message: string; model_dir: string; compute: string;
  model: string; quant: string; family: string; multimodal: boolean; ctx: number; port: number; token: string; note: string }

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]!));

let st: Status | null = null;
let atts: Att[] = []; let pending = 0;
const history: Turn[] = [];
let abort: AbortController | null = null;
let busy = false;

/* ---------------- backend status + model-folder gate ---------------- */
function render(s: Status) {
  st = s;
  const badge = $("badge"); const gate = $("gate"); const body = $("gate-body");
  badge.className = "badge";
  if (s.state === "ready") {
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
async function openCamera() {
  $("cam").hidden = false; $("cam-err").textContent = "";
  try {
    stream = await navigator.mediaDevices.getUserMedia({ video: { width: { ideal: 1600 } }, audio: false });
    $<HTMLVideoElement>("video").srcObject = stream;
  } catch (e) {
    $("cam-err").textContent = "Camera unavailable: " + ((e as Error).name === "NotAllowedError"
      ? "permission denied. Enable it in Windows Settings → Privacy & security → Camera (allow desktop apps)."
      : "no camera found or it is in use by another app.");
  }
}
function closeCamera() { stream?.getTracks().forEach((t) => t.stop()); stream = null; $("cam").hidden = true; }
$("m-cam").onclick = openCamera;
$("cam-cancel").onclick = closeCamera;
$("cam-shot").onclick = () => {
  const v = $<HTMLVideoElement>("video"); if (!v.videoWidth) return;
  const c = document.createElement("canvas"); c.width = v.videoWidth; c.height = v.videoHeight;
  c.getContext("2d")!.drawImage(v, 0, 0);
  c.toBlob((b) => { if (b) addFiles([new File([b], `camera-${new Date().toISOString().replace(/[:.]/g, "-")}.jpg`, { type: "image/jpeg" })]); closeCamera(); }, "image/jpeg", 0.92);
};

/* ---------------- chat ---------------- */
const input = $<HTMLTextAreaElement>("input"); const sendBtn = $<HTMLButtonElement>("send");
function updateSend() { sendBtn.disabled = !busy && (st?.state !== "ready" || pending > 0 || (!input.value.trim() && !atts.some((a) => a.images.length || a.text))); sendBtn.textContent = busy ? "■" : "➤"; }
input.addEventListener("input", () => { input.style.height = "auto"; input.style.height = Math.min(input.scrollHeight, 180) + "px"; updateSend(); });
input.addEventListener("keydown", (e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); send(); } });
sendBtn.onclick = () => (busy ? abort?.abort() : send());

function addMsg(role: "user" | "assistant", html: string): HTMLElement {
  $("empty")?.remove();
  const m = document.createElement("div"); m.className = `msg ${role}`; m.innerHTML = `<div class="bubble">${html}</div>`;
  $("messages").appendChild(m); $("messages").scrollTop = 1e9; return m.firstElementChild as HTMLElement;
}
async function send() {
  if (!st || st.state !== "ready" || busy) return;
  const text = input.value.trim(); const sent = atts; if (!text && !sent.length) return;
  if (!st.multimodal && sent.some((a) => a.images.length)) { addMsg("assistant", `<span class="err">The loaded model is text-only, so images / scanned pages cannot be analysed. Add the 4B mmproj file to the model folder.</span>`); }
  const thumbs = sent.flatMap((a) => a.images).map((u) => `<img src="${u}" alt="">`).join("");
  const names = sent.filter((a) => !a.images.length).map((a) => `<span class="tag">📄 ${esc(a.name)}</span>`).join(" ");
  addMsg("user", `${thumbs ? `<div class="thumbs">${thumbs}</div>` : ""}${names}${names ? "<br>" : ""}${esc(text)}`);
  input.value = ""; input.style.height = "auto"; atts = []; renderChips();

  const out = addMsg("assistant", `<span class="muted">Thinking…</span>`);
  const msgs = buildMessages(history, text, sent, st.ctx);
  let acc = ""; busy = true; abort = new AbortController(); updateSend();
  try {
    await streamChat(`http://127.0.0.1:${st.port}`, st.token, msgs, (d) => { acc += d; out.innerHTML = md(acc); $("messages").scrollTop = 1e9; }, abort.signal);
  } catch (e) {
    if ((e as Error).name !== "AbortError") out.innerHTML += `<p class="err">${esc((e as Error).message)}</p>`;
  } finally {
    busy = false; abort = null; updateSend();
    const doc = sent.filter((a) => a.text).map((a) => `[file: ${a.name}]`).join(" ");
    history.push({ role: "user", text: `${text} ${doc}`.trim() }, { role: "assistant", text: acc });
  }
}

/* ---------------- boot ---------------- */
await listen<Status>("status", (e) => render(e.payload));
render(await invoke<Status>("get_status"));   // state may already have been published before the UI loaded
renderChips();
