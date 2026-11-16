import { Channel, invoke } from "@tauri-apps/api/core";
import type { Att } from "./attachments";

export interface Turn { role: "user" | "assistant"; text: string }
type Part = { type: "text"; text: string } | { type: "image_url"; image_url: { url: string } };

const SYSTEM = "You are MedGemma, a careful medical AI assistant supporting clinicians and health workers. " +
  "When the user provides clinical material (images, scanned pages, documents, lab tables) or asks for an assessment, answer using EXACTLY these markdown sections in this order:\n" +
  "## Findings\n(bullet list of objective observations; name each source image/file; give measurements/values when visible)\n" +
  "## Impression\n(numbered, most likely diagnosis first, with a confidence note and key differentials)\n" +
  "## Recommendations\n(bullets: further tests, treatment considerations, urgency / red flags)\n" +
  "Describe what you actually see before interpreting it, never invent findings that are not visible or stated, and say clearly when image quality or missing data limits the conclusion. " +
  "For ordinary questions without clinical material, answer normally. You are decision support only; the clinician decides.";

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

/** The request goes webview -> Rust -> loopback server. No CORS, and the server key never touches page JS. */
export interface Timings { tps: number; tokens: number; promptTps: number }
export async function streamChat(messages: unknown[], onDelta: (s: string) => void, signal: AbortSignal, onTimings?: (t: Timings) => void) {
  const ch = new Channel<string>();
  ch.onmessage = (d) => {
    if (d === "[DONE]") return;
    try {
      const j = JSON.parse(d); const c = j.choices?.[0]?.delta?.content; if (c) onDelta(c);
      const t = j.timings;                                   // llama-server puts real speed in the final chunk
      if (t && onTimings) onTimings({ tps: Number(t.predicted_per_second) || 0, tokens: Number(t.predicted_n) || 0, promptTps: Number(t.prompt_per_second) || 0 });
    } catch { /* ignore keep-alives */ }
  };
  const onAbort = () => { void invoke("chat_cancel"); };
  signal.addEventListener("abort", onAbort);
  try {
    await invoke("chat_stream", { body: JSON.stringify({ messages, stream: true, temperature: 0.2, max_tokens: 2048 }), onEvent: ch });
    await new Promise((r) => setTimeout(r, 60));          // let the last channel messages drain
  } finally { signal.removeEventListener("abort", onAbort); }
}
