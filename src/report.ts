// Structured medical report: parses the model's answer into Findings / Impression / Recommendations and
// renders a professional A4 PDF fully offline (pdf-lib is bundled; no network, no external service).
import { PDFDocument, StandardFonts, rgb, type PDFFont, type PDFPage } from "pdf-lib";

export interface CaseInfo { patientName: string; patientId: string; ageSex: string; clinician: string; notes: string }
export interface ReportFile { name: string; kind: string; thumb?: string }
export interface ReportInput { caseInfo: CaseInfo; question: string; answer: string; files: ReportFile[]; model: string; when: Date }
export interface Sections { summary: string; findings: string; impression: string; recommendations: string }

const WORDS = "findings?|impressions?|conclusions?|assessment|diagnos[ie]s|recommendations?|plan|advice|clinical summary|summary";
const KEY = (w: string): keyof Sections => {
  w = w.toLowerCase();
  if (w.startsWith("finding")) return "findings";
  if (w.startsWith("impression") || w.startsWith("conclusion") || w.startsWith("assessment") || w.startsWith("diagnos")) return "impression";
  if (w.startsWith("recommend") || w === "plan" || w === "advice") return "recommendations";
  return "summary";
};
const RES = [
  new RegExp(`^#{1,4}\\s*(?:\\d+[.)]\\s*)?(?:\\*\\*)?(${WORDS})(?:\\*\\*)?\\s*:?\\s*(.*)$`, "i"),
  new RegExp(`^\\*\\*\\s*(?:\\d+[.)]\\s*)?(${WORDS})\\s*:?\\s*\\*\\*\\s*:?\\s*(.*)$`, "i"),
  new RegExp(`^(?:\\d+[.)]\\s*)?(${WORDS})\\s*:\\s*(.*)$`, "i"),
];

/** True when the answer contains at least one Findings / Impression / Recommendations style heading. */
export function isStructured(text: string): boolean {
  return text.replace(/\r/g, "").split("\n").some((l) => RES.some((re) => re.test(l.trim())));
}

export function parseSections(text: string): Sections {
  const out: Sections = { summary: "", findings: "", impression: "", recommendations: "" };
  let cur: keyof Sections = "summary"; let found = false;
  for (const line of text.replace(/\r/g, "").split("\n")) {
    const t = line.trim(); let m: RegExpExecArray | null = null;
    for (const re of RES) { m = re.exec(t); if (m) break; }
    if (m) { cur = KEY(m[1]); found = true; if (m[2]) out[cur] += m[2] + "\n"; continue; }
    out[cur] += line + "\n";
  }
  if (!found) return { summary: "", findings: text.trim(), impression: "", recommendations: "" };
  for (const k of Object.keys(out) as (keyof Sections)[]) out[k] = out[k].trim();
  return out;
}

/** Standard PDF fonts only cover WinAnsi: map common medical symbols, replace the rest. */
export function pdfSafe(s: string): string {
  return s
    .replace(/[\u2013\u2014\u2212]/g, "-").replace(/[\u2018\u2019]/g, "'").replace(/[\u201C\u201D]/g, '"')
    .replace(/\u2022|\u25CF|\u25AA/g, "-").replace(/\u2026/g, "...").replace(/\u2265/g, ">=").replace(/\u2264/g, "<=")
    .replace(/\u2192/g, "->").replace(/\u2190/g, "<-").replace(/\u00D7/g, "x").replace(/\u2248/g, "~").replace(/\u03BC/g, "u")
    .replace(/\t/g, "  ").replace(/\*\*|__/g, "").replace(/`/g, "")
    .replace(/[^\n\x20-\x7E\xA0-\xFF]/g, "?");
}

const A4: [number, number] = [595.28, 841.89];
const M = 50, BODY = 10, LEAD = 14;
const INK = rgb(0.1, 0.12, 0.16), MUTED = rgb(0.4, 0.43, 0.5), ACCENT = rgb(0.06, 0.45, 0.56);

function wrap(font: PDFFont, text: string, size: number, width: number): string[] {
  const out: string[] = [];
  for (const para of text.split("\n")) {
    if (!para.trim()) { out.push(""); continue; }
    let line = "";
    for (const word of para.split(/(\s+)/)) {
      if (!word) continue;
      const trial = line + word;
      if (font.widthOfTextAtSize(trial, size) <= width || !line) {
        // hard-split single very long tokens
        if (font.widthOfTextAtSize(trial, size) > width && !line) {
          let chunk = ""; for (const ch of word) { if (font.widthOfTextAtSize(chunk + ch, size) > width) { out.push(chunk); chunk = ""; } chunk += ch; }
          line = chunk;
        } else line = trial;
      } else { out.push(line.trimEnd()); line = word.trimStart(); }
    }
    out.push(line.trimEnd());
  }
  return out;
}

function dataUrlBytes(u: string): Uint8Array {
  const b = atob(u.slice(u.indexOf(",") + 1)); const a = new Uint8Array(b.length);
  for (let i = 0; i < b.length; i++) a[i] = b.charCodeAt(i);
  return a;
}

/** Returns the PDF as base64 (what the native "Save as" command expects). */
export async function buildReportPdf(r: ReportInput): Promise<string> {
  const doc = await PDFDocument.create();
  doc.setTitle("Medical Analysis Report"); doc.setProducer("MedGemma Offline"); doc.setCreator("MedGemma Offline");
  doc.setCreationDate(r.when);
  const reg = await doc.embedFont(StandardFonts.Helvetica), bold = await doc.embedFont(StandardFonts.HelveticaBold);
  const W = A4[0] - 2 * M;
  let page: PDFPage = doc.addPage(A4); let y = A4[1] - M;
  const newPage = () => { page = doc.addPage(A4); y = A4[1] - M; };
  const need = (h: number) => { if (y - h < M + 30) newPage(); };

  const text = (s: string, o: { size?: number; font?: PDFFont; color?: ReturnType<typeof rgb>; indent?: number } = {}) => {
    const size = o.size ?? BODY, font = o.font ?? reg, ind = o.indent ?? 0;
    for (const l of wrap(font, pdfSafe(s), size, W - ind)) {
      need(LEAD); if (l) page.drawText(l, { x: M + ind, y: y - size, size, font, color: o.color ?? INK }); y -= size + 4;
    }
  };
  const heading = (s: string) => {
    need(40); y -= 8;
    page.drawText(s.toUpperCase(), { x: M, y: y - 11, size: 11, font: bold, color: ACCENT }); y -= 16;
    page.drawLine({ start: { x: M, y }, end: { x: M + W, y }, thickness: 0.8, color: ACCENT }); y -= 8;
  };
  const markdownBlock = (md: string) => {
    if (!md.trim()) { text("Not provided.", { color: MUTED }); return; }
    for (const raw of md.split("\n")) {
      const l = raw.trimEnd();
      const h = /^#{1,6}\s+(.*)$/.exec(l); const b = /^\s*[-*\u2022]\s+(.*)$/.exec(l); const n = /^\s*(\d+[.)])\s+(.*)$/.exec(l);
      if (h) { y -= 2; text(h[1], { font: bold }); }
      else if (b) { need(LEAD); page.drawText("-", { x: M + 6, y: y - BODY, size: BODY, font: reg, color: INK }); text(b[1], { indent: 18 }); }
      else if (n) { need(LEAD); page.drawText(pdfSafe(n[1]), { x: M + 4, y: y - BODY, size: BODY, font: reg, color: INK }); text(n[2], { indent: 20 }); }
      else if (!l.trim()) y -= 4;
      else text(l);
    }
  };

  // ---- title
  page.drawRectangle({ x: 0, y: A4[1] - 74, width: A4[0], height: 74, color: ACCENT });
  page.drawText("MEDICAL ANALYSIS REPORT", { x: M, y: A4[1] - 38, size: 20, font: bold, color: rgb(1, 1, 1) });
  page.drawText("AI-assisted decision support - generated offline", { x: M, y: A4[1] - 58, size: 10, font: reg, color: rgb(0.9, 0.97, 1) });
  y = A4[1] - 74 - 20;

  // ---- case header
  heading("Patient / Case");
  const ci = r.caseInfo; const blank = "____________________";
  const rows: [string, string][] = [
    ["Patient name", ci.patientName || blank], ["Patient ID / MRN", ci.patientId || blank],
    ["Age / Sex", ci.ageSex || blank], ["Referring clinician", ci.clinician || blank],
    ["Report date", r.when.toLocaleString()], ["Analysis model", r.model],
  ];
  for (let i = 0; i < rows.length; i += 2) {
    need(18);
    for (let c = 0; c < 2 && rows[i + c]; c++) {
      const x = M + c * (W / 2);
      page.drawText(pdfSafe(rows[i + c][0] + ":"), { x, y: y - 10, size: 9, font: bold, color: MUTED });
      page.drawText(pdfSafe(rows[i + c][1]).slice(0, 44), { x: x + 98, y: y - 10, size: 9.5, font: reg, color: INK });
    }
    y -= 17;
  }
  if (ci.notes.trim()) { y -= 2; text("Clinical notes: " + ci.notes, { size: 9.5 }); }

  // ---- body
  const s = parseSections(r.answer);
  if (r.question.trim()) { heading("Clinical question"); text(r.question.trim()); }
  if (s.summary) { heading("Summary"); markdownBlock(s.summary); }
  heading("Findings"); markdownBlock(s.findings);
  heading("Impression / Conclusion"); markdownBlock(s.impression);
  heading("Recommendations"); markdownBlock(s.recommendations);

  // ---- analysed files
  heading("Analyzed files and images");
  if (!r.files.length) text("No files were attached (text-only consultation).", { color: MUTED });
  r.files.forEach((f, i) => text(`${i + 1}. ${f.name}  (${f.kind})`, { size: 9.5 }));
  const thumbs = r.files.filter((f) => f.thumb).slice(0, 8);
  if (thumbs.length) {
    y -= 6; const tw = (W - 3 * 10) / 4; let col = 0; let rowH = 0;
    for (const f of thumbs) {
      try {
        const img = await doc.embedJpg(dataUrlBytes(f.thumb!)); const sc = Math.min(tw / img.width, 120 / img.height);
        const w = img.width * sc, h = img.height * sc; rowH = Math.max(rowH, h + 14);
        if (col === 0) need(rowH + 4);
        page.drawImage(img, { x: M + col * (tw + 10), y: y - h, width: w, height: h });
        page.drawText(pdfSafe(f.name).slice(0, 26), { x: M + col * (tw + 10), y: y - h - 10, size: 7, font: reg, color: MUTED });
        if (++col === 4) { col = 0; y -= rowH + 4; rowH = 0; }
      } catch { /* skip an image that cannot be embedded */ }
    }
    if (col) y -= rowH + 4;
  }

  // ---- disclaimer + footers on every page
  need(60); y -= 10;
  text("This report was produced by an AI model running locally on this computer. It is decision support only and does not replace the judgement of a qualified clinician. The underlying MedGemma model is not intended to directly inform clinical diagnosis or patient management, has been evaluated mainly on single-image tasks, and its output is preliminary: verify every finding against the original images, documents and clinical context before any medical action.", { size: 8.5, color: MUTED });
  const pages = doc.getPages();
  pages.forEach((p, i) => {
    p.drawLine({ start: { x: M, y: M - 6 }, end: { x: A4[0] - M, y: M - 6 }, thickness: 0.5, color: MUTED });
    p.drawText(pdfSafe(`MedGemma Offline  |  ${r.model}  |  ${r.when.toISOString().replace("T", " ").slice(0, 16)} UTC`), { x: M, y: M - 20, size: 7.5, font: reg, color: MUTED });
    p.drawText(`Page ${i + 1} of ${pages.length}`, { x: A4[0] - M - 50, y: M - 20, size: 7.5, font: reg, color: MUTED });
  });
  return doc.saveAsBase64();
}
