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

fn write_gguf(path: &Path, kvs: &[(&str, V)]) {
    let mut b: Vec<u8> = b"GGUF".to_vec();
    b.extend(3u32.to_le_bytes()); b.extend(0u64.to_le_bytes()); b.extend((kvs.len() as u64).to_le_bytes());
    for (k, v) in kvs {
        b.extend((k.len() as u64).to_le_bytes()); b.extend(k.as_bytes());
        match v {
            V::S(s) => { b.extend(8u32.to_le_bytes()); b.extend((s.len() as u64).to_le_bytes()); b.extend(s.as_bytes()); }
            V::U(n) => { b.extend(4u32.to_le_bytes()); b.extend(n.to_le_bytes()); }
        }
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
