declare module "utif" { const U: any; export default U; }
declare module "mammoth/mammoth.browser" { const m: { extractRawText(o: { arrayBuffer: ArrayBuffer }): Promise<{ value: string }> }; export default m; }
declare module "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url" { const u: string; export default u; }
