import { test } from "node:test";
import assert from "node:assert/strict";
import { buildMessages, type Turn } from "./chat.ts";

const roles = (m: { role: string }[]) => m.map((x) => x.role);

test("trimming old turns never leaves an assistant turn first (the chat template rejects that with HTTP 500)", () => {
  // the assistant answer is exactly as long as the remaining budget, so only IT survives the trimming
  const h: Turn[] = [{ role: "user", text: "hi" }, { role: "assistant", text: "A".repeat(5000) }];
  const r = roles(buildMessages(h, "q", [], 100));
  assert.deepEqual(r, ["system", "user"]);
});

test("user/assistant turns keep alternating after trimming at any point", () => {
  for (const n of [1, 50, 400, 1500, 1990, 1999, 2500]) {
    const h: Turn[] = [];
    for (let i = 0; i < 6; i++) h.push({ role: "user", text: "u".repeat(n) }, { role: "assistant", text: "a".repeat(n) });
    const r = roles(buildMessages(h, "next question", [], 100)).slice(1);      // without the system message
    assert.equal(r[0], "user", `n=${n}: ${r.join(",")}`);
    r.forEach((x, i) => assert.equal(x, i % 2 === 0 ? "user" : "assistant", `n=${n}: ${r.join(",")}`));
    assert.equal(r[r.length - 1], "user");
  }
});

test("a short conversation is sent whole", () => {
  const h: Turn[] = [{ role: "user", text: "a" }, { role: "assistant", text: "b" }];
  assert.deepEqual(roles(buildMessages(h, "c", [], 8192)), ["system", "user", "assistant", "user"]);
});
