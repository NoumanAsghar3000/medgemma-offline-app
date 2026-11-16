// Tiny, safe markdown subset (escapes HTML first, so model output can never inject markup).
const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
export function md(src: string): string {
  const parts = esc(src).split(/```([\s\S]*?)```/g);
  return parts.map((p, i) => {
    if (i % 2) return `<pre><code>${p.replace(/^\w*\n/, "")}</code></pre>`;
    const lines = p.split("\n"); const out: string[] = []; let list: "ul" | "ol" | null = null;
    const close = () => { if (list) { out.push(`</${list}>`); list = null; } };
    for (const l of lines) {
      const bu = /^\s*[-*]\s+(.*)/.exec(l); const nu = /^\s*\d+[.)]\s+(.*)/.exec(l); const li = bu ?? nu;
      if (li) {
        const want = nu && !bu ? "ol" : "ul";
        if (list !== want) { close(); out.push(`<${want}>`); list = want; }
        out.push(`<li>${inline(li[1])}</li>`); continue;
      }
      close();
      const h = /^#{1,6}\s+(.*)/.exec(l);
      out.push(h ? `<p><b>${inline(h[1])}</b></p>` : l.trim() ? `<p>${inline(l)}</p>` : "");
    }
    close();
    return out.join("");
  }).join("");
}
const inline = (s: string) => s.replace(/\*\*(.+?)\*\*/g, "<b>$1</b>").replace(/`([^`]+)`/g, "<code>$1</code>");
