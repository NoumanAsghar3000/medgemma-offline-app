import { test } from "node:test";
import assert from "node:assert/strict";
import { summarizeCsv } from "./csv.ts";

test("a CSV with hundreds of thousands of rows is summarised (Math.min(...values) used to overflow the stack)", () => {
  const rows = ["value,label"]; for (let i = 0; i < 300_000; i++) rows.push(`${i % 1000},x`);
  const s = summarizeCsv(rows.join("\n"));
  assert.match(s, /300000 data rows/); assert.match(s, /value: n=300000, min=0, max=999/);
});

test("numeric statistics (decimal comma, ; delimiter) are right once the file is too big to send whole", () => {
  const rows = ["a;b"]; for (let i = 0; i < 2001; i++) rows.push(`${1 + (i % 3)},5;x`);    // 1,5 / 2,5 / 3,5 repeating
  const s = summarizeCsv(rows.join("\n"));
  assert.match(s, /a: n=2001, min=1.5, max=3.5, mean=2.5/);
});

test("a small file is passed through whole", () => {
  assert.equal(summarizeCsv("a;b\n1;2"), "a;b\n1;2");
});
