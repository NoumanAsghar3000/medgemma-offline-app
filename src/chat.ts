import type { Att } from "./attachments";

export interface Turn { role: "user" | "assistant"; text: string }
type Part = { type: "text"; text: string } | { type: "image_url"; image_url: { url: string } };

const SYSTEM = "You are MedGemma, a careful medical AI assistant supporting clinicians and health workers. " +
  "Be precise and structured. Describe what you see in images/documents before interpreting them. State uncertainty and " +
  "red-flag findings clearly. You are decision support only; remind the user to confirm with clinical judgement and local guidelines when it matters.";

/** Build the OpenAI-style message list. Older turns are text-only and trimmed to fit the context window. */
export function buildMessages(history: Turn[], text: string, atts: Att[], ctx: number) {
  const nImg = atts.reduce((n, a) => n + a.images.length, 0);
  const reserveTokens = 1800 + nImg * 300;               // reply + images + system prompt
  const charBudget = Math.max(2000, (ctx - reserveTokens) * 3);

  let docChars = Math.floor(charBudget * 0.75), docs = "";
  for (const a of atts.filter((x) => x.text)) {
    let t = a.text.trim();
    if (t.length > docChars) t = t.slice(0, Math.max(0, docChars)) + "\n[... truncated to fit the model's context window ...]";
    docs += `\n\n--- Attached file: ${a.name} ---\n${t}`; docChars -= t.length;
    if (docChars <= 0) break;
  }
  const parts: Part[] = [{ type: "text", text: (text || "Please analyse the attached material.") + docs }];
  for (const a of atts) for (const u of a.images) parts.push({ type: "image_url", image_url: { url: u } });

  const firstText = (parts[0] as { text: string }).text;
  let left = charBudget - firstText.length;           // what remains for earlier turns
  const past: { role: string; content: string }[] = [];
  for (let i = history.length - 1; i >= 0 && left > 0; i--) {
    const t = history[i].text.slice(0, left); left -= t.length; past.unshift({ role: history[i].role, content: t });
  }
  return [{ role: "system", content: SYSTEM }, ...past, { role: "user", content: nImg ? parts : firstText }];
}

export async function streamChat(base: string, token: string, messages: unknown[], onDelta: (s: string) => void, signal: AbortSignal) {
  const r = await fetch(`${base}/v1/chat/completions`, {
    method: "POST", signal,
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({ messages, stream: true, temperature: 0.3, max_tokens: 1536 }),
  });
  if (!r.ok || !r.body) throw new Error(`Backend error ${r.status}: ${(await r.text()).slice(0, 300)}`);
  const rd = r.body.getReader(); const dec = new TextDecoder(); let buf = "";
  for (;;) {
    const { done, value } = await rd.read(); if (done) break;
    buf += dec.decode(value, { stream: true });
    let i: number;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1);
      if (!line.startsWith("data:")) continue;
      const d = line.slice(5).trim(); if (d === "[DONE]") return;
      try { const j = JSON.parse(d); const c = j.choices?.[0]?.delta?.content; if (c) onDelta(c); } catch { /* partial JSON */ }
    }
  }
}
