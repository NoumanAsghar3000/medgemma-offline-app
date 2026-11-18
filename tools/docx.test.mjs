import { test } from "node:test";
import assert from "node:assert/strict";
import mammoth from "mammoth";
import { makeDocx } from "./make_docx.mjs";

test("a generated DOCX is read by mammoth (the library the app uses for DOCX attachments)", async () => {
  const buf = makeDocx(["Patient: Jane Roe, 54 F", "HbA1c 7.9 % & rising <check renal function>", "Plan: repeat CT in 3 months"]);
  const r = await mammoth.extractRawText({ buffer: buf });
  assert.match(r.value, /Jane Roe, 54 F/);
  assert.match(r.value, /HbA1c 7\.9 % & rising <check renal function>/);
  assert.match(r.value, /repeat CT in 3 months/);
});

test("non-ASCII clinical text survives", async () => {
  const r = await mammoth.extractRawText({ buffer: makeDocx(["Befund: Lungenrundherd 8 mm \u2013 \u00b5g/L, \u00fcberwachen"]) });
  assert.match(r.value, /Lungenrundherd 8 mm \u2013 \u00b5g\/L, \u00fcberwachen/);
});

test("an empty document yields empty text instead of throwing (the app then keeps Send disabled)", async () => {
  const r = await mammoth.extractRawText({ buffer: makeDocx([]) });
  assert.equal(r.value.trim(), "");
});
