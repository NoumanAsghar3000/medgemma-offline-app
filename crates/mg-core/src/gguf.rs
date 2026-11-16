//! Minimal GGUF header reader + MedGemma pairing checks.
//! Real Hugging Face files are validated by their CONTENT (architecture, layer count, embedding size, projector
//! type/dimension), not only by file name, so a wrong/misnamed/mismatched download is reported precisely instead
//! of crashing llama-server. Only the metadata section is read (a few MB), never the weights.
use crate::select::Family;
use std::{
    fs::File,
    io::{BufReader, Read, Seek},
    path::Path,
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GgufInfo {
    pub version: u32,
    pub arch: String,
    pub name: Option<String>,
    pub block_count: Option<u64>,
    pub embedding_length: Option<u64>,
    pub file_type: Option<u32>,
    pub projector_type: Option<String>,
    pub projection_dim: Option<u64>,
}

/// llama.cpp `llama_ftype` values -> the quant tags used in file names.
pub fn ftype_name(t: u32) -> Option<&'static str> {
    Some(match t {
        0 => "F32", 1 => "F16", 2 => "Q4_0", 3 => "Q4_1", 7 => "Q8_0", 8 => "Q5_0", 9 => "Q5_1", 10 => "Q2_K", 11 => "Q3_K_S",
        12 => "Q3_K_M", 13 => "Q3_K_L", 14 => "Q4_K_S", 15 => "Q4_K_M", 16 => "Q5_K_S", 17 => "Q5_K_M", 18 => "Q6_K", 19 => "IQ2_XXS",
        23 => "IQ3_XXS", 25 => "IQ4_NL", 30 => "IQ4_XS", 32 => "BF16", _ => return None,
    })
}

impl GgufInfo {
    pub fn quant_name(&self) -> Option<&'static str> { self.file_type.and_then(ftype_name) }
    /// Gemma 3 geometry: 27B = 62 layers / 5376 wide, 4B = 34 layers / 2560 wide.
    pub fn family_guess(&self) -> Option<Family> {
        let by_dim = |d: u64| match d { 5376 => Some(Family::G27), 2560 => Some(Family::G4), _ => None };
        match (self.block_count, self.embedding_length, self.projection_dim) {
            (Some(62), _, _) => Some(Family::G27),
            (Some(34), _, _) => Some(Family::G4),
            (_, Some(e), _) => by_dim(e),
            (_, _, Some(p)) => by_dim(p),
            _ => None,
        }
    }
}

fn rd<const N: usize>(r: &mut impl Read) -> Result<[u8; N], String> { let mut b = [0u8; N]; r.read_exact(&mut b).map_err(|e| e.to_string())?; Ok(b) }
fn u32_(r: &mut impl Read) -> Result<u32, String> { Ok(u32::from_le_bytes(rd(r)?)) }
fn u64_(r: &mut impl Read) -> Result<u64, String> { Ok(u64::from_le_bytes(rd(r)?)) }
fn string(r: &mut impl Read, max: u64) -> Result<String, String> {
    let n = u64_(r)?; if n > max { return Err(format!("implausible string length {n}")); }
    let mut b = vec![0u8; n as usize]; r.read_exact(&mut b).map_err(|e| e.to_string())?; Ok(String::from_utf8_lossy(&b).to_string())
}
fn scalar_size(t: u32) -> Option<u64> { Some(match t { 0 | 1 | 7 => 1, 2 | 3 => 2, 4 | 5 | 6 => 4, 10 | 11 | 12 => 8, _ => return None }) }

/// Read a scalar value as u64 (None for non-integer types) while consuming it.
fn read_scalar_u64(r: &mut impl Read, t: u32) -> Result<Option<u64>, String> {
    Ok(match t {
        0 => Some(rd::<1>(r)?[0] as u64), 1 => Some(rd::<1>(r)?[0] as u64), 2 => Some(u16::from_le_bytes(rd(r)?) as u64), 3 => Some(u16::from_le_bytes(rd(r)?) as u64),
        4 => Some(u32_(r)? as u64), 5 => Some(u32_(r)? as u64), 10 | 11 => Some(u64_(r)?),
        6 => { rd::<4>(r)?; None } 12 => { rd::<8>(r)?; None } 7 => Some(rd::<1>(r)?[0] as u64),
        _ => return Err(format!("unsupported value type {t}")),
    })
}

fn skip_value(r: &mut (impl Read + Seek), t: u32) -> Result<(), String> {
    match t {
        8 => { let n = u64_(r)?; r.seek_relative(n as i64).map_err(|e| e.to_string()) }
        9 => {
            let et = u32_(r)?; let n = u64_(r)?;
            if let Some(sz) = scalar_size(et) { r.seek_relative((n * sz) as i64).map_err(|e| e.to_string()) }
            else if et == 8 { for _ in 0..n { let l = u64_(r)?; r.seek_relative(l as i64).map_err(|e| e.to_string())?; } Ok(()) }
            else { Err("nested arrays are not supported".into()) }
        }
        _ => scalar_size(t).map(|s| r.seek_relative(s as i64).map_err(|e| e.to_string())).unwrap_or_else(|| Err(format!("unsupported value type {t}"))),
    }
}

pub fn read_info(path: &Path) -> Result<GgufInfo, String> {
    let f = File::open(path).map_err(|e| e.to_string())?;
    let mut r = BufReader::with_capacity(256 * 1024, f);
    if &rd::<4>(&mut r)? != b"GGUF" { return Err("not a GGUF file (bad magic)".into()); }
    let mut info = GgufInfo { version: u32_(&mut r)?, ..Default::default() };
    if !(2..=3).contains(&info.version) { return Err(format!("unsupported GGUF version {}", info.version)); }
    let _tensors = u64_(&mut r)?; let kvs = u64_(&mut r)?;
    if kvs > 100_000 { return Err("implausible metadata size".into()); }
    for _ in 0..kvs {
        let key = string(&mut r, 1 << 16)?; let t = u32_(&mut r)?;
        let want_u = key == "general.file_type" || key.ends_with(".block_count") || key.ends_with(".embedding_length") || key == "clip.vision.projection_dim";
        match (key.as_str(), t) {
            ("general.architecture", 8) => info.arch = string(&mut r, 1 << 16)?,
            ("general.name", 8) => info.name = Some(string(&mut r, 1 << 16)?),
            ("clip.projector_type", 8) => info.projector_type = Some(string(&mut r, 1 << 16)?),
            _ if want_u && t != 8 && t != 9 => {
                let v = read_scalar_u64(&mut r, t)?;
                match key.as_str() {
                    "general.file_type" => info.file_type = v.map(|x| x as u32),
                    "clip.vision.projection_dim" => info.projection_dim = v,
                    k if k.ends_with(".block_count") && info.block_count.is_none() => info.block_count = v,
                    k if k.ends_with(".embedding_length") && info.embedding_length.is_none() && !k.starts_with("clip.") => info.embedding_length = v,
                    _ => {}
                }
            }
            _ => skip_value(&mut r, t)?,
        }
    }
    if info.arch.is_empty() { return Err("GGUF has no general.architecture".into()); }
    Ok(info)
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Check { pub errors: Vec<String>, pub warnings: Vec<String> }

/// `family` is what the file name / folder claims; `model`/`mmproj` are what the files really are.
pub fn check_pair(family: Family, model: &GgufInfo, mmproj: Option<(&str, &GgufInfo)>) -> Check {
    let mut c = Check::default();
    if model.arch != "gemma3" {
        c.errors.push(format!("this is not a Gemma 3 model (architecture '{}'); MedGemma uses 'gemma3'", model.arch));
        return c;
    }
    match (family, model.block_count) {
        (Family::G27, Some(34)) => c.errors.push("the name says 27B but the file has 34 layers = the 4B model (check the folder / file name)".into()),
        (Family::G4, Some(62)) => c.errors.push("the name says 4B but the file has 62 layers = the 27B model (check the folder / file name)".into()),
        (_, Some(n)) if n != 62 && n != 34 => c.warnings.push(format!("unusual layer count {n} (a fine-tune or a test model?)")),
        _ => {}
    }
    if let Some((mname, m)) = mmproj {
        if m.arch != "clip" { c.errors.push(format!("{mname} is not a vision projector (architecture '{}')", m.arch)); }
        else {
            if let Some(p) = &m.projector_type { if p != "gemma3" { c.errors.push(format!("{mname} is a '{p}' projector, MedGemma needs 'gemma3'")); } }
            if let (Some(pd), Some(e)) = (m.projection_dim, model.embedding_length) {
                if pd != e {
                    let known = |d: u64| d == 5376 || d == 2560;       // Gemma 3 27B / 4B widths
                    let msg = format!("{mname} was made for a model with embedding size {pd}, but this model has {e}: use the matching mmproj (27B: 5376, 4B: 2560)");
                    // Hard error only when BOTH sides are real Gemma-3 widths (the classic 4B-projector-with-27B-model mistake);
                    // otherwise (fine-tune, test model, different key semantics) warn and let llama-server have the final word.
                    if known(pd) && known(e) { c.errors.push(msg); } else { c.warnings.push(msg); }
                }
            }
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    fn data(n: &str) -> std::path::PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(n) }
    #[test] fn reads_a_real_gguf_written_by_gguf_py() {
        let i = read_info(&data("tiny-gemma3.gguf")).unwrap();
        assert_eq!((i.arch.as_str(), i.block_count, i.embedding_length, i.version), ("gemma3", Some(2), Some(64), 3));
        assert_eq!(i.name.as_deref(), Some("tiny-gemma3-random"));
        assert_eq!(i.file_type, Some(0)); assert_eq!(i.quant_name(), Some("F32"));
    }
    #[test] fn reads_a_projector() {
        let i = read_info(&data("tiny-clip-64.gguf")).unwrap();
        assert_eq!((i.arch.as_str(), i.projector_type.as_deref(), i.projection_dim, i.file_type), ("clip", Some("gemma3"), Some(64), Some(1)));
    }
    #[test] fn rejects_non_gguf_and_truncated_files() {
        let p = std::env::temp_dir().join("mg_notgguf.bin"); std::fs::write(&p, b"hello world, not gguf").unwrap();
        assert!(read_info(&p).unwrap_err().contains("bad magic"));
        let full = std::fs::read(data("tiny-gemma3.gguf")).unwrap(); let q = std::env::temp_dir().join("mg_trunc.gguf"); std::fs::write(&q, &full[..300]).unwrap();
        assert!(read_info(&q).is_err());
    }
    #[test] fn pairings_with_real_gemma3_geometry() {
        let m27 = read_info(&data("hdr-27b-q8.gguf")).unwrap(); let m4 = read_info(&data("hdr-4b-q4km.gguf")).unwrap();
        let c27 = read_info(&data("hdr-clip-5376.gguf")).unwrap(); let c4 = read_info(&data("hdr-clip-2560.gguf")).unwrap();
        assert_eq!((m27.block_count, m27.embedding_length, m27.quant_name()), (Some(62), Some(5376), Some("Q8_0")));
        assert_eq!((m4.block_count, m4.embedding_length, m4.quant_name()), (Some(34), Some(2560), Some("Q4_K_M")));
        assert!(check_pair(Family::G27, &m27, Some(("mmproj-F16.gguf", &c27))).errors.is_empty());
        assert!(check_pair(Family::G4, &m4, Some(("mmproj-F16.gguf", &c4))).errors.is_empty());
        let bad = check_pair(Family::G27, &m27, Some(("mmproj-F16.gguf", &c4)));         // 4B projector with the 27B model
        assert!(bad.errors.len() == 1 && bad.errors[0].contains("2560") && bad.errors[0].contains("5376"), "{bad:?}");
        assert!(check_pair(Family::G4, &m4, Some(("mmproj-F16.gguf", &c27))).errors[0].contains("5376"));
        assert!(check_pair(Family::G4, &m27, None).errors[0].contains("62 layers"));          // 27B file placed in the 4B folder
    }
    #[test] fn odd_dimensions_only_warn_and_non_projectors_are_errors() {
        let m = read_info(&data("tiny-gemma3.gguf")).unwrap(); let bad = read_info(&data("tiny-clip-128.gguf")).unwrap();   // tiny test geometry
        let c = check_pair(Family::G4, &m, Some(("mmproj-F16.gguf", &bad))); assert!(c.errors.is_empty() && c.warnings.iter().any(|w| w.contains("128")), "{c:?}");
        assert!(check_pair(Family::G4, &m, Some(("m.gguf", &m))).errors[0].contains("not a vision projector"));
        let other = GgufInfo { arch: "clip".into(), projector_type: Some("qwen2vl".into()), projection_dim: Some(3584), ..Default::default() };
        assert!(check_pair(Family::G4, &m, Some(("mmproj-qwen.gguf", &other))).errors[0].contains("qwen2vl"));
    }
    #[test] fn wrong_family_by_layer_count_is_an_error_but_unusual_counts_only_warn() {
        let mut m = GgufInfo { arch: "gemma3".into(), block_count: Some(34), ..Default::default() };
        assert!(check_pair(Family::G27, &m, None).errors[0].contains("4B"));
        m.block_count = Some(62); assert!(check_pair(Family::G4, &m, None).errors[0].contains("27B"));
        m.block_count = Some(2); let c = check_pair(Family::G4, &m, None); assert!(c.errors.is_empty() && c.warnings.len() == 1);
        m.arch = "llama".into(); assert!(check_pair(Family::G4, &m, None).errors[0].contains("not a Gemma 3"));
    }
    #[test] fn family_guess_from_real_geometry() {
        let g = |b, e| GgufInfo { block_count: Some(b), embedding_length: Some(e), ..Default::default() };
        assert_eq!(g(62, 5376).family_guess(), Some(Family::G27)); assert_eq!(g(34, 2560).family_guess(), Some(Family::G4)); assert_eq!(g(2, 64).family_guess(), None);
        assert_eq!(GgufInfo { projection_dim: Some(5376), ..Default::default() }.family_guess(), Some(Family::G27));
    }
}
