// Tiny, safe markdown subset (escapes HTML first, so model output can never inject markup).
const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
export function md(src: string): string {
  const parts = esc(src).split(/```([\s\S]*?)```/g);
  return parts.map((p, i) => {
    if (i % 2) return `<pre><code>${p.replace(/^\w*\n/, "")}</code></pre>`;
    const lines = p.split("\n"); const out: string[] = []; let list = false;
    for (const l of lines) {
      const li = /^\s*[-*]\s+(.*)/.exec(l) ?? /^\s*\d+\.\s+(.*)/.exec(l);
      if (li) { if (!list) { out.push("<ul>"); list = true; } out.push(`<li>${inline(li[1])}</li>`); continue; }
      if (list) { out.push("</ul>"); list = false; }
      const h = /^#{1,4}\s+(.*)/.exec(l);
      out.push(h ? `<p><b>${inline(h[1])}</b></p>` : l.trim() ? `<p>${inline(l)}</p>` : "");
    }
    if (list) out.push("</ul>");
    return out.join("");
  }).join("");
}
const inline = (s: string) => s.replace(/\*\*(.+?)\*\*/g, "<b>$1</b>").replace(/`([^`]+)`/g, "<code>$1</code>");
