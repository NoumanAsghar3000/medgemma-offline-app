#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Thin native shell: owns config, model-folder validation/re-link, and the lifecycle of the
//! local llama.cpp server. All decisions live in `mg-core` (shared with the installer helper).
use mg_core::{backend, config::Config, hw};
use serde::Serialize;
use std::{
    path::PathBuf,
    process::Child,
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};
use tauri_plugin_dialog::DialogExt;

#[derive(Clone, Serialize, Default)]
struct Status {
    /// need_model_folder | loading | ready | error
    state: String,
    message: String,
    model_dir: String,
    compute: String,
    model: String,
    quant: String,
    family: String,
    multimodal: bool,
    ctx: u32,
    port: u16,
    token: String,
    note: String,
}

#[derive(Default)]
struct Inner { child: Option<Child>, status: Status }
struct AppState { inner: Mutex<Inner>, cancel: Mutex<Arc<AtomicBool>> }

fn exe_dir() -> PathBuf { std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default() }
fn user_cfg(app: &AppHandle) -> PathBuf { app.path().app_data_dir().unwrap_or_else(|_| exe_dir()).join("config.json") }
fn load_cfg(app: &AppHandle) -> Config {
    // user config (written when the folder is re-linked) wins over the installer's seed config
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

/// Stop any running server and (re)start the whole pipeline on a worker thread.
fn bootstrap(app: AppHandle) {
    let st = app.state::<AppState>();
    // cancel a previous wait loop and kill the old server
    {
        let mut c = st.cancel.lock().unwrap();
        c.store(true, Ordering::SeqCst);
        *c = Arc::new(AtomicBool::new(false));
    }
    if let Some(mut ch) = st.inner.lock().unwrap().child.take() { backend::kill(&mut ch); }
    let cancel = st.cancel.lock().unwrap().clone();

    std::thread::spawn(move || {
        let cfg = load_cfg(&app);
        let mut s = Status { model_dir: cfg.model_dir.clone(), compute: cfg.compute.clone(), ..Default::default() };
        let hw = hw::detect();
        let (plan, compute, note) = match backend::prepare(&cfg, &hw, &exe_dir()) {
            Ok(x) => x,
            Err(backend::Prep::NeedFolder(m)) => {
                s.state = "need_model_folder".into(); s.message = m; return publish(&app, s);
            }
            Err(backend::Prep::Error(m)) => { s.state = "error".into(); s.message = m; return publish(&app, s); }
        };
        s.state = "loading".into();
        s.model = plan.model.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        s.quant = plan.quant.clone(); s.family = plan.family_label.clone(); s.multimodal = plan.multimodal;
        s.ctx = plan.ctx; s.note = if note.is_empty() { plan.note.clone() } else { note };
        s.compute = if compute == mg_core::select::Compute::Gpu { "gpu".into() } else { "cpu".into() };
        s.message = format!("Loading {} ({}) - large models can take a few minutes from a USB drive...", s.family, s.quant);
        publish(&app, s.clone());

        let tok = token();
        let log = app.path().app_log_dir().unwrap_or_else(|_| exe_dir()).join("backend.log");
        let mut launched = match backend::launch(&plan, compute, &exe_dir(), &log, &tok) {
            Ok(l) => l,
            Err(e) => { s.state = "error".into(); s.message = e; return publish(&app, s); }
        };
        let port = launched.port;
        match backend::wait_ready(&mut launched.child, port, &cancel, Duration::from_secs(30 * 60), &log) {
            Ok(()) => {
                app.state::<AppState>().inner.lock().unwrap().child = Some(launched.child);
                s.state = "ready".into(); s.port = port; s.token = tok; s.message = String::new();
                publish(&app, s);
            }
            Err(e) => {
                backend::kill(&mut launched.child);
                if e == "cancelled" { return; }
                s.state = "error".into(); s.message = e; publish(&app, s);
            }
        }
    });
}

#[tauri::command]
fn get_status(state: State<AppState>) -> Status { state.inner.lock().unwrap().status.clone() }

/// Native folder picker. Validates the choice; only a folder with real MedGemma files is accepted.
#[tauri::command(async)]
fn pick_model_folder(app: AppHandle) -> Result<bool, String> {
    let Some(p) = app.dialog().file().set_title("Select the folder that contains the MedGemma model files").blocking_pick_folder() else {
        return Ok(false); // user cancelled
    };
    let dir = p.into_path().map_err(|e| e.to_string())?;
    let inv = mg_core::select::scan(&dir).map_err(|e| format!("Cannot read that folder: {e}"))?;
    if !inv.files.iter().any(|f| !f.is_mmproj && f.family.is_some()) {
        return Err("No MedGemma .gguf model files were found in that folder (looking for names containing 27b or 4b).".into());
    }
    let mut cfg = load_cfg(&app);
    cfg.model_dir = dir.display().to_string();
    cfg.save(&user_cfg(&app)).map_err(|e| format!("Could not save settings: {e}"))?; // persisted for future launches
    bootstrap(app);
    Ok(true)
}

#[tauri::command]
fn retry(app: AppHandle) { bootstrap(app); }

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { inner: Mutex::new(Inner::default()), cancel: Mutex::new(Arc::new(AtomicBool::new(false))) })
        .setup(|app| { bootstrap(app.handle().clone()); Ok(()) })
        .invoke_handler(tauri::generate_handler![get_status, pick_model_folder, retry])
        .build(tauri::generate_context!())
        .expect("failed to build app");

    app.run(|h, e| {
        if let RunEvent::Exit = e {
            if let Some(mut ch) = h.state::<AppState>().inner.lock().unwrap().child.take() { backend::kill(&mut ch); }
        }
    });
}
