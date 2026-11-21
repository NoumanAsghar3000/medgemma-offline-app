//! Model discovery + "largest MedGemma multimodal variant that fits" selection.
//!
//! Detection is by CONTENT: the GGUF header (architecture, layer count, embedding width, projector width, file type)
//! decides which family (27B / 4B) and which precision (8-bit / 16-bit ...) a file is. File names and folders are only a
//! hint for files whose header says nothing, so your own fine-tunes work whatever they are called and whatever size they have.
//!
//! Priority (strict), each family independent of the other:
//!   1. MedGemma 27B *multimodal* (GGUF + its own mmproj). Within a family 8-bit comes first, then 16-bit, then the
//!      other quantizations (largest first); a variant is used when its WORST-CASE speed is usable.
//!   2. MedGemma 4B multimodal (same rule).
//!   3. MedGemma 4B text-only (only if no 4B mmproj exists; flagged in the plan).
use crate::{gguf, plan, GIB};
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
    /// Where the precision label came from: "name" (a tag in the file name), "file_type" / "tensors" (the header: authoritative), "unknown".
    pub quant_source: &'static str,
    /// Header metadata read from the real file (None for fake paths in tests / unreadable files).
    pub info: Option<gguf::GgufInfo>,
    /// Set when the file is unusable (wrong architecture, mismatched vision projector ...); such files are skipped and reported.
    pub problem: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Inventory { pub files: Vec<GgufFile> }

const QUANTS: &[&str] = &[
    "IQ4_XS", "IQ4_NL", "IQ3_XXS", "IQ2_XXS", "IQ1_S", "IQ3_XS", "IQ3_M", "IQ2_M", "IQ2_S", "IQ1_M",
    "Q8_K_XL", "Q8_0", "Q6_K_XL", "Q6_K", "Q5_K_XL", "Q5_K_M", "Q5_K_S", "Q5_0", "Q4_K_XL", "Q4_K_M", "Q4_K_S", "Q4_0", "Q4_1",
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

/// Whole-word match (both sides non-alphanumeric): "text" matches "27b-text-it" but not "context".
fn has_word(lower: &str, w: &str) -> bool {
    let b = lower.as_bytes(); let mut start = 0;
    while let Some(i) = lower[start..].find(w) {
        let (st, e) = (start + i, start + i + w.len());
        if (st == 0 || !b[st - 1].is_ascii_alphanumeric()) && (e >= b.len() || !b[e].is_ascii_alphanumeric()) { return true; }
        start = st + 1;
    }
    false
}

pub fn classify(path: &Path, size: u64) -> Option<GgufFile> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".gguf") { return None; }
    let fam_of = |t: &str| if has_token(t, "27b") { Some(Family::G27) } else if has_token(t, "4b") { Some(Family::G4) } else { None };
    // Hugging Face repos name the vision file just `mmproj-F16.gguf` (no size), so when the file name has no
    // size token, the NEAREST parent folder that has one decides (e.g. D:\\Models\\27b\\mmproj-F16.gguf).
    let family = fam_of(&lower).or_else(|| path.ancestors().skip(1).filter_map(|a| a.file_name()).find_map(|n| fam_of(&n.to_string_lossy().to_ascii_lowercase())));
    let upper = name.to_ascii_uppercase().replace('-', "_").replace('.', "_");
    let quant = QUANTS.iter().find(|q| upper.contains(*q)).map(|q| q.to_string()).unwrap_or_else(|| "unknown".into());
    Some(GgufFile {
        path: path.to_path_buf(), size, family, quant,
        is_mmproj: lower.contains("mmproj"),
        text_only: has_word(&lower, "text"),
        quant_source: if QUANTS.iter().any(|q| upper.contains(*q)) { "name" } else { "unknown" },
        info: None, problem: None, warnings: vec![],
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
    enrich(&mut inv);
    Ok(inv)
}

/// Read the real headers and (1) let the CONTENT decide family / precision (names only fill in what the header cannot say),
/// (2) detect unusable files and bad pairings.
fn is_split_part(name: &str) -> bool {
    // llama.cpp split naming: <n>-00001-of-00002.gguf
    let l = name.to_ascii_lowercase();
    l.find("-of-").map(|i| i >= 6 && l.as_bytes()[i - 5..i].iter().all(|b| b.is_ascii_digit()) && l[i + 4..].chars().take(5).all(|c| c.is_ascii_digit())).unwrap_or(false)
}

fn human_bytes(b: u64) -> String {
    if b >= GIB { format!("{:.2} GB", b as f64 / GIB as f64) } else { format!("{:.1} MB", b as f64 / (1u64 << 20) as f64) }
}

pub fn enrich(inv: &mut Inventory) {
    for f in inv.files.iter_mut() {
        if is_split_part(&f.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()) && !f.is_mmproj {
            f.problem = Some("this is one part of a SPLIT GGUF (…-00001-of-0000N.gguf); split models are not supported - download the single-file quantization (for example unsloth's Q8_0, 28.7 GB)".into());
            continue;
        }
        match gguf::read_info(&f.path) {
            Ok(i) => {
                if i.arch == "clip" { f.is_mmproj = true; }
                // A file name containing the word "text" marks MedGemma's text-only release, but it is only a name: when the model's OWN
                // metadata name does not say "text", the file is the user's own (multimodal-capable) fine-tune and is NOT dropped.
                if f.text_only { if let Some(n) = &i.name { if !has_word(&n.to_ascii_lowercase(), "text") {
                    f.text_only = false;
                    f.warnings.push(format!("the file name contains 'text' but the model is named '{n}': treated as a normal (multimodal-capable) model"));
                } } }
                // CONTENT WINS over the file name / folder. 8-bit vs 16-bit: the header's file type is authoritative for the exact
                // tags (F32 / F16 / BF16 / Q8_0); other quantization tags keep the name's more specific label (e.g. Q8_K_XL).
                if let Some(q) = i.quant_name() {
                    if f.quant == "unknown" || matches!(q, "F32" | "F16" | "BF16" | "Q8_0") {
                        f.quant = q.to_string();
                        f.quant_source = if i.file_type.and_then(gguf::ftype_name).is_some() { "file_type" } else { "tensors" };
                    }
                }
                // family: real Gemma-3 geometry (62 layers / 5376 wide = 27B, 34 layers / 2560 wide = 4B; projector width for mmproj files)
                if let Some(g) = i.family_guess() {
                    if let Some(claimed) = f.family {
                        if claimed != g { f.warnings.push(format!("the file name or folder says {} but the file content is {}: the content is used", claimed.label(), g.label())); }
                    }
                    f.family = Some(g);
                }
                // INCOMPLETE copy / download: the tensor table says how big the file must be at least
                if let Some(need) = i.expected_min_size {
                    if f.size < need {
                        f.problem = Some(format!("the file is INCOMPLETE ({} on disk, its header needs at least {}): an interrupted copy or download - copy it again", human_bytes(f.size), human_bytes(need)));
                    }
                }
                f.info = Some(i);
            }
            Err(e) => { if f.size > 1 << 20 { f.problem = Some(format!("cannot read the GGUF header: {e}")); } }   // tiny/fake files: ignore
        }
    }
    // vision projectors that are not Gemma-3 projectors are unusable
    for f in inv.files.iter_mut().filter(|f| f.is_mmproj) {
        if let Some(i) = &f.info {
            if i.arch != "clip" { f.problem = Some(format!("not a vision projector (architecture '{}')", i.arch)); }
            else if i.projector_type.as_deref().map(|p| p != "gemma3").unwrap_or(false) { f.problem = Some(format!("projector type '{}' is not 'gemma3'", i.projector_type.clone().unwrap())); }
        }
    }
    // per model: architecture / layer-count / projector-dimension checks
    let snapshot = inv.clone();
    for f in inv.files.iter_mut().filter(|f| !f.is_mmproj && f.problem.is_none()) {
        let (Some(fam), Some(info)) = (f.family, f.info.clone()) else { continue };
        let mm = best_mmproj(&snapshot, fam).and_then(|m| m.info.as_ref().map(|i| (m.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), i.clone())));
        let c = gguf::check_pair(fam, &info, mm.as_ref().map(|(n, i)| (n.as_str(), i)));
        if !c.errors.is_empty() { f.problem = Some(c.errors.join("; ")); }
        f.warnings.extend(c.warnings);
        let fname = f.path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if let Some(n) = &info.name { if !n.to_ascii_lowercase().contains("medgemma") && !fname.contains("medgemma") {
            f.warnings.push(format!("the model is named '{n}', not MedGemma: accepted (fine for your own fine-tune), but it is a general Gemma 3 model if you did not train it"));
        } }
    }
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
    /// Host RAM that will be resident (weights share + overhead).
    pub required: u64,
    pub budget: u64,
    pub multimodal: bool,
    pub ctx: u32,
    /// 0 = CPU, 1..=998 = that many layers, 999 = all layers in VRAM.
    pub n_gpu_layers: u32,
    pub note: String,
    pub placement: plan::Placement,
    /// True when the user forced this variant even though the planner rated it below MIN_TPS.
    pub forced: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectError {
    /// No usable MedGemma GGUF found in the folder.
    NoModels(String),
    /// Models exist, but none reaches MIN_TPS on this machine.
    TooSlow { best_tps: f64, budget: u64 },
}
impl std::fmt::Display for SelectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectError::NoModels(s) => write!(f, "{s}"),
            SelectError::TooSlow { best_tps, budget } => write!(
                f, "No installed MedGemma variant reaches the minimum usable speed ({:.1} tokens/s) on this computer. The best estimate is {:.2} tokens/s with {:.1} GB of usable RAM. Close other programs, add RAM/VRAM, put the model on a faster drive, or add a smaller quantization (for example the 4B model) to the model folder.",
                plan::MIN_TPS, best_tps, *budget as f64 / GIB as f64),
        }
    }
}

fn best_mmproj(inv: &Inventory, fam: Family) -> Option<&GgufFile> {
    let rank = |f: &GgufFile| match f.quant.as_str() { "F16" => 0, "BF16" => 1, "Q8_0" => 2, _ => 3 };
    inv.files.iter().filter(|f| f.is_mmproj && f.family == Some(fam) && f.problem.is_none()).min_by_key(|f| rank(f))
        .or_else(|| {
            // an mmproj without a size token is shared between families only if there is exactly one
            let m: Vec<_> = inv.files.iter().filter(|f| f.is_mmproj && f.family.is_none() && f.problem.is_none()).collect();
            if m.len() == 1 { Some(m[0]) } else { None }
        })
}

/// Machine description handed to the planner.
#[derive(Debug, Clone, Copy)]
pub struct Machine { pub budget: u64, pub compute: Compute, pub vram: u64, pub bw: plan::Bandwidth }

fn build_plan(fam: Family, m: &GgufFile, mm: Option<&GgufFile>, mc: &Machine, forced: bool) -> Plan {
    let mm_bytes = mm.map(|x| x.size).unwrap_or(0);
    let gpu = mc.compute == Compute::Gpu && mc.vram > 0;
    let pl = plan::estimate(&plan::Input { model_bytes: m.size, mmproj_bytes: mm_bytes, overhead: fam.overhead(), ram_budget: mc.budget, vram: if gpu { mc.vram } else { 0 } }, &mc.bw);
    let n_gpu_layers = if !gpu { 0 } else if pl.gpu_fraction >= 1.0 { 999 } else { (fam.layers() as f64 * pl.gpu_fraction).floor() as u32 };
    let mut note = String::new();
    if mm.is_none() { note = "No vision projector (mmproj) found for this model: running TEXT-ONLY (images/PDF pages will be ignored).".into(); }
    if !m.warnings.is_empty() { if !note.is_empty() { note.push(' '); } note.push_str(&m.warnings.join("; ")); }
    Plan {
        family: fam, family_label: fam.label().into(), model: m.path.clone(), mmproj: mm.map(|x| x.path.clone()), quant: m.quant.clone(),
        model_bytes: m.size, mmproj_bytes: mm_bytes, required: pl.ram_bytes + fam.overhead(), budget: mc.budget, multimodal: mm.is_some(),
        ctx: fam.ctx(), n_gpu_layers, note, placement: pl, forced,
    }
}

/// Preference inside one family: 8-bit first (near-lossless at half the memory and twice the speed of 16-bit), then 16-bit,
/// then 32-bit, then every other quantization. Equal rank: the larger file first.
fn precision_rank(quant: &str) -> u8 {
    match quant { "Q8_0" | "Q8_K_XL" => 0, "F16" | "BF16" => 1, "F32" => 2, _ => 3 }
}

/// Every installed variant with its placement/speed estimate (for the installer text and the in-app selector).
pub fn variants(inv: &Inventory, mc: &Machine) -> Vec<Plan> {
    let mut out = vec![];
    for fam in [Family::G27, Family::G4] {
        let mm = best_mmproj(inv, fam);
        if mm.is_none() && fam == Family::G27 { continue; }
        let mut ms: Vec<&GgufFile> = inv.files.iter().filter(|f| !f.is_mmproj && f.family == Some(fam) && !f.text_only && f.problem.is_none()).collect();
        ms.sort_by(|a, b| precision_rank(&a.quant).cmp(&precision_rank(&b.quant)).then(b.size.cmp(&a.size)));
        for m in ms { out.push(build_plan(fam, m, mm, mc, false)); }
    }
    out
}

/// Automatic policy:
///   1. MedGemma 27B multimodal: the preferred precision (8-bit, then 16-bit, then the other quantizations) whose WORST-CASE
///      estimated speed is >= MIN_TPS.
///   2. else MedGemma 4B, same rule (the only fallback). Each family is independent: a folder with only one of them works.
/// The worst case (a dense model scanned cyclically through a too-small page cache gets no hits) is used so that
/// the automatic choice never gambles on optimistic cache behaviour; a user can still force any variant
/// (`plan_for`) and sees the best-worst range. Returns Err(TooSlow) when nothing qualifies.
pub fn choose(inv: &Inventory, mc: &Machine) -> Result<Plan, SelectError> {
    let all = variants(inv, mc);
    if all.is_empty() {
        let has27 = inv.files.iter().any(|f| !f.is_mmproj && f.family == Some(Family::G27) && !f.text_only);
        let problems: Vec<String> = inv.files.iter().filter_map(|f| f.problem.as_ref().map(|p| format!("{}: {}", f.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), p))).collect();
        if !problems.is_empty() { return Err(SelectError::NoModels(format!("The model files in this folder cannot be used - {}", problems.join(" | ")))); }
        let projectors: Vec<String> = inv.files.iter().filter(|f| f.is_mmproj && f.problem.is_none()).map(|f| format!("{} is a {} projector",
            f.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            f.family.map(|x| x.label().trim_start_matches("MedGemma ").to_string()).unwrap_or_else(|| "unidentified".into()))).collect();
        return Err(SelectError::NoModels(if has27 {
            format!("MedGemma 27B weights found but no vision projector (mmproj) that matches 27B was found{} - 27B is skipped because it would not be multimodal, and no 4B model is present.",
                if projectors.is_empty() { String::new() } else { format!(" (found: {})", projectors.join("; ")) })
        } else if inv.files.iter().any(|f| !f.is_mmproj && f.text_only) {
            let names: Vec<String> = inv.files.iter().filter(|f| !f.is_mmproj && f.text_only).map(|f| f.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).collect();
            format!("Only text-only model files were found ({}): they cannot read images, so they are not used. If this is your own multimodal fine-tune, rename the file so its name does not contain the word 'text'.", names.join(", "))
        } else { "No MedGemma .gguf model files were found in this folder.".into() }));
    }
    let best = all.iter().map(|p| p.placement.tps_best).fold(0.0, f64::max);   // reported in the error only
    for fam in [Family::G27, Family::G4] {
        if let Some(p) = all.iter().find(|p| p.family == fam && p.placement.tps_worst >= plan::MIN_TPS) { return Ok(p.clone()); }
    }
    Err(SelectError::TooSlow { best_tps: best, budget: mc.budget })
}

/// The user (or a saved setting) explicitly picked a file: honour it, flag it if the planner rates it too slow.
pub fn plan_for(inv: &Inventory, file_name: &str, mc: &Machine) -> Option<Plan> {
    variants(inv, mc).into_iter().find(|p| p.model.file_name().map(|n| n.to_string_lossy() == file_name).unwrap_or(false))
        .map(|mut p| { p.forced = p.placement.tps_worst < plan::MIN_TPS; p })
}


/// Readiness of one family, judged from the FILES alone (not from this PC's memory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FamilyHealth {
    /// weights + (for 27B required, for 4B optional) matching projector, no problems
    Ready,
    /// 4B weights without a projector: usable, text only
    TextOnly,
    /// 27B weights found but no projector that matches: 27B cannot be used
    NoProjector,
    /// no weights for this family in the folder
    Absent,
    /// weights exist but every one of them has a problem (incomplete, wrong architecture, unreadable ...)
    Broken,
}
impl FamilyHealth {
    pub fn label(&self) -> &'static str {
        match self { FamilyHealth::Ready => "ready", FamilyHealth::TextOnly => "text-only", FamilyHealth::NoProjector => "no-projector", FamilyHealth::Absent => "absent", FamilyHealth::Broken => "broken" }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthVerdict { Empty, Broken, Degraded, Ok }
impl HealthVerdict {
    pub fn label(&self) -> &'static str { match self { HealthVerdict::Empty => "EMPTY", HealthVerdict::Broken => "BROKEN", HealthVerdict::Degraded => "DEGRADED", HealthVerdict::Ok => "OK" } }
}

#[derive(Debug, Clone)]
pub struct Health { pub verdict: HealthVerdict, pub g27: FamilyHealth, pub g4: FamilyHealth, pub lines: Vec<String> }

fn family_health(inv: &Inventory, fam: Family) -> FamilyHealth {
    let weights: Vec<&GgufFile> = inv.files.iter().filter(|f| !f.is_mmproj && f.family == Some(fam) && !f.text_only).collect();
    if weights.is_empty() { return FamilyHealth::Absent; }
    if weights.iter().all(|f| f.problem.is_some()) { return FamilyHealth::Broken; }
    match (best_mmproj(inv, fam).is_some(), fam) {
        (true, _) => FamilyHealth::Ready,
        (false, Family::G27) => FamilyHealth::NoProjector,
        (false, Family::G4) => FamilyHealth::TextOnly,
    }
}

/// KEY=VALUE health report of a model folder (printed by `mg-probe doctor`, parsed by tools/doctor.mjs): which family works, how every
/// file was identified (header or name), and every problem. Judges the FILES only; speed/memory is the planner's job.
pub fn health(inv: &Inventory) -> Health {
    let g27 = family_health(inv, Family::G27); let g4 = family_health(inv, Family::G4);
    let mut lines = vec![];
    let mut files: Vec<&GgufFile> = inv.files.iter().collect(); files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut any_issue = false;
    for (n, f) in files.iter().enumerate() {
        let name = f.path.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        let (state, detail) = match (&f.problem, f.warnings.is_empty()) {
            (Some(p), _) => { any_issue = true; ("bad", p.clone()) }
            (None, false) => { any_issue = true; ("warning", f.warnings.join("; ")) }
            (None, true) => ("ok", String::new()),
        };
        lines.push(format!("HEALTH.file.{n}.name={name}"));
        lines.push(format!("HEALTH.file.{n}.kind={}", if f.is_mmproj { "projector" } else if f.text_only { "text-only-weights" } else { "weights" }));
        lines.push(format!("HEALTH.file.{n}.family={}", f.family.map(|x| x.label()).unwrap_or("unknown")));
        lines.push(format!("HEALTH.file.{n}.quant={}", f.quant));
        lines.push(format!("HEALTH.file.{n}.quant_source={}", f.quant_source));
        lines.push(format!("HEALTH.file.{n}.size={}", f.size));
        lines.push(format!("HEALTH.file.{n}.expected_min_size={}", f.info.as_ref().and_then(|i| i.expected_min_size).map(|x| x.to_string()).unwrap_or_else(|| "-".into())));
        lines.push(format!("HEALTH.file.{n}.header={}", if f.info.is_some() { "read" } else { "unreadable" }));
        lines.push(format!("HEALTH.file.{n}.state={state}"));
        if !detail.is_empty() { lines.push(format!("HEALTH.file.{n}.detail={detail}")); }
    }
    let usable = |h: &FamilyHealth| matches!(h, FamilyHealth::Ready | FamilyHealth::TextOnly);
    let verdict = if inv.files.is_empty() { HealthVerdict::Empty }
        else if !usable(&g27) && !usable(&g4) { HealthVerdict::Broken }
        else if any_issue || matches!(g27, FamilyHealth::NoProjector | FamilyHealth::Broken) || matches!(g4, FamilyHealth::TextOnly | FamilyHealth::Broken) { HealthVerdict::Degraded }
        else { HealthVerdict::Ok };
    lines.insert(0, format!("HEALTH.family.4B={}", g4.label()));
    lines.insert(0, format!("HEALTH.family.27B={}", g27.label()));
    lines.insert(0, format!("HEALTH.verdict={}", verdict.label()));
    lines.insert(1, format!("HEALTH.files={}", inv.files.len()));
    Health { verdict, g27, g4, lines }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Bandwidth;
    use std::path::PathBuf;
    const NVME: Bandwidth = Bandwidth { ram_gbps: 40.0, disk_mbps: 3000.0 };
    const USB: Bandwidth = Bandwidth { ram_gbps: 40.0, disk_mbps: 200.0 };
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
    fn m(budget_gb: f64, compute: Compute, vram_gb: f64, bw: Bandwidth) -> Machine {
        Machine { budget: (budget_gb * GIB as f64) as u64, compute, vram: (vram_gb * GIB as f64) as u64, bw }
    }
    #[test] fn classifies() {
        let a = f("medgemma-27b-it-Q4_K_M.gguf", 1.0);
        assert_eq!(a.family, Some(Family::G27)); assert_eq!(a.quant, "Q4_K_M"); assert!(!a.is_mmproj);
        assert!(f("medgemma-27b-text-it-Q4_K_M.gguf", 1.0).text_only);
        assert_eq!(f("gemma-14b.gguf", 1.0).family, None);
    }
    #[test] fn family_comes_from_the_parent_folder_when_the_file_name_has_no_size() {
        // unsloth / HF repos ship `mmproj-F16.gguf` for BOTH sizes -> keep them in 27b/ and 4b/ subfolders
        let a = classify(Path::new("D:/Models/27b/mmproj-F16.gguf"), 1).unwrap();
        let b = classify(Path::new("D:/Models/4b/mmproj-F16.gguf"), 1).unwrap();
        let c = classify(Path::new("D:/medgemma-27b/4b/mmproj-F16.gguf"), 1).unwrap();   // nearest folder wins
        assert_eq!((a.family, b.family, c.family), (Some(Family::G27), Some(Family::G4), Some(Family::G4)));
        assert!(a.is_mmproj && a.quant == "F16");
        let d = classify(Path::new("D:/Models/27b/medgemma-27b-it-UD-Q8_K_XL.gguf"), 1).unwrap(); assert_eq!(d.quant, "Q8_K_XL");
    }
    fn layout(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("mg_layout_{name}_{}", std::process::id())); let _ = std::fs::remove_dir_all(&root);
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
        for (dest, fixture) in files { let p = root.join(dest); std::fs::create_dir_all(p.parent().unwrap()).unwrap(); std::fs::copy(data.join(fixture), p).unwrap(); }
        root
    }
    #[test] fn the_required_models_27b_4b_layout_loads_and_prefers_27b() {
        let d = layout("ok", &[("models/27b/medgemma-27b-it-Q8_0.gguf", "hdr-27b-q8.gguf"), ("models/27b/mmproj-F16.gguf", "hdr-clip-5376.gguf"),
                               ("models/4b/medgemma-4b-it-Q4_K_M.gguf", "hdr-4b-q4km.gguf"), ("models/4b/mmproj-F16.gguf", "hdr-clip-2560.gguf")]);
        let inv = scan(&d.join("models")).unwrap();
        assert!(inv.files.iter().all(|f| f.problem.is_none()), "{:?}", inv.files.iter().map(|f| (&f.path, &f.problem)).collect::<Vec<_>>());
        let vs = variants(&inv, &m(60.0, Compute::Cpu, 0.0, NVME)); assert!(vs.iter().all(|p| p.multimodal) && vs.len() == 2);
        // the two same-named mmproj files were paired by FOLDER, and both validated by header geometry
        assert_eq!(choose(&inv, &m(60.0, Compute::Cpu, 0.0, NVME)).unwrap().family, Family::G27);
    }
    #[test] fn a_mismatched_pair_is_reported_precisely_and_selection_falls_back_to_4b() {
        // user copied the 4B mmproj into the 27b folder: the CONTENT says it is a 4B projector, so 27B has no projector of its own
        let d = layout("bad", &[("models/27b/medgemma-27b-it-Q8_0.gguf", "hdr-27b-q8.gguf"), ("models/27b/mmproj-F16.gguf", "hdr-clip-2560.gguf"),
                                ("models/4b/medgemma-4b-it-Q4_K_M.gguf", "hdr-4b-q4km.gguf"), ("models/4b/mmproj-F16.gguf", "hdr-clip-2560.gguf")]);
        let inv = scan(&d.join("models")).unwrap();
        let wrong = inv.files.iter().find(|f| f.is_mmproj && f.path.starts_with(d.join("models").join("27b"))).unwrap();
        assert_eq!(wrong.family, Some(Family::G4));
        assert!(wrong.warnings.iter().any(|w| w.contains("content")), "{:?}", wrong.warnings);
        let p = choose(&inv, &m(60.0, Compute::Cpu, 0.0, NVME)).unwrap();
        assert_eq!(p.family, Family::G4);
        assert!(variants(&inv, &m(60.0, Compute::Cpu, 0.0, NVME)).iter().all(|v| v.family == Family::G4));
        // only the 27B with the wrong projector present -> a precise error, not a crash
        let only = layout("bad2", &[("models/27b/medgemma-27b-it-Q8_0.gguf", "hdr-27b-q8.gguf"), ("models/27b/mmproj-F16.gguf", "hdr-clip-2560.gguf")]);
        let e = choose(&scan(&only.join("models")).unwrap(), &m(60.0, Compute::Cpu, 0.0, NVME)).unwrap_err().to_string();
        assert!(e.contains("27B") && e.contains("4B projector"), "{e}");
    }
    #[test] fn a_27b_file_in_the_4b_folder_and_a_plain_gemma3_are_caught() {
        let d = layout("misplaced", &[("models/4b/medgemma-4b-it-Q8_0.gguf", "hdr-27b-q8.gguf"), ("models/4b/mmproj-F16.gguf", "hdr-clip-2560.gguf")]);
        let e = choose(&scan(&d.join("models")).unwrap(), &m(60.0, Compute::Cpu, 0.0, NVME)).unwrap_err().to_string();
        assert!(e.contains("62 layers") || e.contains("27B"), "{e}");
        let g = layout("plain", &[("models/27b/gemma-3-27b-it-Q8_0.gguf", "hdr-gemma3-27b-plain.gguf"), ("models/27b/mmproj-model-f16.gguf", "hdr-clip-5376.gguf")]);
        let p = choose(&scan(&g.join("models")).unwrap(), &m(60.0, Compute::Cpu, 0.0, NVME)).unwrap();
        assert!(p.note.contains("general Gemma 3"), "{}", p.note);
    }
    #[test] fn real_fixture_files_are_validated_by_content() {
        let d = std::env::temp_dir().join(format!("mg_enrich_{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(d.join("4b")).unwrap();
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
        std::fs::copy(data.join("tiny-gemma3.gguf"), d.join("4b/medgemma-4b-it-F32.gguf")).unwrap();
        std::fs::copy(data.join("tiny-clip-64.gguf"), d.join("4b/mmproj-F16.gguf")).unwrap();
        let inv = scan(&d).unwrap();
        assert!(inv.files.iter().all(|f| f.problem.is_none()));
        let mm = inv.files.iter().find(|f| f.is_mmproj).unwrap(); assert_eq!((mm.family, mm.quant.as_str()), (Some(Family::G4), "F16"));
        assert!(variants(&inv, &m(8.0, Compute::Cpu, 0.0, NVME)).iter().all(|p| p.multimodal));
    }
    #[test] fn every_known_hugging_face_file_name_is_recognised() {
        // (path, is_mmproj, family, quant)
        let t: &[(&str, bool, Family, &str)] = &[
            ("models/27b/medgemma-27b-it-Q8_0.gguf", false, Family::G27, "Q8_0"), ("models/27b/medgemma-27b-it-Q6_K.gguf", false, Family::G27, "Q6_K"),
            ("models/27b/medgemma-27b-it-Q5_K_M.gguf", false, Family::G27, "Q5_K_M"), ("models/27b/medgemma-27b-it-Q4_K_M.gguf", false, Family::G27, "Q4_K_M"),
            ("models/27b/medgemma-27b-it-UD-Q4_K_XL.gguf", false, Family::G27, "Q4_K_XL"), ("models/27b/medgemma-27b-it-UD-Q8_K_XL.gguf", false, Family::G27, "Q8_K_XL"),
            ("models/27b/medgemma-27b-it-UD-IQ2_M.gguf", false, Family::G27, "IQ2_M"), ("models/27b/medgemma-27b-it-UD-Q2_K_XL.gguf", false, Family::G27, "Q2_K_XL"),
            ("models/27b/mmproj-F16.gguf", true, Family::G27, "F16"), ("models/27b/mmproj-BF16.gguf", true, Family::G27, "BF16"),
            ("models/27b/MedGemma-27B-IT-Q4_K_M.gguf", false, Family::G27, "Q4_K_M"), ("models/27b/mmproj-MedGemma-27B-IT-F16.gguf", true, Family::G27, "F16"),   // unofficial-jona (names from the brief)
            ("models/27b/google_medgemma-27b-it-Q8_0.gguf", false, Family::G27, "Q8_0"), ("models/27b/google_medgemma-27b-it-IQ2_M.gguf", false, Family::G27, "IQ2_M"),   // bartowski
            ("models/27b/mmproj-google_medgemma-27b-it-bf16.gguf", true, Family::G27, "BF16"),
            ("models/4b/medgemma-4b-it-Q8_0.gguf", false, Family::G4, "Q8_0"), ("models/4b/medgemma-4b-it-Q6_K.gguf", false, Family::G4, "Q6_K"),
            ("models/4b/medgemma-4b-it-Q5_K_M.gguf", false, Family::G4, "Q5_K_M"), ("models/4b/medgemma-4b-it-Q4_K_M.gguf", false, Family::G4, "Q4_K_M"),
            ("models/4b/mmproj-F16.gguf", true, Family::G4, "F16"), ("models/4b/mmproj-BF16.gguf", true, Family::G4, "BF16"),
            ("models/4b/google_medgemma-4b-it-Q4_K_M.gguf", false, Family::G4, "Q4_K_M"), ("models/4b/mmproj-google_medgemma-4b-it-f16.gguf", true, Family::G4, "F16"),   // bartowski 4B
            ("models/4b/mmproj-model-F16.gguf", true, Family::G4, "F16"), ("models/4b/medgemma-4b-it-mmproj-f16.gguf", true, Family::G4, "F16"),
            ("models/4b/mmproj-medgemma-4b-it-Q8_0.gguf", true, Family::G4, "Q8_0"), ("models/4b/medgemma-4b-it.mmproj-Q8_0.gguf", true, Family::G4, "Q8_0"),
            ("models/4b/medgemma-4b-it.F16-mmproj.gguf", true, Family::G4, "F16"),
            // kelkalot 4B 8-bit pair
            ("models/4b/medgemma-4b-it-Q8_0.gguf", false, Family::G4, "Q8_0"), ("models/4b/mmproj-medgemma-4b-it-Q8_0.gguf", true, Family::G4, "Q8_0"),
            // mradermacher 27B (static quants use `<model>.<quant>.gguf` and `<model>.mmproj-<quant>.gguf`)
            ("models/27b/medgemma-27b-multimodal.Q8_0.gguf", false, Family::G27, "Q8_0"), ("models/27b/medgemma-27b-multimodal.mmproj-Q8_0.gguf", true, Family::G27, "Q8_0"),
            ("models/27b/medgemma-27b-multimodal.mmproj-f16.gguf", true, Family::G27, "F16"), ("models/27b/medgemma-27b-multimodal.Q4_K_M.gguf", false, Family::G27, "Q4_K_M"),
            ("models/27b/medgemma-27b-multimodal.i1-Q4_K_M.gguf", false, Family::G27, "Q4_K_M"),
            // 16-bit exports (your own conversions)
            ("models/27b/medgemma-27b-it-f16.gguf", false, Family::G27, "F16"), ("models/27b/medgemma-27b-it-bf16.gguf", false, Family::G27, "BF16"),
            ("models/4b/medgemma-4b-mammography-f16.gguf", false, Family::G4, "F16"), ("models/4b/mmproj-medgemma-4b-mammography-f16.gguf", true, Family::G4, "F16"),
        ];
        for (path, mm, fam, q) in t {
            let f = classify(Path::new(path), 1).unwrap();
            assert!(f.is_mmproj == *mm && f.family == Some(*fam) && f.quant == *q && !f.text_only, "{path}: {f:?}");
        }
        assert!(classify(Path::new("models/27b/medgemma-27b-text-it-Q4_K_M.gguf"), 1).unwrap().text_only);   // text-only 27B is never chosen
        assert!(!classify(Path::new("models/27b/contextual-mammo-27b-Q8_0.gguf"), 1).unwrap().text_only);    // "context" is not "text"
    }
    #[test] fn your_own_fine_tuned_files_with_arbitrary_names_work_when_the_headers_are_right() {
        // names say nothing about size or family; folder says nothing either (flat folder): the HEADERS decide
        let d = layout("ft_flat", &[("mine/mammo-ft-q8_0.gguf", "hdr-27b-q8.gguf"), ("mine/mammo-ft-mmproj-f16.gguf", "hdr-clip-5376.gguf"),
                                    ("mine/breast-4b-bf16.gguf", "hdr-4b-q4km.gguf"), ("mine/breast-mmproj.gguf", "hdr-clip-2560.gguf")]);
        let inv = scan(&d.join("mine")).unwrap();
        let big = inv.files.iter().find(|f| f.path.ends_with("mammo-ft-q8_0.gguf")).unwrap(); assert_eq!(big.family, Some(Family::G27));
        let mm4 = inv.files.iter().find(|f| f.path.ends_with("breast-mmproj.gguf")).unwrap(); assert_eq!((mm4.is_mmproj, mm4.family), (true, Some(Family::G4)));
        assert!(inv.files.iter().all(|f| f.problem.is_none()), "{:?}", inv.files.iter().map(|f| (&f.path, &f.problem)).collect::<Vec<_>>());
        let vs = variants(&inv, &m(80.0, Compute::Cpu, 0.0, NVME));
        assert_eq!(vs.len(), 2); assert!(vs.iter().all(|p| p.multimodal));           // each model got ITS OWN projector (by projector width)
        assert!(vs.iter().any(|p| p.family == Family::G27 && p.mmproj.as_ref().unwrap().ends_with("mammo-ft-mmproj-f16.gguf")));
        assert!(vs.iter().any(|p| p.family == Family::G4 && p.mmproj.as_ref().unwrap().ends_with("breast-mmproj.gguf")));
        assert!(vs.iter().all(|p| !p.note.contains("not MedGemma")), "model names contain MedGemma, so no naming warning expected");
    }
    #[test] fn split_gguf_parts_are_rejected_with_advice() {
        assert!(is_split_part("gemma-3-27b-it-f16-00001-of-00002.gguf") && is_split_part("model-00002-of-00010.gguf"));
        assert!(!is_split_part("medgemma-27b-it-Q8_0.gguf") && !is_split_part("mmproj-F16.gguf"));
        let d = std::env::temp_dir().join(format!("mg_split_{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(d.join("27b")).unwrap();
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
        std::fs::copy(data.join("hdr-27b-q8.gguf"), d.join("27b/medgemma-27b-it-Q8_0-00001-of-00002.gguf")).unwrap();
        std::fs::copy(data.join("hdr-clip-5376.gguf"), d.join("27b/mmproj-F16.gguf")).unwrap();
        let e = choose(&scan(&d).unwrap(), &m(60.0, Compute::Cpu, 0.0, NVME)).unwrap_err().to_string();
        assert!(e.contains("SPLIT GGUF") && e.contains("single-file"), "{e}");
    }
    #[test] fn big_ram_picks_27b_8bit() {
        let p = choose(&inv(), &m(40.0, Compute::Cpu, 0.0, NVME)).unwrap();
        assert_eq!((p.family, p.quant.as_str()), (Family::G27, "Q8_0")); assert_eq!(p.placement.streamed_bytes, 0);
    }
    #[test] fn eight_bit_is_preferred_over_sixteen_bit_in_each_family_and_16bit_alone_still_works() {
        let mut i = inv(); i.files.push(f("medgemma-27b-it-F16.gguf", 54.0)); i.files.push(f("medgemma-4b-it-F16.gguf", 7.8));
        let big = m(120.0, Compute::Cpu, 0.0, NVME);
        let vs = variants(&i, &big);
        assert_eq!(vs.iter().find(|p| p.family == Family::G27).unwrap().quant, "Q8_0");
        assert_eq!(vs.iter().find(|p| p.family == Family::G4).unwrap().quant, "Q8_0");
        assert_eq!(choose(&i, &big).unwrap().quant, "Q8_0");
        assert!(vs.iter().any(|p| p.quant == "F16" && p.family == Family::G27), "16-bit stays selectable");
        let only16 = Inventory { files: vec![f("mine-27b-F16.gguf", 54.0), f("mmproj-mine-27b-F16.gguf", 0.9)] };
        let p = choose(&only16, &big).unwrap(); assert_eq!((p.family, p.quant.as_str()), (Family::G27, "F16"));
    }
    #[test] fn a_wrong_file_name_never_overrides_the_header() {
        let d = layout("swapped", &[("mix/medgemma-4b-it-Q8_0.gguf", "hdr-27b-q8.gguf"), ("mix/mmproj-medgemma-4b-it-F16.gguf", "hdr-clip-5376.gguf")]);
        let inv = scan(&d.join("mix")).unwrap();
        let model = inv.files.iter().find(|f| !f.is_mmproj).unwrap();
        assert_eq!(model.family, Some(Family::G27), "62 layers = 27B whatever the name claims");
        assert!(model.warnings.iter().any(|w| w.contains("content")), "{:?}", model.warnings);
        assert_eq!(choose(&inv, &m(80.0, Compute::Cpu, 0.0, NVME)).unwrap().family, Family::G27);
    }
    #[test] fn medium_ram_steps_down_to_the_largest_quant_that_is_still_usable() {
        // 21 GB budget: Q8/Q5 exceed RAM -> worst case re-reads everything each token (< MIN_TPS) -> Q4_K_M stays resident
        let p = choose(&inv(), &m(21.0, Compute::Cpu, 0.0, NVME)).unwrap();
        assert_eq!(p.quant, "Q4_K_M", "{:?}", p.placement);
    }
    #[test] fn the_8gb_target_falls_back_to_4b_with_fast_nvme_and_certainly_with_usb() {
        for bw in [NVME, USB] {
            let p = choose(&inv(), &m(7.0, Compute::Cpu, 0.0, bw)).unwrap();
            assert_eq!(p.family, Family::G4); assert!(p.multimodal);
        }
        let p = choose(&inv(), &m(7.0, Compute::Gpu, 8.0, NVME)).unwrap();   // 8 GB VRAM + 7 GB RAM: 27B still too slow
        assert_eq!(p.family, Family::G4);
    }
    #[test] fn forcing_27b_q8_on_the_8gb_target_is_allowed_but_flagged() {
        let p = plan_for(&inv(), "medgemma-27b-it-Q8_0.gguf", &m(7.0, Compute::Gpu, 8.0, NVME)).unwrap();
        assert!(p.forced && p.placement.verdict == plan::Verdict::BatchOnly && p.n_gpu_layers > 0 && p.n_gpu_layers < 62, "{:?}", p.placement);
    }
    #[test] fn gpu_offload_lets_27b_q4_run_with_little_ram() {
        // 10 GB RAM: 27B Q3_K_M cannot be resident on CPU, but a 12 GB GPU takes most of it
        let cpu = choose(&inv(), &m(10.0, Compute::Cpu, 0.0, NVME)).unwrap();
        let gpu = choose(&inv(), &m(10.0, Compute::Gpu, 12.0, NVME)).unwrap();
        assert_eq!(cpu.family, Family::G4); assert_eq!(gpu.family, Family::G27);
    }
    #[test] fn full_offload_reports_999_layers() {
        let p = choose(&inv(), &m(40.0, Compute::Gpu, 48.0, NVME)).unwrap(); assert_eq!(p.n_gpu_layers, 999);
    }
    #[test] fn missing_27b_mmproj_is_skipped() {
        let mut i = inv(); i.files.retain(|x| !(x.is_mmproj && x.family == Some(Family::G27)));
        assert_eq!(choose(&i, &m(40.0, Compute::Cpu, 0.0, NVME)).unwrap().family, Family::G4);
    }
    #[test] fn nothing_usable_is_an_error_not_a_silent_crawl() {
        let only27 = Inventory { files: inv().files.into_iter().filter(|x| x.family == Some(Family::G27)).collect() };
        assert!(matches!(choose(&only27, &m(7.0, Compute::Cpu, 0.0, USB)), Err(SelectError::TooSlow { .. })));
    }
    #[test] fn empty_folder() {
        assert!(matches!(choose(&Inventory::default(), &m(8.0, Compute::Cpu, 0.0, NVME)), Err(SelectError::NoModels(_))));
    }
}
