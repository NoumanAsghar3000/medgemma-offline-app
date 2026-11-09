// Turns any attachment into model-ready pieces: images (as JPEG data URLs) and/or extracted text.
// 100% in-browser (pdf.js, mammoth, UTIF are bundled) - no network, no native helpers.
import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";
import workerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";
import mammoth from "mammoth/mammoth.browser";
import UTIF from "utif";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

export interface Att { id: string; name: string; label: string; thumb: string; images: string[]; text: string; warn: string }

const MAX_IMG = 1024;          // longest side sent to the vision encoder (Gemma 3 uses 896 px)
const MAX_PDF_IMG_PAGES = 6;   // scanned pages rendered as images (each costs ~256 tokens)
const MAX_PDF_PAGES = 60;

function canvasToJpeg(c: HTMLCanvasElement): string { return c.toDataURL("image/jpeg", 0.88); }
function fit(w: number, h: number): [number, number] { const k = Math.min(1, MAX_IMG / Math.max(w, h)); return [Math.round(w * k), Math.round(h * k)]; }

async function imageToJpeg(file: File): Promise<string> {
  const ext = file.name.toLowerCase().split(".").pop() ?? "";
  if (ext === "tif" || ext === "tiff" || file.type === "image/tiff") {
    const buf = await file.arrayBuffer();
    const ifds = UTIF.decode(buf); UTIF.decodeImage(buf, ifds[0]);
    const rgba = UTIF.toRGBA8(ifds[0]);
    const src = document.createElement("canvas"); src.width = ifds[0].width; src.height = ifds[0].height;
    src.getContext("2d")!.putImageData(new ImageData(new Uint8ClampedArray(rgba), src.width, src.height), 0, 0);
    const [w, h] = fit(src.width, src.height);
    const out = document.createElement("canvas"); out.width = w; out.height = h;
    const g = out.getContext("2d")!; g.fillStyle = "#fff"; g.fillRect(0, 0, w, h); g.drawImage(src, 0, 0, w, h);
    return canvasToJpeg(out);
  }
  const bmp = await createImageBitmap(file);
  const [w, h] = fit(bmp.width, bmp.height);
  const c = document.createElement("canvas"); c.width = w; c.height = h;
  const g = c.getContext("2d")!; g.fillStyle = "#fff"; g.fillRect(0, 0, w, h); g.drawImage(bmp, 0, 0, w, h);
  bmp.close();
  return canvasToJpeg(c);
}

async function readPdf(file: File): Promise<{ text: string; images: string[]; warn: string }> {
  const doc = await pdfjs.getDocument({ data: new Uint8Array(await file.arrayBuffer()) }).promise;
  const n = Math.min(doc.numPages, MAX_PDF_PAGES);
  let text = ""; const images: string[] = []; let skipped = 0;
  for (let p = 1; p <= n; p++) {
    const page = await doc.getPage(p);
    const tc = await page.getTextContent();
    const t = tc.items.map((i) => ("str" in i ? i.str : "")).join(" ").replace(/\s+/g, " ").trim();
    if (t.length >= 60) text += `\n[Page ${p}]\n${t}\n`;
    else if (images.length < MAX_PDF_IMG_PAGES) {           // scanned / CamScanner / image-only page
      const v0 = page.getViewport({ scale: 1 });
      const [w] = fit(v0.width, v0.height);
      const vp = page.getViewport({ scale: w / v0.width });
      const c = document.createElement("canvas"); c.width = Math.round(vp.width); c.height = Math.round(vp.height);
      const ctx = c.getContext("2d")!; ctx.fillStyle = "#fff"; ctx.fillRect(0, 0, c.width, c.height);
      await page.render({ canvasContext: ctx, viewport: vp }).promise;
      images.push(canvasToJpeg(c));
    } else skipped++;
  }
  const warn = [doc.numPages > n ? `only the first ${n} of ${doc.numPages} pages were read` : "", skipped ? `${skipped} scanned page(s) skipped (limit ${MAX_PDF_IMG_PAGES} page images)` : ""].filter(Boolean).join("; ");
  return { text, images, warn };
}

const TEXT_EXT = new Set(["txt", "csv", "tsv", "md", "json", "log"]);

export async function processFile(file: File): Promise<Att> {
  const ext = (file.name.toLowerCase().split(".").pop() ?? "");
  const a: Att = { id: crypto.randomUUID(), name: file.name, label: ext.toUpperCase(), thumb: "", images: [], text: "", warn: "" };
  try {
    if (file.type.startsWith("image/") || ["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"].includes(ext)) {
      const j = await imageToJpeg(file); a.images = [j]; a.thumb = j; a.label = "Image";
    } else if (ext === "pdf" || file.type === "application/pdf") {
      const r = await readPdf(file); a.text = r.text; a.images = r.images; a.warn = r.warn; a.thumb = r.images[0] ?? ""; a.label = "PDF";
      if (!a.text && !a.images.length) a.warn = "no readable content found";
    } else if (ext === "docx") {
      a.text = (await mammoth.extractRawText({ arrayBuffer: await file.arrayBuffer() })).value; a.label = "DOCX";
    } else if (TEXT_EXT.has(ext) || file.type.startsWith("text/")) {
      a.text = await file.text(); a.label = ext.toUpperCase() || "TXT";
    } else if (ext === "doc") {
      a.warn = "old .doc files are not supported - save as .docx or PDF";
    } else {
      a.warn = "unsupported file type";
    }
  } catch (e) { a.warn = `could not read file (${(e as Error).message})`; }
  return a;
}
