// Turns a CSV/TSV into a compact, model-friendly summary: shape, numeric column stats, and a sample of rows.
// (Sending a 50 000-row file verbatim would blow the context window; statistics + head carry the signal.)
export function parseCsv(text: string): string[][] {
  const first = text.slice(0, 2000).split("\n")[0] ?? "";
  const delim = [",", ";", "\t"].map((d) => [d, first.split(d).length] as const).sort((a, b) => b[1] - a[1])[0][0];
  const rows: string[][] = []; let row: string[] = []; let cur = ""; let q = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (q) { if (c === '"') { if (text[i + 1] === '"') { cur += '"'; i++; } else q = false; } else cur += c; }
    else if (c === '"') q = true;
    else if (c === delim) { row.push(cur); cur = ""; }
    else if (c === "\n" || c === "\r") { if (c === "\r" && text[i + 1] === "\n") i++; row.push(cur); cur = ""; if (row.some((x) => x.trim() !== "")) rows.push(row); row = []; }
    else cur += c;
  }
  row.push(cur); if (row.some((x) => x.trim() !== "")) rows.push(row);
  return rows;
}

export function summarizeCsv(text: string, sampleRows = 30, fullBelowChars = 6000): string {
  if (text.length <= fullBelowChars) return text.trim();
  const rows = parseCsv(text); if (!rows.length) return text.slice(0, fullBelowChars);
  const head = rows[0]; const body = rows.slice(1);
  const lines = [`CSV summary: ${body.length} data rows x ${head.length} columns.`, `Columns: ${head.map((h) => h.trim()).join(" | ")}`];
  const stats: string[] = [];
  head.forEach((h, c) => {
    const nums: number[] = [];
    for (const r of body) { const v = parseFloat((r[c] ?? "").replace(",", ".")); if (Number.isFinite(v) && /^[\s\-+.\d,eE%]+$/.test(r[c] ?? "")) nums.push(v); }
    if (nums.length >= Math.max(3, body.length * 0.6)) {
      let min = Infinity, max = -Infinity, sum = 0;           // plain loop: Math.min(...nums) throws "Maximum call stack size exceeded" for ~125k+ values
      for (const v of nums) { if (v < min) min = v; if (v > max) max = v; sum += v; }
      const mean = sum / nums.length;
      stats.push(`${h.trim()}: n=${nums.length}, min=${+min.toFixed(3)}, max=${+max.toFixed(3)}, mean=${+mean.toFixed(3)}`);
    }
  });
  if (stats.length) lines.push("Numeric columns:", ...stats);
  lines.push(`First ${Math.min(sampleRows, body.length)} rows:`, ...rows.slice(0, sampleRows + 1).map((r) => r.join(" | ")));
  return lines.join("\n");
}
