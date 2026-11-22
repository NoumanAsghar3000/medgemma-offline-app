import { test } from "node:test";
import assert from "node:assert/strict";
import { declaredZipBytes } from "./zipguard.ts";

/** A minimal ZIP with STORED entries (no compression) whose central directory DECLARES the given uncompressed sizes. */
function zip(entries: { name: string; declared: number }[]): ArrayBuffer {
  const enc = new TextEncoder(); const locals: Uint8Array[] = []; const central: Uint8Array[] = []; let off = 0;
  for (const e of entries) {
    const name = enc.encode(e.name);
    const lh = new DataView(new ArrayBuffer(30 + name.length)); lh.setUint32(0, 0x04034b50, true); lh.setUint16(26, name.length, true);
    new Uint8Array(lh.buffer).set(name, 30); locals.push(new Uint8Array(lh.buffer));
    const ch = new DataView(new ArrayBuffer(46 + name.length)); ch.setUint32(0, 0x02014b50, true); ch.setUint32(24, e.declared, true);
    ch.setUint16(28, name.length, true); ch.setUint32(42, off, true); new Uint8Array(ch.buffer).set(name, 46); central.push(new Uint8Array(ch.buffer));
    off += lh.byteLength;
  }
  const cdSize = central.reduce((a, c) => a + c.length, 0);
  const end = new DataView(new ArrayBuffer(22)); end.setUint32(0, 0x06054b50, true); end.setUint16(8, entries.length, true); end.setUint16(10, entries.length, true);
  end.setUint32(12, cdSize, true); end.setUint32(16, off, true);
  const all = [...locals, ...central, new Uint8Array(end.buffer)]; const out = new Uint8Array(all.reduce((a, c) => a + c.length, 0)); let o = 0;
  for (const c of all) { out.set(c, o); o += c.length; }
  return out.buffer;
}

test("sums the declared sizes of a normal docx-like archive", () => {
  assert.equal(declaredZipBytes(zip([{ name: "[Content_Types].xml", declared: 1200 }, { name: "word/document.xml", declared: 50_000 }])), 51_200);
});
test("a bomb declares a huge size and is visible before anything is inflated", () => {
  assert.ok(declaredZipBytes(zip([{ name: "word/document.xml", declared: 0x7fffffff }, { name: "x", declared: 0x7fffffff }])) > 4_000_000_000);
});
test("ZIP64 markers, truncated and non-zip data are treated as unjudgeable", () => {
  assert.equal(declaredZipBytes(zip([{ name: "a", declared: 0xffffffff }])), Infinity);
  assert.equal(declaredZipBytes(new TextEncoder().encode("not a zip at all, just some text bytes").buffer), Infinity);
  assert.equal(declaredZipBytes(new ArrayBuffer(5)), Infinity);
  const z = new Uint8Array(zip([{ name: "a", declared: 10 }])); assert.equal(declaredZipBytes(z.slice(0, z.length - 30).buffer), Infinity);
});
test("an absurd entry count is refused", () => {
  const z = zip([{ name: "a", declared: 1 }]); const dv = new DataView(z); const e = z.byteLength - 22; dv.setUint16(e + 10, 30000, true);
  assert.equal(declaredZipBytes(z), Infinity);
});
