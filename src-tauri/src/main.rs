#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Native shell. Owns: config, model-folder validation/re-link, lifecycle of the local llama.cpp server,
//! the chat proxy (webview -> Rust -> loopback server), idle unloading ("memory modes") and PDF saving.
//! All hardware/model decisions live in `mg-core` (shared with the installer helper `mg-probe`).
use mg_core::{backend, config::Config, hw, http};
use serde::Serialize;
use std::{
    path::PathBuf,
    process::Child,
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{ipc::Channel, AppHandle, Emitter, Manager, RunEvent, State};
use tauri_plugin_dialog::DialogExt;

#[derive(Clone, Serialize, Default)]
struct Status {
    /// need_model_folder | loading | ready | idle | error
    state: String,
    message: String,
    model_dir: String,
    compute: String,
    model: String,
    quant: String,
    family: String,
    multimodal: bool,
    ctx: u32,
    note: String,
    idle_minutes: u32,
    // planner estimate for the loaded variant
    tps_best: f64,
    tps_worst: f64,
    verdict: String,
    streamed_gb: f64,
    forced: bool,
    model_file: String,
}

struct Conn { port: u16, token: String }
#[derive(Default)]
struct Inner { child: Option<Child>, status: Status, conn: Option<Conn> }
struct AppState {
    inner: Mutex<Inner>,
    boot_cancel: Mutex<Arc<AtomicBool>>,
    chat_cancel: AtomicBool,
    busy: AtomicBool,
    last_active: Mutex<Instant>,
}

fn exe_dir() -> PathBuf { std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default() }
fn user_cfg(app: &AppHandle) -> PathBuf { app.path().app_data_dir().unwrap_or_else(|_| exe_dir()).join("config.json") }
fn load_cfg(app: &AppHandle) -> Config {
    // user config (written when the folder is re-linked / memory mode changed) wins over the installer's seed
    Config::load(&user_cfg(app)).or_else(|| Config::load(&exe_dir().join("install.json"))).unwrap_or_default()
}
fn token() -> String {
    let mut b = [0u8; 16]; let _ = getrandom::getrandom(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn publish(app: &AppHandle, s: Status) {
    app.state::<AppState>().inner.lock().unwrap().status = s.clone();
    let _ = app.emit("status", s);
}
fn touch(app: &AppHandle) { *app.state::<AppState>().last_active.lock().unwrap() = Instant::now(); }

/// (Re)start the whole pipeline on a worker thread; the UI never blocks.
fn bootstrap(app: AppHandle) {
    let st = app.state::<AppState>();
    {
        let mut c = st.boot_cancel.lock().unwrap();
        c.store(true, Ordering::SeqCst);
        *c = Arc::new(AtomicBool::new(false));
    }
    {
        let mut g = st.inner.lock().unwrap();
        if let Some(mut ch) = g.child.take() { backend::kill(&mut ch); }
        g.conn = None;
    }
    let cancel = st.boot_cancel.lock().unwrap().clone();

    std::thread::spawn(move || {
        let cfg = load_cfg(&app);
        let mut s = Status { model_dir: cfg.model_dir.clone(), compute: cfg.compute.clone(), idle_minutes: cfg.idle_minutes, ..Default::default() };
        let hw = hw::detect();
        let (plan, compute, note) = match backend::prepare(&cfg, &hw, &exe_dir()) {
            Ok(x) => x,
            Err(backend::Prep::NeedFolder(m)) => { s.state = "need_model_folder".into(); s.message = m; return publish(&app, s); }
            Err(backend::Prep::Error(m)) => { s.state = "error".into(); s.message = m; return publish(&app, s); }
        };
        s.state = "loading".into();
        s.model = plan.model.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        s.quant = plan.quant.clone(); s.family = plan.family_label.clone(); s.multimodal = plan.multimodal;
        s.tps_best = plan.placement.tps_best; s.tps_worst = plan.placement.tps_worst; s.verdict = plan.placement.verdict.label().into();
        s.streamed_gb = plan.placement.streamed_bytes as f64 / (1u64 << 30) as f64; s.forced = plan.forced; s.model_file = s.model.clone();
        s.ctx = plan.ctx; s.note = if note.is_empty() { plan.note.clone() } else { note };
        s.compute = if compute == mg_core::select::Compute::Gpu { "gpu".into() } else { "cpu".into() };
        s.message = format!("Loading {} ({}). Weights are read lazily from disk - a large model can take a few minutes from a USB drive.", s.family, s.quant);
        publish(&app, s.clone());

        let tok = token();
        let log = app.path().app_log_dir().unwrap_or_else(|_| exe_dir()).join("backend.log");
        match backend::start_ready(&plan, compute, &exe_dir(), &log, &tok, &cancel, Duration::from_secs(30 * 60)) {
            Ok(l) => {
                if cancel.load(Ordering::SeqCst) { let mut c = l.child; backend::kill(&mut c); return; }
                {
                    let state = app.state::<AppState>();
                    let mut g = state.inner.lock().unwrap();
                    g.child = Some(l.child); g.conn = Some(Conn { port: l.port, token: tok });
                }
                touch(&app);
                s.state = "ready".into(); s.message = String::new();
                publish(&app, s);
            }
            Err(e) => { if e == "cancelled" { return; } s.state = "error".into(); s.message = e; publish(&app, s); }
        }
    });
}

/// Memory modes: unload the model (kill the server) when idle so the RAM goes back to Windows.
fn start_idle_watcher(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(3));
        let st = app.state::<AppState>();
        let mut g = st.inner.lock().unwrap();
        if g.status.state != "ready" || st.busy.load(Ordering::SeqCst) { continue; }
        let mins = g.status.idle_minutes;
        if mins >= 9999 { continue; }
        let limit = if mins == 0 { Duration::from_secs(4) } else { Duration::from_secs(mins as u64 * 60) };
        if st.last_active.lock().unwrap().elapsed() < limit { continue; }
        if let Some(mut ch) = g.child.take() { backend::kill(&mut ch); }
        g.conn = None;
        g.status.state = "idle".into();
        g.status.message = "Model unloaded to free memory. It reloads automatically when you send your next message.".into();
        let s = g.status.clone(); drop(g);
        let _ = app.emit("status", s);
    });
}

#[tauri::command]
fn get_status(state: State<AppState>) -> Status { state.inner.lock().unwrap().status.clone() }

#[tauri::command(async)]
fn pick_model_folder(app: AppHandle) -> Result<bool, String> {
    // CI-only hook (never set for users): lets the end-to-end test choose the folder without a native dialog.
    let dir = if let Ok(t) = std::env::var("MG_TEST_PICK_FOLDER") { PathBuf::from(t) } else {
        let Some(p) = app.dialog().file().set_title("Select the folder that contains the MedGemma model files").blocking_pick_folder() else {
            return Ok(false);
        };
        p.into_path().map_err(|e| e.to_string())?
    };
    let inv = mg_core::select::scan(&dir).map_err(|e| format!("Cannot read that folder: {e}"))?;
    if !inv.files.iter().any(|f| !f.is_mmproj && f.family.is_some()) {
        return Err("No MedGemma .gguf model files were found in that folder (looking for names containing 27b or 4b).".into());
    }
    let mut cfg = load_cfg(&app);
    cfg.model_dir = dir.display().to_string();
    cfg.save(&user_cfg(&app)).map_err(|e| format!("Could not save settings: {e}"))?;
    bootstrap(app);
    Ok(true)
}

#[tauri::command]
fn retry(app: AppHandle) { bootstrap(app); }

#[derive(Serialize)]
struct VariantInfo { file: String, family: String, quant: String, size_gb: f64, multimodal: bool, tps_best: f64, tps_worst: f64, verdict: String, streamed_gb: f64, auto: bool, current: bool }

/// Every installed variant with its estimated speed, for the header selector.
#[tauri::command(async)]
fn list_variants(app: AppHandle) -> Result<Vec<VariantInfo>, String> {
    let cfg = load_cfg(&app);
    let (vs, auto) = backend::list_variants(&cfg, &hw::detect(), &exe_dir())?;
    let cur = app.state::<AppState>().inner.lock().unwrap().status.model_file.clone();
    Ok(vs.into_iter().map(|p| {
        let file = p.model.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        VariantInfo { auto: auto.as_deref() == Some(file.as_str()), current: file == cur, family: p.family_label.clone(), quant: p.quant.clone(),
            size_gb: p.model_bytes as f64 / (1u64 << 30) as f64, multimodal: p.multimodal, tps_best: p.placement.tps_best, tps_worst: p.placement.tps_worst,
            verdict: p.placement.verdict.label().into(), streamed_gb: p.placement.streamed_bytes as f64 / (1u64 << 30) as f64, file }
    }).collect())
}

/// Force a variant by file name ("" = back to the automatic policy) and reload.
#[tauri::command]
fn choose_variant(app: AppHandle, file: String) -> Result<(), String> {
    let mut cfg = load_cfg(&app);
    cfg.forced_model = file;
    cfg.save(&user_cfg(&app)).map_err(|e| e.to_string())?;
    bootstrap(app);
    Ok(())
}

#[tauri::command]
fn set_memory_mode(app: AppHandle, minutes: u32) -> Result<(), String> {
    let mut cfg = load_cfg(&app);
    cfg.idle_minutes = minutes;
    cfg.save(&user_cfg(&app)).map_err(|e| e.to_string())?;
    let st = app.state::<AppState>();
    let s = { let mut g = st.inner.lock().unwrap(); g.status.idle_minutes = minutes; g.status.clone() };
    touch(&app);
    let _ = app.emit("status", s);
    Ok(())
}

/// Streams the model's reply. `body` is the OpenAI-style JSON built by the UI. Each SSE `data:` payload is
/// pushed to the UI through `on_event`. The server key and port never reach the webview.
#[tauri::command(async)]
fn chat_stream(app: AppHandle, body: String, on_event: Channel<String>) -> Result<(), String> {
    let st = app.state::<AppState>();
    let (port, tok) = {
        let g = st.inner.lock().unwrap();
        match &g.conn { Some(c) => { st.busy.store(true, Ordering::SeqCst); (c.port, c.token.clone()) }
                        None => return Err("The model is not loaded yet.".into()) }
    };
    st.chat_cancel.store(false, Ordering::SeqCst);
    let r = http::post_sse(port, &tok, &body, &st.chat_cancel, |e| { let _ = on_event.send(e); });
    st.busy.store(false, Ordering::SeqCst);
    touch(&app);
    if r.is_err() {
        // did the engine crash (e.g. out of memory)? then surface a recoverable error state instead of a dead chat
        let mut g = st.inner.lock().unwrap();
        let died = g.child.as_mut().map(|c| matches!(c.try_wait(), Ok(Some(_)))).unwrap_or(false);
        if died {
            g.child = None; g.conn = None; g.status.state = "error".into();
            let log = app.path().app_log_dir().unwrap_or_else(|_| exe_dir()).join("backend.log");
            g.status.message = format!("The inference engine stopped unexpectedly (possibly out of memory).\n{}", backend::log_tail(&log));
            let s = g.status.clone(); drop(g); let _ = app.emit("status", s);
        }
    }
    r
}

#[tauri::command]
fn chat_cancel(state: State<AppState>) { state.chat_cancel.store(true, Ordering::SeqCst); }

/// Native "Save as" + write. (Browser-style <a download> is unreliable inside WebView2.)
#[tauri::command(async)]
fn save_report(app: AppHandle, filename: String, data_b64: String) -> Result<Option<String>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let bytes = STANDARD.decode(data_b64).map_err(|e| e.to_string())?;
    // CI-only hook (never set for users): the native Save dialog cannot be clicked by the headless end-to-end test.
    let path = if let Ok(t) = std::env::var("MG_TEST_SAVE_PATH") { PathBuf::from(t) } else {
        let Some(p) = app.dialog().file().set_title("Save medical report").set_file_name(filename).add_filter("PDF document", &["pdf"]).blocking_save_file() else {
            return Ok(None);
        };
        p.into_path().map_err(|e| e.to_string())?
    };
    std::fs::write(&path, bytes).map_err(|e| format!("Could not write the file: {e}"))?;
    Ok(Some(path.display().to_string()))
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            inner: Mutex::new(Inner::default()),
            boot_cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
            chat_cancel: AtomicBool::new(false), busy: AtomicBool::new(false),
            last_active: Mutex::new(Instant::now()),
        })
        .setup(|app| { bootstrap(app.handle().clone()); start_idle_watcher(app.handle().clone()); Ok(()) })
        .invoke_handler(tauri::generate_handler![get_status, pick_model_folder, retry, list_variants, choose_variant, set_memory_mode, chat_stream, chat_cancel, save_report])
        .build(tauri::generate_context!())
        .expect("failed to build app");

    app.run(|h, e| {
        if let RunEvent::Exit = e {
            if let Some(mut ch) = h.state::<AppState>().inner.lock().unwrap().child.take() { backend::kill(&mut ch); }
        }
    });
}
