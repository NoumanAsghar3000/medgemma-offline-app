//! Drives the REAL llama-server binary (b12244) with a tiny random-weight gemma3 GGUF.
//! Skipped unless MG_REAL_LLAMA_DIR (folder with llama-server + libs) and MG_TINY_GGUF are set.
#![cfg(unix)]
use mg_core::{backend, config::Config, guard, hw::Hardware, http, plan::Bandwidth, GIB};
use serde_json::json;
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

    // The conversation the Windows end-to-end test has (steps 5a -> 5b), through the shell's request gate: first a request WITH an image against
    // this text-only engine (must fail with the server's own words), then a text-only follow-up whose prompt is far longer than 512 tokens
    // (the app's system prompt + attached documents + the earlier turns, one of them an empty assistant turn from the failed request).
    // It must be ANSWERED. Recent llama.cpp builds cap the server's context to the model's declared training context, so a fixture that
    // declares too little makes every realistic request fail with "exceeds the available context size" - caught here, in seconds, on Linux.
    let system = "You are a careful clinical assistant. ".repeat(45);                 // ~1.7 KB, like the app's real system prompt
    let docs = "Lab result: glucose 5.4 mmol/L, creatinine 82 umol/L, HbA1c 7.9 %. ".repeat(90);   // ~6 KB of attached document text
    let with_image = json!({ "stream": true, "temperature": 0.2, "max_tokens": 4, "messages": [
        { "role": "system", "content": system },
        { "role": "user", "content": [ { "type": "text", "text": "Please assess this case." }, { "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQ==" } } ] } ] });
    let err = http::post_sse(l.port, "k3y", &guard::sanitize_chat_body(&with_image.to_string()).unwrap(), &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(err.contains("image input is not supported"), "{err}");

    let follow_up = json!({ "stream": true, "temperature": 0.2, "max_tokens": 8, "messages": [
        { "role": "system", "content": system },
        { "role": "user", "content": "Please assess this case. [file: labs.csv]" },
        { "role": "assistant", "content": "" },
        { "role": "user", "content": [ { "type": "text", "text": format!("Summarise the documents.\n\n{docs}") } ] } ] });
    let mut ev2 = vec![];
    http::post_sse(l.port, "k3y", &guard::sanitize_chat_body(&follow_up.to_string()).unwrap(), &AtomicBool::new(false), |e| ev2.push(e))
        .unwrap_or_else(|e| panic!("the follow-up request was rejected: {e}\n{}", backend::log_tail(&log)));
    let last2: serde_json::Value = serde_json::from_str(ev2.iter().rev().find(|e| e.starts_with('{')).unwrap()).unwrap();
    let prompt_n = last2["timings"]["prompt_n"].as_u64().expect("timings.prompt_n");
    assert!(prompt_n > 512, "the prompt was only {prompt_n} tokens: the test no longer exercises a context beyond 512");
    assert!(ev2.iter().any(|e| e.contains("\"content\":\"")), "no text was streamed: {ev2:?}");
    backend::kill(&mut l.child);
    eprintln!("REAL llama-server OK: {} events, {tps:.0} tok/s (random tiny model)", ev.len());
}
