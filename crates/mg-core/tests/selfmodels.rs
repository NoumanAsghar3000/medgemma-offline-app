//! Self-trained MedGemma 27B and 4B, each 8-bit and 16-bit: detection by FILE CONTENT, independence of the two families,
//! 8-bit preference, GPU planning through the real `prepare_with`, and refusal of files that are not Gemma 3.
//! The GGUF files are synthetic HEADERS written here (the app only ever reads the metadata section), so every rule is
//! exercised with exactly the numbers a real 27B / 4B file carries: 62 layers / 5376 wide and 34 layers / 2560 wide.
use mg_core::{
    backend::prepare_with,
    config::Config,
    hw::Hardware,
    plan::Bandwidth,
    select::*,
    GIB,
};
use std::{fs, path::{Path, PathBuf}};

const NVME: Bandwidth = Bandwidth { ram_gbps: 40.0, disk_mbps: 3000.0 };

#[derive(Clone, Copy)]
enum Kind { Model { layers: u32, width: u32, ftype: u32 }, Projector { dim: u32 } }
type Item = (&'static str, Kind, f64);

// llama_ftype: 7 = 8-bit (Q8_0), 1 = 16-bit float (F16)
const A8: Item = ("alpha-final.gguf", Kind::Model { layers: 62, width: 5376, ftype: 7 }, 27.2);
const A16: Item = ("alpha-full-precision.gguf", Kind::Model { layers: 62, width: 5376, ftype: 1 }, 51.0);
const AV: Item = ("alpha-vision.gguf", Kind::Projector { dim: 5376 }, 0.9);
const B8: Item = ("beta-final.gguf", Kind::Model { layers: 34, width: 2560, ftype: 7 }, 4.1);
const B16: Item = ("beta-full.gguf", Kind::Model { layers: 34, width: 2560, ftype: 1 }, 7.8);
const BV: Item = ("beta-vision.gguf", Kind::Projector { dim: 2560 }, 0.85);

enum V<'a> { S(&'a str), U(u32) }

fn write_gguf(path: &Path, kvs: &[(&str, V)]) { write_gguf_t(path, kvs, &[]); }

/// `tensors`: (name, dims, ggml type id). Written as a real GGUF tensor table after the metadata.
fn write_gguf_t(path: &Path, kvs: &[(&str, V)], tensors: &[(&str, &[u64], u32)]) {
    let mut b: Vec<u8> = b"GGUF".to_vec();
    b.extend(3u32.to_le_bytes()); b.extend((tensors.len() as u64).to_le_bytes()); b.extend((kvs.len() as u64).to_le_bytes());
    for (k, v) in kvs {
        b.extend((k.len() as u64).to_le_bytes()); b.extend(k.as_bytes());
        match v {
            V::S(s) => { b.extend(8u32.to_le_bytes()); b.extend((s.len() as u64).to_le_bytes()); b.extend(s.as_bytes()); }
            V::U(n) => { b.extend(4u32.to_le_bytes()); b.extend(n.to_le_bytes()); }
        }
    }
    for (name, dims, ty) in tensors {
        b.extend((name.len() as u64).to_le_bytes()); b.extend(name.as_bytes());
        b.extend((dims.len() as u32).to_le_bytes()); for d in *dims { b.extend(d.to_le_bytes()); }
        b.extend(ty.to_le_bytes()); b.extend(0u64.to_le_bytes());
    }
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b).unwrap();
}

fn write_item(dir: &Path, name: &str, kind: Kind) {
    let p = dir.join(name);
    match kind {
        Kind::Model { layers, width, ftype } => write_gguf(&p, &[
            ("general.architecture", V::S("gemma3")), ("general.name", V::S("medgemma-selftrained")),
            ("gemma3.block_count", V::U(layers)), ("gemma3.embedding_length", V::U(width)), ("general.file_type", V::U(ftype)),
        ]),
        Kind::Projector { dim } => write_gguf(&p, &[
            ("general.architecture", V::S("clip")), ("clip.projector_type", V::S("gemma3")),
            ("clip.vision.projection_dim", V::U(dim)), ("general.file_type", V::U(1)),
        ]),
    }
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mg_self_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d); fs::create_dir_all(&d).unwrap();
    d
}

/// Writes the headers into a fresh folder and returns the scanned inventory with realistic file sizes applied.
fn folder(name: &str, items: &[Item]) -> (PathBuf, Inventory) {
    let d = tmp(name);
    for (n, k, _) in items { write_item(&d, n, *k); }
    let mut inv = scan(&d).unwrap();
    for f in inv.files.iter_mut() {
        let n = f.path.file_name().unwrap().to_string_lossy().to_string();
        if let Some((_, _, gb)) = items.iter().find(|(x, _, _)| *x == n) { f.size = (gb * GIB as f64) as u64; }
    }
    (d, inv)
}

fn machine(budget_gb: f64, compute: Compute, vram_gb: f64) -> Machine {
    Machine { budget: (budget_gb * GIB as f64) as u64, compute, vram: (vram_gb * GIB as f64) as u64, bw: NVME }
}
fn cpu(budget_gb: f64) -> Machine { machine(budget_gb, Compute::Cpu, 0.0) }
fn file_name(p: &Plan) -> String { p.model.file_name().unwrap().to_string_lossy().to_string() }

#[test]
fn all_six_files_are_recognised_by_content_in_a_flat_folder_with_meaningless_names() {
    let (_, inv) = folder("flat", &[A8, A16, AV, B8, B16, BV]);
    assert!(inv.files.iter().all(|f| f.problem.is_none()), "{:?}", inv.files.iter().map(|f| (&f.path, &f.problem)).collect::<Vec<_>>());
    let get = |n: &str| inv.files.iter().find(|f| f.path.ends_with(n)).unwrap().clone();
    for (n, fam, q) in [("alpha-final.gguf", Family::G27, "Q8_0"), ("alpha-full-precision.gguf", Family::G27, "F16"),
                        ("beta-final.gguf", Family::G4, "Q8_0"), ("beta-full.gguf", Family::G4, "F16")] {
        let f = get(n); assert!(!f.is_mmproj, "{n}"); assert_eq!((f.family, f.quant.as_str()), (Some(fam), q), "{n}");
    }
    let (av, bv) = (get("alpha-vision.gguf"), get("beta-vision.gguf"));
    assert!(av.is_mmproj && bv.is_mmproj, "projectors are found by architecture 'clip', not by a 'mmproj' file name");
    assert_eq!((av.family, bv.family), (Some(Family::G27), Some(Family::G4)));
}

#[test]
fn both_present_27b_8bit_wins_when_it_fits_and_4b_takes_over_when_it_does_not() {
    let (_, inv) = folder("both", &[A8, A16, AV, B8, B16, BV]);
    // plenty of RAM: 27B, and the 8-bit file even though the (larger) 16-bit file would also fit
    let p = choose(&inv, &cpu(120.0)).unwrap();
    assert_eq!((p.family, p.quant.as_str(), file_name(&p).as_str()), (Family::G27, "Q8_0", "alpha-final.gguf"));
    assert!(p.multimodal && p.mmproj.as_ref().unwrap().ends_with("alpha-vision.gguf"), "27B must use ITS OWN projector");
    // 40 GB usable: 27B 8-bit still resident
    assert_eq!(choose(&inv, &cpu(40.0)).unwrap().family, Family::G27);
    // 12 GB usable: 27B would be re-read from the drive for every token -> automatic fallback to 4B 8-bit
    let q = choose(&inv, &cpu(12.0)).unwrap();
    assert_eq!((q.family, q.quant.as_str(), file_name(&q).as_str()), (Family::G4, "Q8_0", "beta-final.gguf"));
    assert!(q.multimodal && q.mmproj.as_ref().unwrap().ends_with("beta-vision.gguf"), "4B must use ITS OWN projector");
}

#[test]
fn each_model_works_completely_on_its_own() {
    // only the 27B files exist
    let (_, only27) = folder("only27", &[A8, A16, AV]);
    let p = choose(&only27, &cpu(40.0)).unwrap(); assert_eq!((p.family, p.quant.as_str()), (Family::G27, "Q8_0"));
    // only the 4B files exist
    let (_, only4) = folder("only4", &[B8, B16, BV]);
    let p = choose(&only4, &cpu(12.0)).unwrap(); assert_eq!((p.family, p.quant.as_str()), (Family::G4, "Q8_0"));
    // the 4B model does not need the 27B files and vice versa: removing one family's projector only removes that family
    let (_, no27v) = folder("no27v", &[A8, B8, BV]);
    assert_eq!(choose(&no27v, &cpu(120.0)).unwrap().family, Family::G4);
    let (_, no4v) = folder("no4v", &[A8, AV, B8]);
    assert_eq!(choose(&no4v, &cpu(120.0)).unwrap().family, Family::G27);
}

#[test]
fn sixteen_bit_is_used_when_it_is_the_only_precision_and_stays_selectable_otherwise() {
    let (_, a16) = folder("a16", &[A16, AV]);
    let p = choose(&a16, &cpu(60.0)).unwrap(); assert_eq!((p.family, p.quant.as_str()), (Family::G27, "F16"));
    // 50 GB usable: 51 GB of 16-bit weights cannot stay resident -> too slow, and there is no 4B to fall back to: a precise error
    assert!(matches!(choose(&a16, &cpu(50.0)), Err(SelectError::TooSlow { .. })));
    let (_, b16) = folder("b16", &[B16, BV]);
    let p = choose(&b16, &cpu(12.0)).unwrap(); assert_eq!((p.family, p.quant.as_str()), (Family::G4, "F16"));
    // with both precisions installed the user can still pick 16-bit by hand, and it is not flagged slow on a big machine
    let (_, all) = folder("all", &[A8, A16, AV, B8, B16, BV]);
    let f = plan_for(&all, "alpha-full-precision.gguf", &cpu(120.0)).unwrap();
    assert_eq!((f.family, f.quant.as_str(), f.forced), (Family::G27, "F16", false));
    assert_eq!(variants(&all, &cpu(120.0)).len(), 4);
}

#[test]
fn content_beats_misleading_names_and_precision_labels() {
    // names are SWAPPED on purpose: the 27B file is called 4b, the 4B file is called 27b, and a 16-bit file claims Q4_K_M
    let d = tmp("swapped");
    write_item(&d, "medgemma-4b-it-Q8_0.gguf", Kind::Model { layers: 62, width: 5376, ftype: 7 });
    write_item(&d, "mmproj-medgemma-4b-it-F16.gguf", Kind::Projector { dim: 5376 });
    write_item(&d, "medgemma-27b-it-Q4_K_M.gguf", Kind::Model { layers: 34, width: 2560, ftype: 1 });
    write_item(&d, "mmproj-medgemma-27b-it-F16.gguf", Kind::Projector { dim: 2560 });
    let mut inv = scan(&d).unwrap();
    for f in inv.files.iter_mut() {
        let n = f.path.file_name().unwrap().to_string_lossy().to_string();
        f.size = ((if n.contains("mmproj") { 0.9 } else if n.contains("4b-it-Q8") { 27.2 } else { 7.8 }) * GIB as f64) as u64;
    }
    assert!(inv.files.iter().all(|f| f.problem.is_none()), "{:?}", inv.files.iter().map(|f| (&f.path, &f.problem)).collect::<Vec<_>>());
    let big = choose(&inv, &cpu(120.0)).unwrap();
    assert_eq!(big.family, Family::G27, "62 layers / 5376 wide is the 27B whatever the file is called");
    assert!(big.model.ends_with("medgemma-4b-it-Q8_0.gguf") && big.mmproj.as_ref().unwrap().ends_with("mmproj-medgemma-4b-it-F16.gguf"));
    assert!(big.note.contains("content"), "the user is told the name was wrong: {}", big.note);
    let small = choose(&inv, &cpu(12.0)).unwrap();
    assert_eq!((small.family, small.quant.as_str()), (Family::G4, "F16"), "header file type F16 beats the 'Q4_K_M' in the name");
    assert!(small.model.ends_with("medgemma-27b-it-Q4_K_M.gguf"));
}

#[test]
fn files_that_are_not_gemma3_are_refused_precisely() {
    let d = tmp("foreign");
    write_gguf(&d.join("mystery.gguf"), &[("general.architecture", V::S("llama")), ("llama.block_count", V::U(62)), ("llama.embedding_length", V::U(5376)), ("general.file_type", V::U(7))]);
    let e = choose(&scan(&d).unwrap(), &cpu(120.0)).unwrap_err().to_string();
    assert!(e.contains("cannot be used") && e.contains("not a Gemma 3"), "{e}");
    // a projector of another family is not accepted as a MedGemma projector
    let d2 = tmp("qwen");
    write_item(&d2, A8.0, A8.1);
    write_gguf(&d2.join("qwen-vision.gguf"), &[("general.architecture", V::S("clip")), ("clip.projector_type", V::S("qwen2vl")), ("clip.vision.projection_dim", V::U(3584))]);
    let e = choose(&scan(&d2).unwrap(), &cpu(120.0)).unwrap_err().to_string();
    assert!(e.contains("qwen2vl"), "{e}");
}

/// A converter (convert_hf_to_gguf --outtype f16 / q8_0 / bf16, or a custom export) may leave `general.file_type` out. The tensor table
/// still says what the weights are, so 8-bit and 16-bit are told apart by CONTENT, whatever the file is called.
#[test]
fn precision_is_read_from_the_tensor_table_when_the_header_has_no_file_type() {
    let d = tmp("tensors");
    let kv = |layers: u32, width: u32| vec![("general.architecture", V::S("gemma3")), ("general.name", V::S("medgemma-selftrained")),
                                             ("gemma3.block_count", V::U(layers)), ("gemma3.embedding_length", V::U(width))];
    // (file, layers, width, type of the big matrices): 1 = F16, 8 = Q8_0, 30 = BF16
    let cases: [(&str, u32, u32, u32, &str); 4] = [
        ("run7-sixteen.gguf", 62, 5376, 1, "F16"), ("run7-eight.gguf", 62, 5376, 8, "Q8_0"),
        ("run9-small-sixteen.gguf", 34, 2560, 1, "F16"), ("run9-small-brain.gguf", 34, 2560, 30, "BF16"),
    ];
    for (name, layers, width, ty, _) in cases {
        // 1-D norm weights are F32 (type 0) in every real file; they must not decide the precision
        // small tensors (the file is "complete" once it is padded to the size the table implies)
        write_gguf_t(&d.join(name), &kv(layers, width), &[("output_norm.weight", &[256], 0), ("blk.0.ffn_up.weight", &[256, 512], ty), ("token_embd.weight", &[256, 1024], ty)]);
    }
    for (name, ..) in cases { fs::OpenOptions::new().write(true).open(d.join(name)).unwrap().set_len(2 << 20).unwrap(); }   // tensor data present
    let inv = scan(&d).unwrap();
    for (name, layers, _, _, want) in cases {
        let f = inv.files.iter().find(|f| f.path.ends_with(name)).unwrap();
        assert_eq!(f.quant, want, "{name}");
        assert_eq!(f.family, Some(if layers == 62 { Family::G27 } else { Family::G4 }), "{name}");
        assert!(f.problem.is_none(), "{name}: {:?}", f.problem);
    }
}

#[test]
fn a_damaged_or_missing_tensor_table_never_hides_a_model() {
    let d = tmp("badtensors");
    // tensor_count says 5 but the file ends right after the metadata (header-only export)
    let mut b: Vec<u8> = b"GGUF".to_vec();
    b.extend(3u32.to_le_bytes()); b.extend(5u64.to_le_bytes()); b.extend(4u64.to_le_bytes());
    for (k, v) in [("general.architecture", V::S("gemma3")), ("general.name", V::S("medgemma-selftrained")), ("gemma3.block_count", V::U(62)), ("gemma3.embedding_length", V::U(5376))] {
        b.extend((k.len() as u64).to_le_bytes()); b.extend(k.as_bytes());
        match v { V::S(s) => { b.extend(8u32.to_le_bytes()); b.extend((s.len() as u64).to_le_bytes()); b.extend(s.as_bytes()); } V::U(n) => { b.extend(4u32.to_le_bytes()); b.extend(n.to_le_bytes()); } }
    }
    fs::write(d.join("export.gguf"), b).unwrap();
    let inv = scan(&d).unwrap();
    let f = &inv.files[0];
    assert_eq!((f.family, f.problem.is_none(), f.quant.as_str()), (Some(Family::G27), true, "unknown"));
}

/// The word "text" in a FILE NAME marks MedGemma's text-only release, but a fine-tune may be called anything: the model's own metadata
/// name decides when it is there.
#[test]
fn a_file_name_with_the_word_text_does_not_hide_your_own_multimodal_fine_tune() {
    let build = |dir: &str, general_name: &str| {
        let d = tmp(dir);
        for (n, k) in [("mammo-text-reports-27b-final.gguf", Kind::Model { layers: 62, width: 5376, ftype: 7 }), ("vision-final.gguf", Kind::Projector { dim: 5376 })] { write_item(&d, n, k); }
        write_gguf(&d.join("mammo-text-reports-27b-final.gguf"), &[("general.architecture", V::S("gemma3")), ("general.name", V::S(general_name)),
            ("gemma3.block_count", V::U(62)), ("gemma3.embedding_length", V::U(5376)), ("general.file_type", V::U(7))]);
        scan(&d).unwrap()
    };
    // own fine-tune: metadata name has no "text" -> used
    let own = build("txt_own", "medgemma-mammo-ft");
    let p = choose(&own, &cpu(120.0)).unwrap();
    assert_eq!(p.family, Family::G27); assert!(p.note.contains("'text'"), "{}", p.note);
    // the real text-only release names itself "text" in its metadata -> still excluded, with a precise message
    let text_only = build("txt_real", "medgemma-27b-text-it");
    let e = choose(&text_only, &cpu(120.0)).unwrap_err().to_string();
    assert!(e.contains("text-only") && e.contains("rename"), "{e}");
}

/// The tensor table implies a minimum file size. A model copied or downloaded only partly is refused with a precise message; the same
/// file, complete, is accepted.
#[test]
fn an_incomplete_copy_is_refused_precisely_and_the_complete_file_is_accepted() {
    let d = tmp("incomplete");
    let kv = vec![("general.architecture", V::S("gemma3")), ("general.name", V::S("medgemma-selftrained")), ("gemma3.block_count", V::U(34)), ("gemma3.embedding_length", V::U(2560)), ("general.file_type", V::U(1))];
    write_gguf_t(&d.join("beta-full.gguf"), &kv, &[("blk.0.ffn_up.weight", &[4096, 4096], 1)]);      // 4096*4096*2 = 32 MiB of F16 data
    write_item(&d, "beta-vision.gguf", Kind::Projector { dim: 2560 });
    let inv = scan(&d).unwrap();
    let f = inv.files.iter().find(|f| f.path.ends_with("beta-full.gguf")).unwrap();
    assert!(f.problem.as_deref().unwrap_or("").contains("INCOMPLETE"), "{:?}", f.problem);
    let e = choose(&inv, &cpu(60.0)).unwrap_err().to_string();
    assert!(e.contains("cannot be used") && e.contains("INCOMPLETE") && e.contains("copy it again"), "{e}");
    fs::OpenOptions::new().write(true).open(d.join("beta-full.gguf")).unwrap().set_len(40 << 20).unwrap();   // now the data is there
    let ok = scan(&d).unwrap();
    assert!(ok.files.iter().all(|f| f.problem.is_none()), "{:?}", ok.files.iter().map(|f| &f.problem).collect::<Vec<_>>());
    assert_eq!(choose(&ok, &cpu(60.0)).unwrap().family, Family::G4);
}

fn raw_header(version: u32, tensors: u64, kvs: u64, body: &[u8]) -> Vec<u8> {
    let mut b: Vec<u8> = b"GGUF".to_vec();
    b.extend(version.to_le_bytes()); b.extend(tensors.to_le_bytes()); b.extend(kvs.to_le_bytes()); b.extend(body);
    b
}
fn kv_key(k: &str) -> Vec<u8> { let mut b = (k.len() as u64).to_le_bytes().to_vec(); b.extend(k.as_bytes()); b }

/// Runs `f` on a thread and fails (instead of hanging the whole test run) when it does not finish: a corrupt header must never loop.
fn finishes_within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let _ = tx.send(f()); });
    rx.recv_timeout(std::time::Duration::from_secs(secs)).expect("reading a corrupt header hung or panicked")
}

#[test]
fn damaged_headers_give_clean_errors_never_a_panic_or_a_hang() {
    use mg_core::gguf::read_info;
    let d = tmp("corrupt");
    let w = |name: &str, bytes: &[u8]| { let p = d.join(name); fs::write(&p, bytes).unwrap(); p };
    // 1) arrays whose declared length is absurd / would overflow / would seek backwards
    let mut huge_arr = kv_key("tokenizer.ggml.tokens"); huge_arr.extend(9u32.to_le_bytes()); huge_arr.extend(4u32.to_le_bytes()); huge_arr.extend(u64::MAX.to_le_bytes());   // u32 array, 2^64-1 entries
    let mut str_arr = kv_key("tokenizer.ggml.tokens"); str_arr.extend(9u32.to_le_bytes()); str_arr.extend(8u32.to_le_bytes()); str_arr.extend(1_000_000u64.to_le_bytes());
    str_arr.extend((1u64 << 63).to_le_bytes());                                                                                                                    // first string "length" 2^63 (negative as i64)
    let mut long_str = kv_key("general.description"); long_str.extend(8u32.to_le_bytes()); long_str.extend((1u64 << 40).to_le_bytes());                          // string far past the end of the file
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("huge-array.gguf", raw_header(3, 0, 1, &huge_arr)),
        ("string-array.gguf", raw_header(3, 0, 1, &str_arr)),
        ("long-string.gguf", raw_header(3, 0, 1, &long_str)),
        ("kv-count.gguf", raw_header(3, 0, 1 << 40, b"")),
        ("version-0.gguf", raw_header(0, 0, 0, b"")),
        ("version-99.gguf", raw_header(99, 0, 0, b"")),
        ("empty.gguf", vec![]),
        ("magic-only.gguf", b"GGUF".to_vec()),
        ("not-gguf.gguf", b"PK\x03\x04 this is a zip, not a model".to_vec()),
    ];
    for (name, bytes) in &cases {
        let p = w(name, bytes);
        let r = finishes_within(20, move || read_info(&p));
        assert!(r.is_err(), "{name} must be rejected, got {r:?}");
    }
    // big-endian files are named as such
    let be = w("be.gguf", &[b"GGUF".as_slice(), &3u32.to_be_bytes(), &[0u8; 16]].concat());
    assert!(read_info(&be).unwrap_err().contains("big-endian"), "{:?}", read_info(&be));
    // oversized tensor counts / dimension counts are ignored (the metadata still identifies the file), not followed
    let mut meta = kv_key("general.architecture"); meta.extend(8u32.to_le_bytes()); meta.extend(6u64.to_le_bytes()); meta.extend(b"gemma3");
    let p = w("tensors-huge.gguf", &raw_header(3, u64::MAX, 1, &meta));
    let i = finishes_within(20, move || read_info(&p)).unwrap(); assert_eq!((i.arch.as_str(), i.weight_type, i.expected_min_size), ("gemma3", None, None));
}

/// Truncating a valid header at EVERY byte and flipping/ruining bytes at pseudo-random places: always Ok or Err, never a panic or a hang.
#[test]
fn every_truncation_and_thousands_of_mutations_are_survived() {
    use mg_core::gguf::read_info;
    let d = tmp("fuzz");
    let kv = vec![("general.architecture", V::S("gemma3")), ("general.name", V::S("medgemma-selftrained")), ("gemma3.block_count", V::U(62)), ("gemma3.embedding_length", V::U(5376)), ("general.file_type", V::U(7))];
    write_gguf_t(&d.join("base.gguf"), &kv, &[("output_norm.weight", &[5376], 0), ("blk.0.ffn_up.weight", &[5376, 21504], 8), ("token_embd.weight", &[5376, 262144], 8)]);
    let base = fs::read(d.join("base.gguf")).unwrap();
    let probe = d.join("probe.gguf");
    let outcome = finishes_within(120, move || {
        let (mut ok, mut err) = (0u32, 0u32);
        let mut run = |bytes: &[u8]| { fs::write(&probe, bytes).unwrap(); if read_info(&probe).is_ok() { ok += 1 } else { err += 1 } };
        for cut in 0..=base.len() { run(&base[..cut]); }                                              // every truncation point
        let mut x: u64 = 0x9E3779B97F4A7C15;
        let mut next = || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
        for _ in 0..3000 {                                                                             // random single-byte damage
            let mut m = base.clone(); let at = (next() as usize) % m.len(); m[at] = next() as u8; run(&m);
        }
        for _ in 0..1500 {                                                                             // 8-byte windows set to 0xFF: huge counts / lengths
            let mut m = base.clone(); let at = (next() as usize) % (m.len() - 8); for b in &mut m[at..at + 8] { *b = 0xFF; } run(&m);
        }
        (ok, err)
    });
    assert!(outcome.0 > 0 && outcome.1 > 0, "expected both accepted and rejected inputs, got {outcome:?}");
}

/// All 63 non-empty subsets of the six files x several memory budgets: the invariants of "27B and 4B are independent, each uses its own
/// projector, 8-bit before 16-bit, nothing is chosen that cannot work" hold everywhere, and the folder's file ORDER never changes the choice.
#[test]
fn every_combination_of_the_six_files_obeys_the_selection_invariants() {
    let all = [A8, A16, AV, B8, B16, BV];
    for mask in 1u32..64 {
        let items: Vec<Item> = all.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, it)| *it).collect();
        let mut reversed = items.clone(); reversed.reverse();
        let (_, inv) = folder(&format!("combo{mask}"), &items);
        let (_, inv_rev) = folder(&format!("combor{mask}"), &reversed);
        let has = |n: &str| items.iter().any(|(x, _, _)| *x == n);
        for budget in [6.0, 12.0, 30.0, 60.0, 120.0] {
            let a = choose(&inv, &cpu(budget));
            let b = choose(&inv_rev, &cpu(budget));
            assert_eq!(a.as_ref().map(file_name).ok(), b.as_ref().map(file_name).ok(), "order changed the choice: mask {mask} budget {budget}");
            let Ok(p) = a else {
                // an error is only acceptable when no variant can work at this budget
                let usable_27 = (has("alpha-final.gguf") || has("alpha-full-precision.gguf")) && has("alpha-vision.gguf");
                let usable_4 = has("beta-final.gguf") || has("beta-full.gguf");
                assert!(!(usable_4 && budget >= 12.0), "a 4B model fits {budget} GB but nothing was chosen: mask {mask}");
                assert!(!(usable_27 && budget >= 120.0), "27B fits {budget} GB but nothing was chosen: mask {mask}");
                continue;
            };
            let name = file_name(&p);
            match p.family {
                Family::G27 => {
                    assert!(name.starts_with("alpha-") && !name.contains("vision"), "{name}");
                    assert!(p.multimodal && p.mmproj.as_ref().unwrap().ends_with("alpha-vision.gguf"), "27B must run with ITS projector (mask {mask})");
                    if has("alpha-final.gguf") { assert_eq!(name, "alpha-final.gguf", "8-bit comes before 16-bit (mask {mask} budget {budget})"); }
                }
                Family::G4 => {
                    assert!(name.starts_with("beta-") && !name.contains("vision"), "{name}");
                    if has("beta-vision.gguf") { assert!(p.mmproj.as_ref().unwrap().ends_with("beta-vision.gguf"), "4B must run with ITS projector (mask {mask})"); } else { assert!(!p.multimodal); }
                    if has("beta-final.gguf") { assert_eq!(name, "beta-final.gguf", "8-bit comes before 16-bit (mask {mask} budget {budget})"); }
                    // 4B is only chosen when 27B is absent, unusable (no projector) or does not fit
                    if budget >= 120.0 { assert!(!(has("alpha-vision.gguf") && (has("alpha-final.gguf") || has("alpha-full-precision.gguf"))), "27B should have won at {budget} GB (mask {mask})"); }
                }
            }
            if budget >= 120.0 && has("alpha-vision.gguf") && has("alpha-final.gguf") { assert_eq!((p.family, name.as_str()), (Family::G27, "alpha-final.gguf"), "mask {mask}"); }
        }
    }
}

fn health_lines(inv: &Inventory) -> std::collections::HashMap<String, String> {
    health(inv).lines.iter().filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect()
}

/// The doctor's health report: per-family readiness, how every file was identified, and every problem, in a stable KEY=VALUE format.
#[test]
fn the_health_report_says_exactly_what_works_and_why() {
    // everything present
    let (_, all) = folder("h_all", &[A8, A16, AV, B8, B16, BV]);
    let h = health_lines(&all);
    assert_eq!((h["HEALTH.verdict"].as_str(), h["HEALTH.family.27B"].as_str(), h["HEALTH.family.4B"].as_str(), h["HEALTH.files"].as_str()), ("OK", "ready", "ready", "6"));
    for n in 0..6 { assert_eq!(h[&format!("HEALTH.file.{n}.state")], "ok"); assert_eq!(h[&format!("HEALTH.file.{n}.header")], "read"); }
    // the precision of a file with a meaningless name came from the header
    let q: Vec<(&String, &String)> = h.iter().filter(|(k, _)| k.ends_with(".quant_source")).collect();
    assert!(q.iter().all(|(_, v)| v.as_str() == "file_type"), "{q:?}");
    // 27B without its projector, 4B complete -> degraded, with the reason per family
    let (_, no_av) = folder("h_noav", &[A8, B8, BV]);
    let h = health_lines(&no_av);
    assert_eq!((h["HEALTH.verdict"].as_str(), h["HEALTH.family.27B"].as_str(), h["HEALTH.family.4B"].as_str()), ("DEGRADED", "no-projector", "ready"));
    // 4B weights without projector: usable but text-only
    let (_, txt) = folder("h_txt", &[B16]);
    let h = health_lines(&txt);
    assert_eq!((h["HEALTH.verdict"].as_str(), h["HEALTH.family.4B"].as_str(), h["HEALTH.family.27B"].as_str()), ("DEGRADED", "text-only", "absent"));
    // nothing usable
    let (_, only_vision) = folder("h_vis", &[AV, BV]);
    assert_eq!(health_lines(&only_vision)["HEALTH.verdict"], "BROKEN");
    assert_eq!(health_lines(&Inventory::default())["HEALTH.verdict"], "EMPTY");
}

#[test]
fn the_health_report_names_incomplete_wrong_and_unreadable_files() {
    let d = tmp("h_bad");
    // incomplete 4B (tensor table needs 32 MiB, file is tiny)
    let kv = vec![("general.architecture", V::S("gemma3")), ("general.name", V::S("medgemma-selftrained")), ("gemma3.block_count", V::U(34)), ("gemma3.embedding_length", V::U(2560))];
    write_gguf_t(&d.join("cut-off.gguf"), &kv, &[("blk.0.ffn_up.weight", &[4096, 4096], 1)]);
    // wrong architecture
    write_gguf(&d.join("llama.gguf"), &[("general.architecture", V::S("llama")), ("llama.block_count", V::U(62)), ("llama.embedding_length", V::U(5376))]);
    // unreadable (>1 MiB of garbage)
    fs::write(d.join("garbage.gguf"), vec![7u8; 2 << 20]).unwrap();
    let inv = scan(&d).unwrap();
    let h = health_lines(&inv);
    assert_eq!(h["HEALTH.verdict"], "BROKEN");
    let find = |name: &str| (0..3).find(|n| h[&format!("HEALTH.file.{n}.name")] == name).unwrap();
    let (a, b, c) = (find("cut-off.gguf"), find("llama.gguf"), find("garbage.gguf"));
    assert!(h[&format!("HEALTH.file.{a}.detail")].contains("INCOMPLETE")); assert_eq!(h[&format!("HEALTH.file.{a}.state")], "bad");
    assert!(h[&format!("HEALTH.file.{b}.detail")].contains("not a Gemma 3")); assert_eq!(h[&format!("HEALTH.file.{b}.state")], "bad");
    assert_eq!(h[&format!("HEALTH.file.{c}.header")], "unreadable"); assert!(h[&format!("HEALTH.file.{c}.detail")].contains("cannot read the GGUF header"));
    assert_eq!(h["HEALTH.family.4B"], "broken");
}

// ---------------------------------------------------------------------------------------------------------------------
// GPU planning through the REAL prepare_with (the function the app calls). Needs real multi-GB file sizes, which are only
// cheap as sparse files, so these run on the Linux job (NTFS would reserve the space).
// ---------------------------------------------------------------------------------------------------------------------
#[cfg(unix)]
mod gpu {
    use super::*;

    fn real_sizes(name: &str, items: &[Item]) -> PathBuf {
        let d = tmp(name);
        for (n, k, gb) in items {
            write_item(&d, n, *k);
            fs::OpenOptions::new().write(true).open(d.join(n)).unwrap().set_len((gb * GIB as f64) as u64).unwrap();   // sparse
        }
        d
    }
    fn engines(with_gpu: bool) -> PathBuf {
        let e = tmp(if with_gpu { "eng_gpu" } else { "eng_cpu" });
        for sub in if with_gpu { vec!["cpu", "vulkan"] } else { vec!["cpu"] } {
            fs::create_dir_all(e.join("backend").join(sub)).unwrap();
            fs::write(e.join("backend").join(sub).join("llama-server"), b"stub").unwrap();
        }
        e
    }
    fn hw(free_gb: u64, vram_gb: u64, gpu_ok: bool) -> Hardware {
        Hardware { total_ram: 64 * GIB, free_ram: free_gb * GIB, gpu_ok, gpu_name: "Test GPU".into(), vram: vram_gb * GIB, vulkan_loader: gpu_ok }
    }
    fn cfg(dir: &Path, compute: &str) -> Config { Config { compute: compute.into(), model_dir: dir.display().to_string(), ..Default::default() } }

    #[test]
    fn a_24gb_gpu_runs_27b_8bit_with_partial_offload_and_48gb_offloads_everything() {
        let d = real_sizes("gpu24", &[A8, A16, AV, B8, BV]);
        let (plan, compute, note) = prepare_with(&cfg(&d, "gpu"), &hw(40, 24, true), &engines(true), NVME).unwrap();
        assert_eq!(compute, Compute::Gpu); assert!(note.is_empty(), "{note}");
        assert_eq!((plan.family, plan.quant.as_str()), (Family::G27, "Q8_0"));
        assert!(plan.n_gpu_layers > 0 && plan.n_gpu_layers < 62, "partial offload expected, got {}", plan.n_gpu_layers);
        let (full, _, _) = prepare_with(&cfg(&d, "gpu"), &hw(40, 48, true), &engines(true), NVME).unwrap();
        assert_eq!((full.family, full.n_gpu_layers), (Family::G27, 999));
    }

    #[test]
    fn cpu_choice_never_offloads_even_when_a_gpu_exists() {
        let d = real_sizes("cpuchoice", &[A8, AV, B8, BV]);
        let (plan, compute, _) = prepare_with(&cfg(&d, "cpu"), &hw(40, 24, true), &engines(true), NVME).unwrap();
        assert_eq!((compute, plan.n_gpu_layers), (Compute::Cpu, 0));
        assert_eq!(plan.family, Family::G27);
    }

    #[test]
    fn requesting_gpu_without_a_usable_gpu_or_engine_falls_back_to_cpu_with_a_clear_note() {
        let d = real_sizes("nogpu", &[A8, AV, B8, BV]);
        let (p, c, note) = prepare_with(&cfg(&d, "gpu"), &hw(40, 0, false), &engines(true), NVME).unwrap();
        assert_eq!((c, p.n_gpu_layers), (Compute::Cpu, 0)); assert!(note.contains("GPU not available"), "{note}");
        let (p, c, note) = prepare_with(&cfg(&d, "gpu"), &hw(40, 24, true), &engines(false), NVME).unwrap();
        assert_eq!((c, p.n_gpu_layers), (Compute::Cpu, 0)); assert!(note.contains("GPU engine not installed"), "{note}");
    }

    #[test]
    fn a_small_gpu_with_little_ram_falls_back_to_4b_and_still_offloads_part_of_it() {
        let d = real_sizes("smallgpu", &[A8, A16, AV, B8, B16, BV]);
        let (plan, compute, _) = prepare_with(&cfg(&d, "gpu"), &hw(8, 6, true), &engines(true), NVME).unwrap();
        assert_eq!(compute, Compute::Gpu);
        assert_eq!((plan.family, plan.quant.as_str()), (Family::G4, "Q8_0"));
        assert!(plan.n_gpu_layers > 0 && plan.n_gpu_layers < 999, "{}", plan.n_gpu_layers);
    }

    #[test]
    fn sixteen_bit_27b_alone_runs_on_a_24gb_gpu() {
        let d = real_sizes("gpu16", &[A16, AV]);
        let (plan, _, _) = prepare_with(&cfg(&d, "gpu"), &hw(40, 24, true), &engines(true), NVME).unwrap();
        assert_eq!((plan.family, plan.quant.as_str()), (Family::G27, "F16"));
        assert!(plan.n_gpu_layers > 0 && plan.n_gpu_layers < 62);
    }
}
