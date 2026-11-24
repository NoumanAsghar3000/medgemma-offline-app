//! CLI used by the Inno Setup wizard. Writes simple KEY=VALUE lines to `--out`.
//!   mg-probe hw   --out f
//!   mg-probe plan --dir D --compute cpu|gpu --out f
//!   mg-probe doctor --dir D [--compute cpu|gpu]      human report + a machine-readable HEALTH block (HEALTH.verdict=OK|DEGRADED|BROKEN|EMPTY)
//! (A GUI-subsystem build would avoid console flashes, but Inno runs it with SW_HIDE.)
use mg_core::{bench, hw, plan, select, GIB, HEADROOM, MIN_TOTAL_RAM};
use std::{fmt::Write as _, path::Path};

fn arg(a: &[String], k: &str) -> Option<String> {
    a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned()
}
fn gb(b: u64) -> String { format!("{:.1}", b as f64 / GIB as f64) }

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = arg(&a, "--out");
    let mut s = String::new();
    let h = hw::detect();
    match a.first().map(String::as_str) {
        Some("hw") => {
            let _ = writeln!(s, "total_ram={}\ntotal_gb={}\nfree_ram={}\nfree_gb={}", h.total_ram, gb(h.total_ram), h.free_ram, gb(h.free_ram));
            let _ = writeln!(s, "ram_ok={}", (h.total_ram >= MIN_TOTAL_RAM) as u8);
            let _ = writeln!(s, "gpu_ok={}\ngpu_name={}\nvram_gb={}", h.gpu_ok as u8, h.gpu_name, gb(h.vram));
        }
        Some("doctor") => {
            // Human-readable report for supervisors: what is in the model folder, whether it is valid, and what the planner thinks.
            let dir = arg(&a, "--dir").unwrap_or_default();
            let compute = select::Compute::parse(&arg(&a, "--compute").unwrap_or_default());
            let _ = writeln!(s, "MedGemma model folder doctor\nFolder: {dir}\nThis PC: {} GB RAM installed, {} GB free, GPU: {}", gb(h.total_ram), gb(h.free_ram),
                if h.gpu_ok { format!("{} ({} GB VRAM)", h.gpu_name, gb(h.vram)) } else { "none usable".into() });
            match select::scan(Path::new(&dir)) {
                Err(e) => { let _ = writeln!(s, "\nCANNOT READ THE FOLDER: {e}"); }
                Ok(inv) => {
                    // Machine-readable health (parsed by tools/doctor.mjs): which family works, how each file was identified, every problem.
                    let health = select::health(&inv);
                    let _ = writeln!(s, "\n--- HEALTH (machine readable) ---");
                    for l in &health.lines { let _ = writeln!(s, "{l}"); }
                    let _ = writeln!(s, "--- END HEALTH ---");
                    let _ = writeln!(s, "\nFiles found ({}):", inv.files.len());
                    let mut files: Vec<_> = inv.files.iter().collect(); files.sort_by_key(|f| f.path.clone());
                    for f in files {
                        let fam = f.family.map(|x| x.label()).unwrap_or("family unknown");
                        let geo = f.info.as_ref().map(|i| format!("arch={} layers={} embd={}{}", i.arch, i.block_count.map(|x| x.to_string()).unwrap_or("-".into()), i.embedding_length.or(i.projection_dim).map(|x| x.to_string()).unwrap_or("-".into()),
                            i.projector_type.as_ref().map(|p| format!(" projector={p}")).unwrap_or_default())).unwrap_or_else(|| "header not readable".into());
                        let _ = writeln!(s, "  [{}] {}  ({}, {}, {}, {} GB)  {}", if f.problem.is_some() { "BAD " } else { " ok " }, f.path.display(), if f.is_mmproj { "vision projector" } else { "model weights" }, fam, f.quant, gb(f.size), geo);
                        if let Some(p) = &f.problem { let _ = writeln!(s, "         PROBLEM: {p}"); }
                        for w in &f.warnings { let _ = writeln!(s, "         warning: {w}"); }
                    }
                    let probe_file = inv.files.iter().filter(|f| !f.is_mmproj).max_by_key(|f| f.size).map(|f| f.path.clone());
                    let bw = bench::measure(probe_file.as_deref());
                    let mc = select::Machine { budget: h.free_ram.saturating_sub(HEADROOM), compute, vram: h.vram, bw };
                    let _ = writeln!(s, "\nMeasured: memory {:.1} GB/s, drive {:.0} MB/s (a cached file reads faster than the real drive). Usable RAM {} GB. Compute: {:?}", bw.ram_gbps, bw.disk_mbps, gb(mc.budget), compute);
                    let vs = select::variants(&inv, &mc);
                    let _ = writeln!(s, "\nUsable variants (estimated speed range = worst-case .. best-case tokens/s; minimum for automatic choice {}):", plan::MIN_TPS);
                    if vs.is_empty() { let _ = writeln!(s, "  none - see the PROBLEM lines above. Expected layout: 27b\\medgemma-27b-it-Q8_0.gguf + 27b\\mmproj-F16.gguf, 4b\\medgemma-4b-it-Q8_0.gguf + 4b\\mmproj-F16.gguf"); }
                    for p in &vs {
                        let pl = p.placement;
                        let _ = writeln!(s, "  {:<13} {:<8} {:>5} GB  {:>6.2} .. {:<6.2} tok/s  {:<22} GPU {} GB / RAM {} GB / re-read from drive {} GB per token{}", p.family_label, p.quant, gb(p.model_bytes), pl.tps_worst, pl.tps_best,
                            pl.verdict.label(), gb(pl.gpu_bytes), gb(pl.ram_bytes), gb(pl.streamed_bytes), if p.multimodal { "" } else { "  [TEXT ONLY: no vision file]" });
                    }
                    match select::choose(&inv, &mc) {
                        Ok(p) => { let _ = writeln!(s, "\nAutomatic choice: {} {}  ({})", p.family_label, p.quant, p.model.display()); }
                        Err(e) => { let _ = writeln!(s, "\nAutomatic choice: NONE - {e}"); }
                    }
                }
            }
        }
        Some("plan") => {
            let dir = arg(&a, "--dir").unwrap_or_default();
            let compute = select::Compute::parse(&arg(&a, "--compute").unwrap_or_default());
            let budget = h.free_ram.saturating_sub(HEADROOM);
            let _ = writeln!(s, "budget_gb={}", gb(budget));
            let inv = select::scan(Path::new(&dir));
            let probe_file = inv.as_ref().ok().and_then(|i| i.files.iter().filter(|f| !f.is_mmproj).max_by_key(|f| f.size).map(|f| f.path.clone()));
            let bw = bench::measure(probe_file.as_deref());
            let _ = writeln!(s, "ram_gbps={:.1}\ndisk_mbps={:.0}", bw.ram_gbps, bw.disk_mbps);
            let mc = select::Machine { budget, compute, vram: h.vram, bw };
            match inv.map_err(|e| select::SelectError::NoModels(format!("Cannot read folder: {e}"))).and_then(|inv| select::choose(&inv, &mc)) {
                Ok(p) => {
                    let pl = p.placement;
                    let speed = if pl.tps_best - pl.tps_worst < 0.05 { format!("~{:.1} tokens/s", pl.tps_best) } else { format!("~{:.1} to {:.1} tokens/s", pl.tps_worst, pl.tps_best) };
                    let _ = writeln!(s, "status=ok\nfamily={}\nquant={}\nmodel={}\nmmproj={}\nneeds_gb={}\nmultimodal={}\nnote={}",
                        p.family_label, p.quant,
                        p.model.file_name().unwrap().to_string_lossy(),
                        p.mmproj.as_ref().map(|m| m.file_name().unwrap().to_string_lossy().to_string()).unwrap_or_default(),
                        gb(p.required), p.multimodal as u8, p.note);
                    let _ = writeln!(s, "speed={speed}\nverdict={}\ngpu_gb={}\nram_gb={}\nstreamed_gb={}",
                        pl.verdict.label(), gb(pl.gpu_bytes), gb(pl.ram_bytes), gb(pl.streamed_bytes));
                    let _ = writeln!(s, "min_tps={}", plan::MIN_TPS);
                    // sizes in MiB: the installer checks free disk space BEFORE it copies a multi-GB model into the install folder
                    let mib = |b: u64| b / (1024 * 1024);
                    let _ = writeln!(s, "model_mb={}\nmmproj_mb={}", mib(p.model_bytes), mib(if p.mmproj.is_some() { p.mmproj_bytes } else { 0 }));
                    let _ = writeln!(s, "model_path={}\nmmproj_path={}", p.model.display(), p.mmproj.map(|m| m.display().to_string()).unwrap_or_default());
                }
                Err(e) => { let _ = writeln!(s, "status=error\nmessage={e}"); }
            }
        }
        _ => { eprintln!("usage: mg-probe hw|plan [--dir D] [--compute cpu|gpu] --out FILE"); std::process::exit(2); }
    }
    // `--out` files are read by Inno Setup's TStringList.LoadFromFile, which decodes a file WITHOUT a byte-order mark as ANSI: a model folder
    // with non-ASCII characters (accents, Arabic/Urdu/Chinese names) would come back garbled and the copy / validation would fail. A UTF-8 BOM fixes that.
    match out {
        Some(f) => { let mut bytes = vec![0xEF, 0xBB, 0xBF]; bytes.extend_from_slice(s.as_bytes()); std::fs::write(f, bytes).expect("write out") }
        None => print!("{s}"),
    }
}
