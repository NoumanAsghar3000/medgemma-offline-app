#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Native shell. Owns: config, model-folder validation/re-link, lifecycle of the local llama.cpp server,
//! the chat proxy (webview -> Rust -> loopback server), idle unloading ("memory modes") and PDF saving.
//! All hardware/model decisions live in `mg-core` (shared with the installer helper `mg-probe`).
use mg_core::{backend, config::Config, guard, hw, http};
use serde::Serialize;
use std::{
    path::PathBuf,
    process::Child,
    sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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
    /// a Vulkan-capable GPU (>= 2 GB VRAM) AND the GPU engine are present: the CPU/GPU switch in the header offers GPU
    gpu_available: bool,
    /// increases with every status change; the UI ignores a status that is older than one it has already shown (events and
    /// get_status answers can arrive out of order, and a stale one must never overwrite a newer state)
    seq: u64,
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
    /// Hardware + bandwidth measured BEFORE the current model was loaded. The variant list reuses them: measured again with the model
    /// resident, free RAM is lower by the model's own size and every estimate (and the "very slow" warning) would be wrong.
    machine: Mutex<Option<(hw::Hardware, mg_core::plan::Bandwidth)>>,
    seq: AtomicU64,
}

fn exe_dir() -> PathBuf { std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default() }
fn user_cfg(app: &AppHandle) -> PathBuf { app.path().app_data_dir().unwrap_or_else(|_| exe_dir()).join("config.json") }
fn load_cfg(app: &AppHandle) -> Config {
    // The user's config (written when the folder is re-linked / a setting is changed) wins over the installer's install.json,
    // unless the installer was run again since: then its new CPU/GPU and model-folder choices apply (see Config::resolve).
    let seed = std::fs::read_to_string(exe_dir().join("install.json")).ok();
    Config::resolve(Config::load(&user_cfg(app)), seed.as_deref())
}
/// Per-session server key. Fails CLOSED: if the OS cannot supply randomness the engine is not started (an ignored error here used to
/// leave an all-zero, guessable key).
fn token() -> Result<String, String> {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}
/// Plain-text diagnostics next to the engine's own log (backend.log): every status change, and the size and outcome of every chat request.
/// Only states, sizes and error messages go in (never the prompt, attachments or answers), so it holds no patient data. Capped at 2 MB.
fn diag(app: &AppHandle, msg: &str) {
    use std::io::Write;
    let dir = app.path().app_log_dir().unwrap_or_else(|_| exe_dir());
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("diagnostics.log");
    if std::fs::metadata(&p).map(|m| m.len() > 2_000_000).unwrap_or(false) { let _ = std::fs::remove_file(&p); }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let one_line: String = msg.chars().map(|c| if c.is_control() { ' ' } else { c }).take(400).collect();
        let _ = writeln!(f, "{ms} {one_line}");
    }
}

/// Next status number. Taken while the status lock is held, so the stored status always carries the newest number.
fn next_seq(st: &AppState) -> u64 { st.seq.fetch_add(1, Ordering::SeqCst) + 1 }

fn publish(app: &AppHandle, mut s: Status) {
    let st = app.state::<AppState>();
    { let mut g = st.inner.lock().unwrap(); s.seq = next_seq(&st); g.status = s.clone(); }
    diag(app, &format!("status #{} {} model={} compute={} ctx={} {}", s.seq, s.state, s.model_file, s.compute, s.ctx, s.message));
    let _ = app.emit("status", s);
}

fn touch(app: &AppHandle) { *app.state::<AppState>().last_active.lock().unwrap() = Instant::now(); }

/// (Re)start the whole pipeline on a worker thread; the UI never blocks.
fn bootstrap(app: AppHandle, announce: bool) {
    let st = app.state::<AppState>();
    {
        let mut c = st.boot_cancel.lock().unwrap();
        c.store(true, Ordering::SeqCst);
        *c = Arc::new(AtomicBool::new(false));
    }
    // Show "loading" AT ONCE. Until the worker publishes its first status (hardware probe + bandwidth measurement, a second or more),
    // the UI would otherwise show the previous state - at start-up an empty status, which it renders as "The model could not start".
    let (old_child, snapshot) = {
        let mut g = st.inner.lock().unwrap();
        g.conn = None;
        g.status.state = "loading".into();
        g.status.message = "Preparing the model...".into();
        g.status.seq = next_seq(&st);
        (g.child.take(), g.status.clone())
    };
    // At start-up nobody is listening yet (the page asks get_status when it is ready), so nothing is emitted from the setup callback.
    diag(&app, "bootstrap: preparing the model");
    if announce { let _ = app.emit("status", snapshot); }
    // Stop the previous engine OUTSIDE the lock: releasing a mapped 20+ GB model can take seconds and must not block status queries.
    if let Some(mut ch) = old_child { backend::kill(&mut ch); }
    let cancel = st.boot_cancel.lock().unwrap().clone();

    std::thread::spawn(move || {
        // A newer bootstrap supersedes this one: it must not publish anything any more (stale errors / states would overwrite the new run).
        let cancelled = || cancel.load(Ordering::SeqCst);
        let cfg = load_cfg(&app);
        let mut s = Status { model_dir: cfg.model_dir.clone(), compute: cfg.compute.clone(), idle_minutes: cfg.idle_minutes, ..Default::default() };
        let hw = hw::detect();
        s.gpu_available = hw.gpu_ok && backend::server_exe(&exe_dir(), mg_core::select::Compute::Gpu).is_file();
        let bw = backend::probe_bandwidth(&cfg);
        *app.state::<AppState>().machine.lock().unwrap() = Some((hw.clone(), bw));
        let prepared = backend::prepare_with(&cfg, &hw, &exe_dir(), bw);
        if cancelled() { return; }
        let (plan, compute, note) = match prepared {
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

        let tok = match token() {
            Ok(t) => t,
            Err(e) => { s.state = "error".into(); s.message = format!("Could not create a secure session key ({e}). The model server was not started."); return publish(&app, s); }
        };
        let log = app.path().app_log_dir().unwrap_or_else(|_| exe_dir()).join("backend.log");
        match backend::start_ready(&plan, compute, &exe_dir(), &log, &tok, &cancel, Duration::from_secs(30 * 60)) {
            Ok(l) => {
                {
                    let state = app.state::<AppState>();
                    let mut g = state.inner.lock().unwrap();
                    // Checked INSIDE the lock: a newer bootstrap sets `cancel` first and only then takes this lock to collect the child, so either
                    // it sees this child (and stops it) or we see `cancel` here. Otherwise the old engine would be orphaned, holding all its RAM.
                    if cancelled() { drop(g); let mut c = l.child; backend::kill(&mut c); return; }
                    g.child = Some(l.child); g.conn = Some(Conn { port: l.port, token: tok });
                }
                touch(&app);
                s.state = "ready".into(); s.message = String::new();
                publish(&app, s);
            }
            Err(e) => { if e == "cancelled" || cancelled() { return; } s.state = "error".into(); s.message = e; publish(&app, s); }
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
        let child = g.child.take();
        g.conn = None;
        g.status.state = "idle".into();
        g.status.message = "Model unloaded to free memory. It reloads automatically when you send your next message.".into();
        g.status.seq = next_seq(&st);
        let s = g.status.clone(); drop(g);
        let _ = app.emit("status", s);
        if let Some(mut ch) = child { backend::kill(&mut ch); }   // outside the lock: freeing a mapped model can take seconds
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
        return Err("No usable MedGemma model files were found in that folder. The app looks at the content of every .gguf file (27B or 4B Gemma 3 weights, 8-bit or 16-bit, plus the matching mmproj), not at the file names.".into());
    }
    let mut cfg = load_cfg(&app);
    cfg.model_dir = dir.display().to_string();
    cfg.save(&user_cfg(&app)).map_err(|e| format!("Could not save settings: {e}"))?;
    bootstrap(app, true);
    Ok(true)
}

#[tauri::command(async)]
fn retry(app: AppHandle) { bootstrap(app, true); }

#[derive(Serialize)]
struct VariantInfo { file: String, family: String, quant: String, size_gb: f64, multimodal: bool, tps_best: f64, tps_worst: f64, verdict: String, streamed_gb: f64, auto: bool, current: bool }

/// Every installed variant with its estimated speed, for the header selector.
#[tauri::command(async)]
fn list_variants(app: AppHandle) -> Result<Vec<VariantInfo>, String> {
    let cfg = load_cfg(&app);
    let snapshot = app.state::<AppState>().machine.lock().unwrap().clone();
    let (hwd, bw) = match snapshot {
        Some(x) => x,
        None => { let h = hw::detect(); let b = backend::probe_bandwidth(&cfg); (h, b) }
    };
    let (vs, auto) = backend::list_variants_with(&cfg, &hwd, &exe_dir(), bw)?;
    let cur = app.state::<AppState>().inner.lock().unwrap().status.model_file.clone();
    Ok(vs.into_iter().map(|p| {
        let file = p.model.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        VariantInfo { auto: auto.as_deref() == Some(file.as_str()), current: file == cur, family: p.family_label.clone(), quant: p.quant.clone(),
            size_gb: p.model_bytes as f64 / (1u64 << 30) as f64, multimodal: p.multimodal, tps_best: p.placement.tps_best, tps_worst: p.placement.tps_worst,
            verdict: p.placement.verdict.label().into(), streamed_gb: p.placement.streamed_bytes as f64 / (1u64 << 30) as f64, file }
    }).collect())
}

/// Force a variant by file name ("" = back to the automatic policy) and reload.
#[tauri::command(async)]
fn choose_variant(app: AppHandle, file: String) -> Result<(), String> {
    let mut cfg = load_cfg(&app);
    cfg.forced_model = file;
    cfg.save(&user_cfg(&app)).map_err(|e| e.to_string())?;
    bootstrap(app, true);
    Ok(())
}

/// CPU / GPU switch in the header (the installer's choice is only the starting point). GPU needs a Vulkan-capable GPU with >= 2 GB VRAM
/// and the GPU engine that the installer copies when the GPU option was selected; otherwise the request is refused with a clear reason.
#[tauri::command(async)]
fn set_compute(app: AppHandle, compute: String) -> Result<(), String> {
    let want = compute.trim().to_ascii_lowercase();
    if want != "cpu" && want != "gpu" { return Err("Unknown processor choice.".into()); }
    if want == "gpu" {
        let h = hw::detect();
        if !h.gpu_ok { return Err("GPU is not available: no Vulkan-capable graphics card with at least 2 GB of video memory was found on this computer.".into()); }
        if !backend::server_exe(&exe_dir(), mg_core::select::Compute::Gpu).is_file() { return Err("GPU is not available: the GPU engine was not installed. Run the setup again and select GPU.".into()); }
    }
    let mut cfg = load_cfg(&app);
    cfg.compute = want;
    cfg.save(&user_cfg(&app)).map_err(|e| format!("Could not save settings: {e}"))?;
    bootstrap(app, true);
    Ok(())
}

#[tauri::command(async)]
fn set_memory_mode(app: AppHandle, minutes: u32) -> Result<(), String> {
    let mut cfg = load_cfg(&app);
    cfg.idle_minutes = minutes;
    cfg.save(&user_cfg(&app)).map_err(|e| e.to_string())?;
    let st = app.state::<AppState>();
    let s = { let mut g = st.inner.lock().unwrap(); g.status.idle_minutes = minutes; g.status.seq = next_seq(&st); g.status.clone() };
    touch(&app);
    let _ = app.emit("status", s);
    Ok(())
}

/// Streams the model's reply. `body` is the OpenAI-style JSON built by the UI. Each SSE `data:` payload is
/// pushed to the UI through `on_event`. The server key and port never reach the webview.
#[tauri::command(async)]
fn chat_stream(app: AppHandle, body: String, on_event: Channel<String>) -> Result<(), String> {
    // The webview is not trusted to talk to the engine directly: only the allow-listed request shape gets through (see mg_core::guard).
    let body = guard::sanitize_chat_body(&body).map_err(|e| { diag(&app, &format!("chat request REFUSED by the request gate: {e}")); e })?;
    diag(&app, &format!("chat request {} bytes", body.len()));
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
    match &r { Ok(()) => diag(&app, "chat request finished"), Err(e) => diag(&app, &format!("chat request FAILED: {e}")) }
    if r.is_err() {
        // The engine can no longer serve the chat: it crashed (e.g. out of memory), or the drive that holds the model was unplugged / the
        // folder was moved. A missing folder opens the same re-link dialog as at start-up (and "Retry" works once the drive is back);
        // a crash shows its log. Either way the dead engine is stopped and the chat is not left in a state that looks "ready".
        let (child, s) = {
            let mut g = st.inner.lock().unwrap();
            let died = g.child.as_mut().map(|c| matches!(c.try_wait(), Ok(Some(_)))).unwrap_or(false);
            let gone = g.conn.is_some() && backend::folder_missing(&g.status.model_dir);
            if died || gone {
                g.conn = None;
                let child = g.child.take();
                if gone {
                    g.status.state = "need_model_folder".into();
                    g.status.message = format!("{}. The folder \"{}\" is no longer available (drive unplugged, or the folder was moved or renamed). Connect the drive or select the folder again.", backend::MSG_NOT_FOUND, g.status.model_dir);
                } else {
                    g.status.state = "error".into();
                    let log = app.path().app_log_dir().unwrap_or_else(|_| exe_dir()).join("backend.log");
                    g.status.message = format!("The inference engine stopped unexpectedly (possibly out of memory).\n{}", backend::log_tail(&log));
                }
                g.status.seq = next_seq(&st);
                (child, Some(g.status.clone()))
            } else { (None, None) }
        };
        if let Some(s) = s { diag(&app, &format!("engine unavailable after a failed request -> {} {}", s.state, s.message)); let _ = app.emit("status", s); }
        if let Some(mut ch) = child { backend::kill(&mut ch); }
    }
    r
}

#[tauri::command]
fn chat_cancel(state: State<AppState>) { state.chat_cancel.store(true, Ordering::SeqCst); }

/// Native "Save as" + write. (Browser-style <a download> is unreliable inside WebView2.)
/// Only a PDF is ever written, the file name offered is a bare `*.pdf` name, and the saved file always ends in `.pdf`.
#[tauri::command(async)]
fn save_report(app: AppHandle, filename: String, data_b64: String) -> Result<Option<String>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    if data_b64.len() > guard::MAX_REPORT_BYTES / 3 * 4 + 8 { return Err("The report is too large.".into()); }
    let bytes = STANDARD.decode(data_b64).map_err(|_| "The report data is not valid.".to_string())?;
    guard::check_report_bytes(&bytes)?;
    // CI-only hook (never set for users): the native Save dialog cannot be clicked by the headless end-to-end test.
    let mut path = if let Ok(t) = std::env::var("MG_TEST_SAVE_PATH") { PathBuf::from(t) } else {
        let Some(p) = app.dialog().file().set_title("Save medical report").set_file_name(guard::safe_report_name(&filename)).add_filter("PDF document", &["pdf"]).blocking_save_file() else {
            return Ok(None);
        };
        p.into_path().map_err(|e| e.to_string())?
    };
    if !path.extension().map(|e| e.eq_ignore_ascii_case("pdf")).unwrap_or(false) {
        let mut os = path.into_os_string(); os.push(".pdf"); path = PathBuf::from(os);
    }
    std::fs::write(&path, bytes).map_err(|e| format!("Could not write the file: {e}"))?;
    Ok(Some(path.display().to_string()))
}

/// The main window is created here (tauri.conf.json has `"create": false`) with exactly the configuration it would otherwise get.
/// The ONLY reason it is created in code: on Windows, Tauri/wry hand WebView2 their own browser arguments through the API, which the
/// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS environment variable cannot reliably extend. The CI-only hook below (never set for users) lets the
/// end-to-end test add `--remote-debugging-port` and fake-camera flags through the API.
fn create_main_window(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = app.config().app.windows.first().cloned().ok_or("no window is configured in tauri.conf.json")?;
    let builder = tauri::WebviewWindowBuilder::from_config(app, &cfg)?;
    #[cfg(windows)]
    let builder = match std::env::var("MG_TEST_BROWSER_ARGS") {
        // only the exact Chromium flags the test suite needs are honoured (see guard::allowed_test_browser_args)
        Ok(a) if !guard::allowed_test_browser_args(&a).is_empty() => builder.additional_browser_args(&guard::allowed_test_browser_args(&a)),
        _ => builder,
    };
    builder.build()?;
    Ok(())
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            inner: Mutex::new(Inner::default()),
            boot_cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
            chat_cancel: AtomicBool::new(false), busy: AtomicBool::new(false),
            last_active: Mutex::new(Instant::now()),
            machine: Mutex::new(None),
            seq: AtomicU64::new(0),
        })
        .setup(|app| { create_main_window(app)?; diag(app.handle(), &format!("app started, version {}", env!("CARGO_PKG_VERSION")));
            bootstrap(app.handle().clone(), false); start_idle_watcher(app.handle().clone()); Ok(()) })
        .invoke_handler(tauri::generate_handler![get_status, pick_model_folder, retry, list_variants, choose_variant, set_compute, set_memory_mode, chat_stream, chat_cancel, save_report])
        .build(tauri::generate_context!())
        .expect("failed to build app");

    app.run(|h, e| {
        if let RunEvent::Exit = e {
            if let Some(mut ch) = h.state::<AppState>().inner.lock().unwrap().child.take() { backend::kill(&mut ch); }
        }
    });
}
