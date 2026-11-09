//! CLI used by the Inno Setup wizard. Writes simple KEY=VALUE lines to `--out`.
//!   mg-probe hw   --out f
//!   mg-probe plan --dir D --compute cpu|gpu --out f
//! (A GUI-subsystem build would avoid console flashes, but Inno runs it with SW_HIDE.)
use mg_core::{hw, select, GIB, HEADROOM, MIN_TOTAL_RAM};
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
        Some("plan") => {
            let dir = arg(&a, "--dir").unwrap_or_default();
            let compute = select::Compute::parse(&arg(&a, "--compute").unwrap_or_default());
            let budget = h.free_ram.saturating_sub(HEADROOM);
            let _ = writeln!(s, "budget_gb={}", gb(budget));
            match select::scan(Path::new(&dir)).map_err(|e| select::SelectError::NoModels(format!("Cannot read folder: {e}")))
                .and_then(|inv| select::choose(&inv, budget, compute, h.vram)) {
                Ok(p) => {
                    let _ = writeln!(s, "status=ok\nfamily={}\nquant={}\nmodel={}\nmmproj={}\nneeds_gb={}\nmultimodal={}\nnote={}",
                        p.family_label, p.quant,
                        p.model.file_name().unwrap().to_string_lossy(),
                        p.mmproj.as_ref().map(|m| m.file_name().unwrap().to_string_lossy().to_string()).unwrap_or_default(),
                        gb(p.required), p.multimodal as u8, p.note);
                    let _ = writeln!(s, "model_path={}\nmmproj_path={}", p.model.display(), p.mmproj.map(|m| m.display().to_string()).unwrap_or_default());
                }
                Err(e) => { let _ = writeln!(s, "status=error\nmessage={e}"); }
            }
        }
        _ => { eprintln!("usage: mg-probe hw|plan [--dir D] [--compute cpu|gpu] --out FILE"); std::process::exit(2); }
    }
    match out { Some(f) => std::fs::write(f, s).expect("write out"), None => print!("{s}") }
}
