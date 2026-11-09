//! Model discovery + "largest MedGemma multimodal variant that fits" selection.
//!
//! Priority (strict):
//!   1. MedGemma 27B *multimodal* (GGUF + mmproj). Largest quantization whose
//!      (weights + mmproj + runtime overhead) <= free RAM - 1 GiB.
//!   2. MedGemma 4B multimodal (same rule).
//!   3. MedGemma 4B text-only (only if no 4B mmproj exists; flagged in the plan).
use crate::GIB;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Family { G27, G4 }
impl Family {
    pub fn label(self) -> &'static str { match self { Family::G27 => "MedGemma 27B", Family::G4 => "MedGemma 4B" } }
    pub fn layers(self) -> u64 { match self { Family::G27 => 62, Family::G4 => 34 } }
    /// KV cache + compute buffers + vision encoder activations at our context size.
    pub fn overhead(self) -> u64 { match self { Family::G27 => GIB * 3 / 2, Family::G4 => GIB * 3 / 4 } }
    pub fn ctx(self) -> u32 { match self { Family::G27 => 8192, Family::G4 => 16384 } }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compute { Cpu, Gpu }
impl Compute {
    pub fn parse(s: &str) -> Compute { if s.eq_ignore_ascii_case("gpu") { Compute::Gpu } else { Compute::Cpu } }
}

#[derive(Debug, Clone)]
pub struct GgufFile {
    pub path: PathBuf,
    pub size: u64,
    pub family: Option<Family>,
    pub quant: String,
    pub is_mmproj: bool,
    pub text_only: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Inventory { pub files: Vec<GgufFile> }

const QUANTS: &[&str] = &[
    "IQ4_XS", "IQ3_XXS", "IQ3_XS", "IQ3_M", "IQ2_M", "IQ2_S", "IQ1_M",
    "Q8_0", "Q6_K_XL", "Q6_K", "Q5_K_XL", "Q5_K_M", "Q5_K_S", "Q5_0", "Q4_K_XL", "Q4_K_M", "Q4_K_S", "Q4_0", "Q4_1",
    "Q3_K_XL", "Q3_K_L", "Q3_K_M", "Q3_K_S", "Q2_K_XL", "Q2_K", "BF16", "F16", "F32",
];

fn has_token(lower: &str, tok: &str) -> bool {
    // match "27b"/"4b" only as a whole size token (not "14b", "24b")
    let b = lower.as_bytes();
    let mut start = 0;
    while let Some(i) = lower[start..].find(tok) {
        let s = start + i;
        let e = s + tok.len();
        let before_ok = s == 0 || !b[s - 1].is_ascii_digit();
        let after_ok = e >= b.len() || !b[e].is_ascii_alphanumeric();
        if before_ok && after_ok { return true; }
        start = s + 1;
    }
    false
}

pub fn classify(path: &Path, size: u64) -> Option<GgufFile> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".gguf") { return None; }
    let family = if has_token(&lower, "27b") { Some(Family::G27) } else if has_token(&lower, "4b") { Some(Family::G4) } else { None };
    let upper = name.to_ascii_uppercase().replace('-', "_").replace('.', "_");
    let quant = QUANTS.iter().find(|q| upper.contains(*q)).map(|q| q.to_string()).unwrap_or_else(|| "unknown".into());
    Some(GgufFile {
        path: path.to_path_buf(), size, family, quant,
        is_mmproj: lower.contains("mmproj"),
        text_only: lower.contains("text"),
    })
}

/// Scan `dir` (depth <= 3) for .gguf files.
pub fn scan(dir: &Path) -> std::io::Result<Inventory> {
    let mut inv = Inventory::default();
    fn walk(d: &Path, depth: u32, inv: &mut Inventory) -> std::io::Result<()> {
        for e in std::fs::read_dir(d)? {
            let e = match e { Ok(e) => e, Err(_) => continue };
            let p = e.path();
            let md = match e.metadata() { Ok(m) => m, Err(_) => continue };
            if md.is_dir() { if depth < 3 { let _ = walk(&p, depth + 1, inv); } }
            else if let Some(f) = classify(&p, md.len()) { inv.files.push(f); }
        }
        Ok(())
    }
    walk(dir, 0, &mut inv)?;
    Ok(inv)
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub family: Family,
    pub family_label: String,
    pub model: PathBuf,
    pub mmproj: Option<PathBuf>,
    pub quant: String,
    pub model_bytes: u64,
    pub mmproj_bytes: u64,
    pub required: u64,
    pub budget: u64,
    pub multimodal: bool,
    pub ctx: u32,
    pub n_gpu_layers: u32,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectError {
    /// No usable MedGemma GGUF found in the folder.
    NoModels(String),
    /// Models exist but none fits `budget`; `smallest` is what the cheapest needs.
    TooLittleRam { budget: u64, smallest: u64 },
}
impl std::fmt::Display for SelectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectError::NoModels(s) => write!(f, "{s}"),
            SelectError::TooLittleRam { budget, smallest } => write!(
                f, "Not enough free RAM: {:.1} GB usable (free RAM minus 1 GB reserve), but the smallest model needs {:.1} GB. Close other programs and try again.",
                *budget as f64 / GIB as f64, *smallest as f64 / GIB as f64),
        }
    }
}

fn best_mmproj(inv: &Inventory, fam: Family) -> Option<&GgufFile> {
    let rank = |f: &GgufFile| match f.quant.as_str() { "F16" => 0, "BF16" => 1, "Q8_0" => 2, _ => 3 };
    inv.files.iter().filter(|f| f.is_mmproj && f.family == Some(fam)).min_by_key(|f| rank(f))
        .or_else(|| {
            // an mmproj without a size token is shared between families only if there is exactly one
            let m: Vec<_> = inv.files.iter().filter(|f| f.is_mmproj && f.family.is_none()).collect();
            if m.len() == 1 { Some(m[0]) } else { None }
        })
}

/// `budget` = free RAM - HEADROOM (caller computes). `vram` only affects GPU layer offload.
pub fn choose(inv: &Inventory, budget: u64, compute: Compute, vram: u64) -> Result<Plan, SelectError> {
    let mut smallest: Option<u64> = None;
    let mut notes: Vec<String> = vec![];

    for fam in [Family::G27, Family::G4] {
        let mut models: Vec<&GgufFile> = inv.files.iter()
            .filter(|f| !f.is_mmproj && f.family == Some(fam) && !f.text_only).collect();
        models.sort_by(|a, b| b.size.cmp(&a.size)); // biggest quant first
        if models.is_empty() { continue; }

        let mm = best_mmproj(inv, fam);
        if mm.is_none() && fam == Family::G27 {
            notes.push("MedGemma 27B weights found but its mmproj (vision) file is missing - 27B is skipped because it would not be multimodal.".into());
            continue;
        }
        let mm_bytes = mm.map(|m| m.size).unwrap_or(0);
        for m in &models {
            let required = m.size + mm_bytes + fam.overhead();
            smallest = Some(smallest.map_or(required, |s| s.min(required)));
            if required <= budget {
                let mut note = String::new();
                if mm.is_none() { note = "No 4B mmproj found: running TEXT-ONLY (images/PDF pages will be ignored).".into(); }
                let n_gpu_layers = if compute == Compute::Gpu && vram > 0 {
                    let usable = (vram as f64 * 0.85) as u64;
                    let total = m.size + mm_bytes;
                    if usable >= total + fam.overhead() { 999 } else {
                        ((fam.layers() as f64) * (usable.saturating_sub(GIB / 2) as f64 / total as f64)).floor().max(0.0) as u32
                    }
                } else { 0 };
                return Ok(Plan {
                    family: fam, family_label: fam.label().into(), model: m.path.clone(), mmproj: mm.map(|x| x.path.clone()),
                    quant: m.quant.clone(), model_bytes: m.size, mmproj_bytes: mm_bytes, required, budget,
                    multimodal: mm.is_some(), ctx: fam.ctx(), n_gpu_layers, note,
                });
            }
        }
    }
    match smallest {
        Some(s) => Err(SelectError::TooLittleRam { budget, smallest: s }),
        None => Err(SelectError::NoModels(
            if notes.is_empty() { "No MedGemma .gguf model files were found in this folder.".into() } else { notes.join(" ") })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn f(name: &str, gb: f64) -> GgufFile { classify(Path::new(name), (gb * GIB as f64) as u64).unwrap() }
    fn inv() -> Inventory {
        Inventory { files: vec![
            f("medgemma-27b-it-Q8_0.gguf", 28.7), f("medgemma-27b-it-Q5_K_M.gguf", 19.3),
            f("medgemma-27b-it-Q4_K_M.gguf", 16.5), f("medgemma-27b-it-Q3_K_M.gguf", 13.5),
            f("mmproj-medgemma-27b-it-F16.gguf", 0.9),
            f("medgemma-4b-it-Q8_0.gguf", 4.1), f("medgemma-4b-it-Q4_K_M.gguf", 2.5),
            f("mmproj-medgemma-4b-it-F16.gguf", 0.85),
        ]}
    }
    #[test] fn classifies() {
        let a = f("medgemma-27b-it-Q4_K_M.gguf", 1.0);
        assert_eq!(a.family, Some(Family::G27)); assert_eq!(a.quant, "Q4_K_M"); assert!(!a.is_mmproj);
        let b = f("medgemma-27b-text-it-Q4_K_M.gguf", 1.0); assert!(b.text_only);
        assert_eq!(f("gemma-14b.gguf", 1.0).family, None);
    }
    #[test] fn picks_largest_27b_that_fits() {
        let p = choose(&inv(), 40 * GIB, Compute::Cpu, 0).unwrap();
        assert_eq!(p.family, Family::G27); assert_eq!(p.quant, "Q8_0");
        let p = choose(&inv(), 21 * GIB, Compute::Cpu, 0).unwrap(); // 19.3+0.9+1.5 = 21.7 > 21
        assert_eq!(p.quant, "Q4_K_M");
        let p = choose(&inv(), 16 * GIB, Compute::Cpu, 0).unwrap(); // Q3_K_M: 13.5+.9+1.5=15.9
        assert_eq!(p.quant, "Q3_K_M");
    }
    #[test] fn falls_back_to_4b() {
        let p = choose(&inv(), 10 * GIB, Compute::Cpu, 0).unwrap();
        assert_eq!(p.family, Family::G4); assert_eq!(p.quant, "Q8_0"); assert!(p.multimodal);
        let p = choose(&inv(), 5 * GIB, Compute::Cpu, 0).unwrap(); // Q4_K_M needs 4.1
        assert_eq!(p.quant, "Q4_K_M");
    }
    #[test] fn too_little_ram() {
        assert!(matches!(choose(&inv(), 2 * GIB, Compute::Cpu, 0), Err(SelectError::TooLittleRam { .. })));
    }
    #[test] fn missing_27b_mmproj_is_skipped() {
        let mut i = inv(); i.files.retain(|x| !(x.is_mmproj && x.family == Some(Family::G27)));
        assert_eq!(choose(&i, 40 * GIB, Compute::Cpu, 0).unwrap().family, Family::G4);
    }
    #[test] fn empty_folder() {
        assert!(matches!(choose(&Inventory::default(), 8 * GIB, Compute::Cpu, 0), Err(SelectError::NoModels(_))));
    }
    #[test] fn gpu_layers_partial() {
        let p = choose(&inv(), 40 * GIB, Compute::Gpu, 8 * GIB).unwrap();
        assert!(p.n_gpu_layers > 0 && p.n_gpu_layers < 62);
        let p = choose(&inv(), 40 * GIB, Compute::Gpu, 48 * GIB).unwrap();
        assert_eq!(p.n_gpu_layers, 999);
    }
}
