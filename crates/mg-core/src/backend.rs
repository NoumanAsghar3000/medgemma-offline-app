//! Spawns and supervises the local inference server (llama.cpp `llama-server`).
//! The UI never blocks on this: callers run `prepare`/`launch`/`wait_ready` on a worker thread.
use crate::{config::Config, hw::Hardware, select::*, HEADROOM, MIN_TOTAL_RAM};
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
pub fn prepare(cfg: &Config, hw: &Hardware, exe_dir: &Path) -> Result<(Plan, Compute, String), Prep> {
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
    let budget = hw.free_ram.saturating_sub(HEADROOM);
    match choose(&inv, budget, compute, hw.vram) {
        Ok(p) => Ok((p, compute, note)),
        Err(SelectError::NoModels(m)) => Err(Prep::NeedFolder(format!("{MSG_NOT_FOUND}. {m}"))),
        Err(e @ SelectError::TooLittleRam { .. }) => Err(Prep::Error(e.to_string())),
    }
}

pub struct Launched { pub child: Child, pub port: u16 }

pub fn launch(plan: &Plan, compute: Compute, exe_dir: &Path, log: &Path, token: &str) -> Result<Launched, String> {
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
        .args(["-ngl", &plan.n_gpu_layers.to_string()])
        .arg("--no-webui")
        .stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
    if let Some(m) = &plan.mmproj { cmd.arg("--mmproj").arg(m); }
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
        let cfg = Config { compute: "cpu".into(), model_dir: "/definitely/not/here".into() };
        assert_eq!(prepare(&cfg, &hw(8), Path::new(".")).unwrap_err(), Prep::NeedFolder(MSG_NOT_FOUND.into()));
        let empty = Config::default();
        assert!(matches!(prepare(&empty, &hw(8), Path::new(".")), Err(Prep::NeedFolder(_))));
    }
    #[test] fn folder_with_no_gguf_triggers_relink() {
        let d = std::env::temp_dir().join("mg_test_empty"); std::fs::create_dir_all(&d).unwrap();
        let cfg = Config { compute: "cpu".into(), model_dir: d.display().to_string() };
        assert!(matches!(prepare(&cfg, &hw(8), Path::new(".")), Err(Prep::NeedFolder(_))));
    }
    #[test] fn under_4gb_is_refused() {
        let h = Hardware { total_ram: 3 * GIB, free_ram: 2 * GIB, ..Default::default() };
        assert!(matches!(prepare(&Config::default(), &h, Path::new(".")), Err(Prep::Error(_))));
    }
    #[test] fn health_probe_parses_status() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap(); let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (i, s) in l.incoming().enumerate().take(2) {
                let mut s = s.unwrap(); let mut b = [0u8; 256]; let _ = s.read(&mut b);
                let _ = s.write_all(if i == 0 { b"HTTP/1.1 503 Loading\r\n\r\n" } else { b"HTTP/1.1 200 OK\r\n\r\n{}" });
            }
        });
        assert_eq!(http_status(port, "/health"), Some(503));
        assert_eq!(http_status(port, "/health"), Some(200));
    }
}
