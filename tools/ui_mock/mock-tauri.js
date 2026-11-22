/* TEST-ONLY mock of the Tauri IPC layer so the built UI can run in a normal browser (Firefox in CI).
   It reproduces the contract the real shell exposes: commands, the 'status' event and Channel streaming. Scenario: ?scenario=relink|ready */
(() => {
  const cbs = {}; let nid = 1; const handlers = {}; let cancel = false;
  const scenario = new URLSearchParams(location.search).get("scenario") || "ready";
  const GB = 1;
  const base = { state: "ready", message: "", model_dir: "D:\\models", compute: "cpu", model: "medgemma-27b-it-Q4_K_M.gguf", quant: "Q4_K_M", family: "MedGemma 27B", multimodal: true,
    ctx: 8192, note: "", idle_minutes: 5, tps_best: 2.4, tps_worst: 2.3, verdict: "interactive", streamed_gb: 0, forced: false, model_file: "medgemma-27b-it-Q4_K_M.gguf", gpu_available: true };
  let status = scenario === "relink"
    ? { ...base, state: "need_model_folder", model: "", model_file: "", family: "", quant: "", message: "Backend model files not found. The model files in this folder cannot be used - medgemma-27b-it-Q8_0.gguf: mmproj-F16.gguf was made for a model with embedding size 2560, but this model has 5376: use the matching mmproj (27B: 5376, 4B: 2560)", model_dir: "D:\\models-old" }
    : { ...base };
  const variants = [
    { file: "medgemma-27b-it-Q8_0.gguf", family: "MedGemma 27B", quant: "Q8_0", size_gb: 28.7, multimodal: true, tps_best: 0.11, tps_worst: 0.1, verdict: "batch only (very slow)", streamed_gb: 24, auto: false, current: false },
    { file: "medgemma-27b-it-Q4_K_M.gguf", family: "MedGemma 27B", quant: "Q4_K_M", size_gb: 16.5, multimodal: true, tps_best: 2.4, tps_worst: 2.3, verdict: "interactive", streamed_gb: 0, auto: true, current: true },
    { file: "medgemma-4b-it-Q8_0.gguf", family: "MedGemma 4B", quant: "Q8_0", size_gb: 4.1, multimodal: true, tps_best: 9.2, tps_worst: 9.0, verdict: "interactive", streamed_gb: 0, auto: false, current: false },
  ];
  const ANSWER = "## Findings\n- Right breast, upper outer quadrant: 12 mm irregular, spiculated mass with associated architectural distortion.\n- Scattered fibroglandular density (BI-RADS category B). No suspicious microcalcifications.\n- Left breast: no mass, asymmetry or skin thickening.\n\n## Impression\n1. Suspicious right breast mass - **BI-RADS 4C**; malignancy cannot be excluded.\n2. Left breast: negative (BI-RADS 1).\n\n## Recommendations\n- Image-guided core needle biopsy of the right breast mass.\n- Targeted ultrasound correlation and axillary assessment.\n- Multidisciplinary review once pathology is available.";
  const emit = (name, payload) => (handlers[name] || []).forEach((h) => cbs[h]?.({ event: name, id: 1, payload }));
  const setStatus = (s) => { status = s; emit("status", status); };
  const load = (patch, ms = 700) => { setStatus({ ...status, ...patch, state: "loading", message: "Loading model (mock)..." }); setTimeout(() => setStatus({ ...status, ...patch, state: "ready", message: "" }), ms); };
  window.__saved = null; window.__calls = [];

  window.__TAURI_INTERNALS__ = {
    transformCallback(cb) { const id = nid++; cbs[id] = cb; return id; },
    unregisterCallback(id) { delete cbs[id]; },
    convertFileSrc: (p) => p,
    async invoke(cmd, args = {}) {
      window.__calls.push(cmd);
      switch (cmd) {
        case "plugin:event|listen": (handlers[args.event] ||= []).push(args.handler); return 1;
        case "plugin:event|unlisten": return null;
        case "get_status": return status;
        case "list_variants": return variants.map((v) => ({ ...v, current: v.file === status.model_file }));
        case "set_memory_mode": status = { ...status, idle_minutes: args.minutes }; emit("status", status); return null;
        case "retry": load({}); return null;
        case "set_compute": load({ compute: args.compute }); return null;
        case "choose_variant": {
          const v = variants.find((x) => x.file === args.file) || variants[1];
          load({ model: v.file, model_file: v.file, quant: v.quant, family: v.family, tps_best: v.tps_best, tps_worst: v.tps_worst, verdict: v.verdict, streamed_gb: v.streamed_gb, forced: v.tps_worst < 0.5 });
          return null;
        }
        case "pick_model_folder": load({ model_dir: "D:\\models", model: "medgemma-27b-it-Q4_K_M.gguf", model_file: "medgemma-27b-it-Q4_K_M.gguf", family: "MedGemma 27B", quant: "Q4_K_M", multimodal: true }); return true;
        case "chat_cancel": cancel = true; return null;
        case "save_report": window.__saved = { filename: args.filename, bytes: Math.round((args.dataB64.length * 3) / 4), head: atob(args.dataB64.slice(0, 12)) }; return "C:\\Reports\\" + args.filename;
        case "chat_stream": {
          cancel = false; const ch = args.onEvent; let i = 0; const parts = ANSWER.match(/.{1,4}/gs);
          await new Promise((resolve) => {
            const tick = () => {
              if (cancel || i >= parts.length) {
                if (!cancel) {
                  cbs[ch.id]({ index: i++, message: JSON.stringify({ choices: [{ delta: {}, finish_reason: "stop" }], timings: { predicted_per_second: 2.31, predicted_n: parts.length, prompt_per_second: 41.2 } }) });
                  cbs[ch.id]({ index: i++, message: "[DONE]" });
                }
                cbs[ch.id]({ index: i, end: true }); return resolve();
              }
              cbs[ch.id]({ index: i, message: JSON.stringify({ choices: [{ delta: { content: parts[i] } }] }) }); i++; setTimeout(tick, 30);
            }; tick();
          });
          return null;
        }
        default: throw new Error("mock: unhandled command " + cmd);
      }
    },
  };
  void GB;
})();
