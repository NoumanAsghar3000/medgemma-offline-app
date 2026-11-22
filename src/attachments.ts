// Turns any attachment into model-ready pieces: images (as JPEG data URLs) and/or extracted text.
// 100% in-browser (pdf.js, mammoth, UTIF are bundled) - no network, no native helpers.
import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";
import workerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";
import mammoth from "mammoth/mammoth.browser";
import UTIF from "utif";
import { summarizeCsv } from "./csv";
import { declaredZipBytes } from "./zipguard";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

export interface Att { id: string; name: string; label: string; thumb: string; images: string[]; text: string; warn: string }

const MAX_IMG = 1024;          // longest side sent to the vision encoder (Gemma 3 uses 896 px)
const MAX_PDF_IMG_PAGES = 6;   // scanned pages rendered as images (each costs ~256 tokens)
const MAX_PDF_PAGES = 60;
const MAX_FILE_BYTES = 120 * 1024 * 1024;
// Attachments are untrusted input. These caps keep a hostile file from freezing or crashing the window (memory / CPU exhaustion).
const MAX_PIXELS = 100_000_000;                 // ~10 000 x 10 000: a decoded RGBA canvas of this size is already 400 MB
const MAX_DOCX_INFLATED = 200 * 1024 * 1024;     // total uncompressed size a .docx may declare
const MAX_TEXT_BYTES = 4 * 1024 * 1024;           // plain text beyond this is cut (the prompt only has room for a few tens of KB anyway)

function canvasToJpeg(c: HTMLCanvasElement): string { return c.toDataURL("image/jpeg", 0.88); }
function fit(w: number, h: number): [number, number] { const k = Math.min(1, MAX_IMG / Math.max(w, h)); return [Math.round(w * k), Math.round(h * k)]; }

async function imageToJpeg(file: File): Promise<string> {
  const ext = file.name.toLowerCase().split(".").pop() ?? "";
  if (ext === "tif" || ext === "tiff" || file.type === "image/tiff") {
    const buf = await file.arrayBuffer();
    const ifds = UTIF.decode(buf);
    if (!ifds.length) throw new Error("not a readable TIFF image");
    const tw = Number(ifds[0].t256?.[0] ?? 0), th = Number(ifds[0].t257?.[0] ?? 0);      // width / height tags, read before anything is allocated
    if (!(tw > 0 && th > 0) || tw * th > MAX_PIXELS) throw new Error("image dimensions are too large");
    UTIF.decodeImage(buf, ifds[0]);
    const rgba = UTIF.toRGBA8(ifds[0]);
    const src = document.createElement("canvas"); src.width = ifds[0].width; src.height = ifds[0].height;
    src.getContext("2d")!.putImageData(new ImageData(new Uint8ClampedArray(rgba), src.width, src.height), 0, 0);
    const [w, h] = fit(src.width, src.height);
    const out = document.createElement("canvas"); out.width = w; out.height = h;
    const g = out.getContext("2d")!; g.fillStyle = "#fff"; g.fillRect(0, 0, w, h); g.drawImage(src, 0, 0, w, h);
    const j = canvasToJpeg(out); src.width = src.height = out.width = out.height = 0; return j;
  }
  const bmp = await createImageBitmap(file);
  if (bmp.width * bmp.height > MAX_PIXELS) { bmp.close(); throw new Error("image dimensions are too large"); }
  const [w, h] = fit(bmp.width, bmp.height);
  const c = document.createElement("canvas"); c.width = w; c.height = h;
  const g = c.getContext("2d")!; g.fillStyle = "#fff"; g.fillRect(0, 0, w, h); g.drawImage(bmp, 0, 0, w, h);
  bmp.close();
  const j = canvasToJpeg(c); c.width = c.height = 0;   // release the backing bitmap right away
  return j;
}

async function readPdf(file: File): Promise<{ text: string; images: string[]; warn: string }> {
  const doc = await pdfjs.getDocument({ data: new Uint8Array(await file.arrayBuffer()), isEvalSupported: false }).promise;   // no eval()/Function for fonts: a hostile PDF cannot reach it
  const total = doc.numPages;
  try {
    const n = Math.min(total, MAX_PDF_PAGES);
    let text = ""; const images: string[] = []; let skipped = 0;
    for (let p = 1; p <= n; p++) {
      const page = await doc.getPage(p);
      const tc = await page.getTextContent();
      const t = tc.items.map((i) => ("str" in i ? i.str : "")).join(" ").replace(/\s+/g, " ").trim();
      if (t.length >= 60) text += `\n[Page ${p}]\n${t}\n`;
      else if (images.length < MAX_PDF_IMG_PAGES) {           // scanned / CamScanner / image-only page (its few words of text, if any, are kept too)
        if (t) text += `\n[Page ${p}]\n${t}\n`;
        const v0 = page.getViewport({ scale: 1 });
        const [w] = fit(v0.width, v0.height);
        const vp = page.getViewport({ scale: w / v0.width });
        const c = document.createElement("canvas"); c.width = Math.round(vp.width); c.height = Math.round(vp.height);
        const ctx = c.getContext("2d")!; ctx.fillStyle = "#fff"; ctx.fillRect(0, 0, c.width, c.height);
        await page.render({ canvasContext: ctx, viewport: vp }).promise;
        images.push(canvasToJpeg(c)); c.width = c.height = 0;
      } else if (t) text += `\n[Page ${p}]\n${t}\n`;         // image budget used up: a short text-only page must not be lost
      else skipped++;
      page.cleanup();
    }
    const warn = [total > n ? `only the first ${n} of ${total} pages were read` : "", skipped ? `${skipped} scanned page(s) skipped (limit ${MAX_PDF_IMG_PAGES} page images)` : ""].filter(Boolean).join("; ");
    return { text, images, warn };
  } finally {
    await doc.destroy();                                     // frees pdf.js worker memory, also when a page failed to render
  }
}

const TEXT_EXT = new Set(["txt", "csv", "tsv", "md", "json", "log"]);

export async function processFile(file: File): Promise<Att> {
  const ext = (file.name.toLowerCase().split(".").pop() ?? "");
  const a: Att = { id: crypto.randomUUID(), name: file.name, label: ext.toUpperCase(), thumb: "", images: [], text: "", warn: "" };
  try {
    if (file.size > MAX_FILE_BYTES) { a.warn = "file too large (limit 120 MB)"; return a; }
    if (file.type.startsWith("image/") || ["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"].includes(ext)) {
      const j = await imageToJpeg(file); a.images = [j]; a.thumb = j; a.label = "Image";
    } else if (ext === "pdf" || file.type === "application/pdf") {
      const r = await readPdf(file); a.text = r.text; a.images = r.images; a.warn = r.warn; a.thumb = r.images[0] ?? ""; a.label = "PDF";
      if (!a.text && !a.images.length) a.warn = "no readable content found";
    } else if (ext === "docx") {
      const buf = await file.arrayBuffer();
      if (declaredZipBytes(buf) > MAX_DOCX_INFLATED) { a.warn = "this .docx is not safe to open (it would expand to an unreasonable size)"; a.label = "DOCX"; return a; }
      a.text = (await mammoth.extractRawText({ arrayBuffer: buf })).value; a.label = "DOCX";
    } else if (TEXT_EXT.has(ext) || file.type.startsWith("text/")) {
      a.label = ext.toUpperCase() || "TXT";
      const isCsv = ext === "csv" || ext === "tsv";
      const cut = !isCsv && file.size > MAX_TEXT_BYTES;
      a.text = await (cut ? file.slice(0, MAX_TEXT_BYTES) : file).text();
      if (cut) a.warn = `only the first ${MAX_TEXT_BYTES / 1048576} MB of this text file were read`;
      if (isCsv) a.text = summarizeCsv(a.text);
    } else if (ext === "doc") {
      a.warn = "old .doc files are not supported - save as .docx or PDF";
    } else {
      a.warn = "unsupported file type";
    }
  } catch (e) { a.warn = `could not read file (${(e as Error).message})`; }
  // An empty .txt/.csv/.docx used to end up with neither text nor warning: sending was then refused with "see the red notes" and there was none.
  if (!a.text.trim()) a.text = "";
  if (!a.warn && !a.text && !a.images.length) a.warn = "no readable content found";
  return a;
}
