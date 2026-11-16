//! Spawns and supervises the local inference server (llama.cpp `llama-server`).
//! The UI never blocks on this: callers run `prepare`/`launch`/`wait_ready` on a worker thread.
use crate::{bench, config::Config, hw::Hardware, plan::Bandwidth, select::*, HEADROOM, MIN_TOTAL_RAM};
use std::{
    fs::File,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// Why the backend cannot start right now.
#[derive(Debug, Clone, PartialEq)]
pub enum Prep {
    /// Model folder missing / moved / deleted / contains no MedGemma files -> show re-link dialog.
    NeedFolder(String),
    /// Anything else (too little RAM, missing engine files ...).
    Error(String),
}

pub const MSG_NOT_FOUND: &str = "Backend model files not found";

pub fn backend_dir(exe_dir: &Path, c: Compute) -> PathBuf {
    exe_dir.join("backend").join(if c == Compute::Gpu { "vulkan" } else { "cpu" })
}
pub fn server_exe(exe_dir: &Path, c: Compute) -> PathBuf {
    backend_dir(exe_dir, c).join(if cfg!(windows) { "llama-server.exe" } else { "llama-server" })
}

/// Validate the saved folder and decide which model + compute path to use *right now*.
/// Measures RAM and drive bandwidth (about a second) so the planner works from real numbers.
pub fn prepare(cfg: &Config, hw: &Hardware, exe_dir: &Path) -> Result<(Plan, Compute, String), Prep> {
    let dir = cfg.model_path();
    let probe_file = scan(&dir).ok().and_then(|inv| inv.files.iter().filter(|f| !f.is_mmproj).max_by_key(|f| f.size).map(|f| f.path.clone()));
    let bw = bench::measure(probe_file.as_deref());
    prepare_with(cfg, hw, exe_dir, bw)
}

pub fn prepare_with(cfg: &Config, hw: &Hardware, exe_dir: &Path, bw: Bandwidth) -> Result<(Plan, Compute, String), Prep> {
    if hw.total_ram < MIN_TOTAL_RAM {
        return Err(Prep::Error("This computer has less than 4 GB of installed RAM, which is below the minimum requirement.".into()));
    }
    let dir = cfg.model_path();
    if cfg.model_dir.trim().is_empty() || !dir.is_dir() {
        return Err(Prep::NeedFolder(MSG_NOT_FOUND.into()));
    }
    let inv = scan(&dir).map_err(|_| Prep::NeedFolder(MSG_NOT_FOUND.into()))?;
    if !inv.files.iter().any(|f| !f.is_mmproj) {
        return Err(Prep::NeedFolder(MSG_NOT_FOUND.into()));
    }

    let mut note = String::new();
    let mut compute = Compute::parse(&cfg.compute);
    if compute == Compute::Gpu {
        if !hw.gpu_ok { compute = Compute::Cpu; note = "GPU not available - using CPU.".into(); }
        else if !server_exe(exe_dir, Compute::Gpu).is_file() { compute = Compute::Cpu; note = "GPU engine not installed - using CPU.".into(); }
    }
    let mc = Machine { budget: hw.free_ram.saturating_sub(HEADROOM), compute, vram: hw.vram, bw };
    if !cfg.forced_model.is_empty() {
        if let Some(p) = plan_for(&inv, &cfg.forced_model, &mc) { return Ok((p, compute, note)); }
    }
    match choose(&inv, &mc) {
        Ok(p) => Ok((p, compute, note)),
        Err(SelectError::NoModels(m)) => Err(Prep::NeedFolder(format!("{MSG_NOT_FOUND}. {m}"))),
        Err(e @ SelectError::TooSlow { .. }) => Err(Prep::Error(e.to_string())),
    }
}

/// All installed variants with estimates for the in-app selector; second value = file the automatic policy would pick.
pub fn list_variants(cfg: &Config, hw: &Hardware, exe_dir: &Path) -> Result<(Vec<Plan>, Option<String>), String> {
    let dir = cfg.model_path();
    let inv = scan(&dir).map_err(|e| e.to_string())?;
    let probe_file = inv.files.iter().filter(|f| !f.is_mmproj).max_by_key(|f| f.size).map(|f| f.path.clone());
    let bw = bench::measure(probe_file.as_deref());
    let mut compute = Compute::parse(&cfg.compute);
    if compute == Compute::Gpu && (!hw.gpu_ok || !server_exe(exe_dir, Compute::Gpu).is_file()) { compute = Compute::Cpu; }
    let mc = Machine { budget: hw.free_ram.saturating_sub(HEADROOM), compute, vram: hw.vram, bw };
    let auto = choose(&inv, &mc).ok().and_then(|p| p.model.file_name().map(|n| n.to_string_lossy().to_string()));
    Ok((variants(&inv, &mc), auto))
}

pub struct Launched { pub child: Child, pub port: u16 }

/// "Colibri-style" memory behaviour comes from llama.cpp's own mechanisms (see README section 4):
///  * weights stay on disk/USB and are memory-mapped (mmap is llama.cpp's default; we never pass --no-mmap or --mlock),
///    so pages are read lazily, count as reclaimable file cache, and Windows can evict them under pressure;
///  * `--no-warmup` skips the full-model warm-up pass so the model becomes ready without touching every page;
///  * `--cache-ram 0` disables the server's extra prompt cache (keeps RAM use predictable);
///  * `--offline` forbids any network access inside llama.cpp (verified to exist in build b12244);
///  * GPU runs offload as many layers as VRAM allows (`-ngl`), so offloaded weights need no system RAM afterwards;
///  * the supervisor in the app UNLOADS the whole server when idle (memory modes), returning the RAM.
/// `optional` args are dropped automatically (see `start_ready`) if the bundled llama.cpp build rejects them.
pub fn launch(plan: &Plan, compute: Compute, exe_dir: &Path, log: &Path, token: &str, optional: bool) -> Result<Launched, String> {
    let exe = server_exe(exe_dir, compute);
    if !exe.is_file() { return Err(format!("Inference engine missing: {}", exe.display())); }
    let port = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?.local_addr().map_err(|e| e.to_string())?.port();
    if let Some(d) = log.parent() { let _ = std::fs::create_dir_all(d); }
    let out = File::create(log).map_err(|e| format!("log: {e}"))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;

    let mut cmd = Command::new(&exe);
    cmd.current_dir(exe.parent().unwrap())
        .arg("-m").arg(&plan.model)
        .args(["--host", "127.0.0.1", "--port", &port.to_string(), "--api-key", token])
        .args(["-c", &plan.ctx.to_string()])
        .args(["-np", "1"])               // ONE slot: otherwise llama-server splits the context across 4 slots
        // GPU: `auto` lets llama.cpp's own --fit use the REAL free VRAM; CPU: nothing offloaded
        .args(["-ngl", if compute == Compute::Gpu { "auto" } else { "0" }])
        .arg("--no-webui")
        .stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
    if let Some(m) = &plan.mmproj { cmd.arg("--mmproj").arg(m); }
    if optional { cmd.args(["--no-warmup", "--cache-ram", "0", "--offline"]); }
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000); } // CREATE_NO_WINDOW
    let child = cmd.spawn().map_err(|e| format!("Could not start inference engine: {e}"))?;
    kill_with_parent(&child);
    Ok(Launched { child, port })
}

/// Raw HTTP/1.0 GET to avoid pulling an HTTP client crate into the binary. Returns the status code.
pub fn http_status(port: u16, path: &str) -> Option<u16> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(1)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    s.set_write_timeout(Some(Duration::from_secs(2))).ok()?;
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes()).ok()?;
    let mut buf = [0u8; 32];
    let n = s.read(&mut buf).ok()?;
    let head = std::str::from_utf8(&buf[..n]).ok()?;
    head.split_whitespace().nth(1)?.parse().ok()
}

pub fn log_tail(log: &Path) -> String {
    let s = std::fs::read_to_string(log).unwrap_or_default();
    let t: Vec<&str> = s.lines().rev().take(12).collect();
    t.into_iter().rev().collect::<Vec<_>>().join("\n")
}

/// Block until /health is 200 (model loaded), the process dies, `cancel` is set, or timeout.
pub fn wait_ready(child: &mut Child, port: u16, cancel: &AtomicBool, timeout: Duration, log: &Path) -> Result<(), String> {
    let t0 = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) { return Err("cancelled".into()); }
        if let Ok(Some(st)) = child.try_wait() {
            return Err(format!("The inference engine stopped ({st}).\n{}", log_tail(log)));
        }
        if http_status(port, "/health") == Some(200) { return Ok(()); }
        if t0.elapsed() > timeout { return Err("Timed out while loading the model.".into()); }
        std::thread::sleep(Duration::from_millis(700));
    }
}

/// Launch + wait until the model is loaded. Retries once without the optional tuning flags if the
/// engine exits immediately complaining about an unknown/invalid argument (different llama.cpp version).
pub fn start_ready(plan: &Plan, compute: Compute, exe_dir: &Path, log: &Path, token: &str, cancel: &AtomicBool, timeout: Duration) -> Result<Launched, String> {
    for optional in [true, false] {
        let mut l = launch(plan, compute, exe_dir, log, token, optional)?;
        match wait_ready(&mut l.child, l.port, cancel, timeout, log) {
            Ok(()) => return Ok(l),
            Err(e) => {
                kill(&mut l.child);
                // any early exit with the tuning flags -> retry once without them (different llama.cpp build / combination)
                if optional && e.starts_with("The inference engine stopped") { continue; }
                return Err(e);
            }
        }
    }
    Err("unreachable".into())
}

pub fn kill(child: &mut Child) { let _ = child.kill(); let _ = child.wait(); }

/// Windows: put the server in a job object that is killed when this process dies for ANY reason,
/// so a crash/Task-Manager kill of the app can never leave a 20 GB orphan process holding RAM.
#[cfg(windows)]
fn kill_with_parent(child: &Child) {
    use std::{ffi::c_void, os::windows::io::AsRawHandle, sync::OnceLock};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::*;
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| unsafe {
        let Ok(h) = CreateJobObjectW(None, None) else { return 0 };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let _ = SetInformationJobObject(h, JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void, std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
        h.0 as usize
    });
    if job != 0 {
        unsafe { let _ = AssignProcessToJobObject(HANDLE(job as *mut c_void), HANDLE(child.as_raw_handle() as *mut c_void)); }
    }
}
#[cfg(not(windows))]
fn kill_with_parent(_: &Child) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GIB;
    fn hw(free_gb: u64) -> Hardware { Hardware { total_ram: 16 * GIB, free_ram: free_gb * GIB, ..Default::default() } }

    #[test] fn missing_folder_triggers_relink() {
        let cfg = Config { compute: "cpu".into(), model_dir: "/definitely/not/here".into(), ..Default::default() };
        assert_eq!(prepare_with(&cfg, &hw(8), Path::new("."), Bandwidth::conservative()).unwrap_err(), Prep::NeedFolder(MSG_NOT_FOUND.into()));
        let empty = Config::default();
        assert!(matches!(prepare_with(&empty, &hw(8), Path::new("."), Bandwidth::conservative()), Err(Prep::NeedFolder(_))));
    }
    #[test] fn folder_with_no_gguf_triggers_relink() {
        let d = std::env::temp_dir().join("mg_test_empty"); std::fs::create_dir_all(&d).unwrap();
        let cfg = Config { compute: "cpu".into(), model_dir: d.display().to_string(), ..Default::default() };
        assert!(matches!(prepare_with(&cfg, &hw(8), Path::new("."), Bandwidth::conservative()), Err(Prep::NeedFolder(_))));
    }
    #[test] fn under_4gb_is_refused() {
        let h = Hardware { total_ram: 3 * GIB, free_ram: 2 * GIB, ..Default::default() };
        assert!(matches!(prepare_with(&Config::default(), &h, Path::new("."), Bandwidth::conservative()), Err(Prep::Error(_))));
    }
    #[test] fn health_probe_parses_status() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap(); let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (i, s) in l.incoming().enumerate().take(2) {
                let mut s = s.unwrap(); crate::http::tests::drain_request(&mut s);
                let _ = s.write_all(if i == 0 { b"HTTP/1.1 503 Loading\r\n\r\n" } else { b"HTTP/1.1 200 OK\r\n\r\n{}" });
            }
        });
        assert_eq!(http_status(port, "/health"), Some(503));
        assert_eq!(http_status(port, "/health"), Some(200));
    }
}
