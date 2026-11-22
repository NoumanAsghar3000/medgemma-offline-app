// Guard against "zip bombs" in .docx files (a docx is a ZIP). JSZip, which mammoth uses, inflates entries fully in memory and verifies
// the declared sizes, so the sizes in the central directory are trustworthy for a go/no-go decision BEFORE anything is inflated.

/** Sum of the uncompressed sizes declared in the ZIP central directory, or Infinity when the archive looks malformed / ZIP64 / too large to judge. */
export function declaredZipBytes(buf: ArrayBuffer, maxEntries = 20000): number {
  const v = new DataView(buf);
  const n = v.byteLength;
  if (n < 22) return Infinity;
  let eocd = -1;
  for (let i = n - 22; i >= Math.max(0, n - 22 - 0xffff); i--) { if (v.getUint32(i, true) === 0x06054b50) { eocd = i; break; } }
  if (eocd < 0) return Infinity;
  const entries = v.getUint16(eocd + 10, true);
  let p = v.getUint32(eocd + 16, true);
  if (entries === 0xffff || p === 0xffffffff || entries > maxEntries) return Infinity;
  let total = 0;
  for (let e = 0; e < entries; e++) {
    if (p + 46 > n || v.getUint32(p, true) !== 0x02014b50) return Infinity;
    const size = v.getUint32(p + 24, true);
    if (size === 0xffffffff) return Infinity;
    total += size;
    p += 46 + v.getUint16(p + 28, true) + v.getUint16(p + 30, true) + v.getUint16(p + 32, true);
  }
  return total;
}
