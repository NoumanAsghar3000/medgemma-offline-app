//! Drives the REAL llama-server binary (b12244) with a tiny random-weight gemma3 GGUF.
//! Skipped unless MG_REAL_LLAMA_DIR (folder with llama-server + libs) and MG_TINY_GGUF are set.
#![cfg(unix)]
use mg_core::{backend, config::Config, hw::Hardware, http, plan::Bandwidth, GIB};
use std::{fs, os::unix::fs::symlink, path::PathBuf, sync::atomic::AtomicBool, time::Duration};

#[test]
fn real_llama_server_end_to_end() {
    let (Ok(dir), Ok(gguf)) = (std::env::var("MG_REAL_LLAMA_DIR"), std::env::var("MG_TINY_GGUF")) else {
        eprintln!("SKIPPED: set MG_REAL_LLAMA_DIR and MG_TINY_GGUF to run against the real llama-server");
        return;
    };
    let root = std::env::temp_dir().join(format!("mg_real_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let models = root.join("models"); let app = root.join("app");
    fs::create_dir_all(&models).unwrap(); fs::create_dir_all(app.join("backend")).unwrap();
    fs::copy(&gguf, models.join("medgemma-4b-it-F32.gguf")).unwrap();   // classified as the 4B family
    symlink(PathBuf::from(&dir), app.join("backend/cpu")).unwrap();

    let cfg = Config { compute: "cpu".into(), model_dir: models.display().to_string(), ..Default::default() };
    let hw = Hardware { total_ram: 16 * GIB, free_ram: 8 * GIB, ..Default::default() };
    let (plan, compute, _) = backend::prepare_with(&cfg, &hw, &app, Bandwidth { ram_gbps: 20.0, disk_mbps: 500.0 }).expect("prepare");
    assert!(!plan.multimodal, "tiny model has no mmproj");

    let log = app.join("backend.log");
    let mut l = backend::start_ready(&plan, compute, &app, &log, "k3y", &AtomicBool::new(false), Duration::from_secs(60))
        .unwrap_or_else(|e| panic!("start_ready failed: {e}\n{}", backend::log_tail(&log)));

    let mut ev = vec![];
    http::post_sse(l.port, "k3y", r#"{"messages":[{"role":"user","content":"hello"}],"stream":true,"max_tokens":6}"#, &AtomicBool::new(false), |e| ev.push(e))
        .expect("streamed chat against the real server");
    assert!(ev.len() >= 4, "{ev:?}");
    assert_eq!(ev.last().unwrap(), "[DONE]");
    let last_json = ev.iter().rev().find(|e| e.starts_with('{')).unwrap();
    let v: serde_json::Value = serde_json::from_str(last_json).unwrap();
    let tps = v["timings"]["predicted_per_second"].as_f64().expect("timings.predicted_per_second in the final chunk");
    assert!(tps > 0.0);
    assert!(http::post_sse(l.port, "WRONG", "{}", &AtomicBool::new(false), |_| {}).unwrap_err().contains("401"));
    backend::kill(&mut l.child);
    eprintln!("REAL llama-server OK: {} events, {tps:.0} tok/s (random tiny model)", ev.len());
}
