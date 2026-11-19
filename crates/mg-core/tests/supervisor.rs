//! End-to-end test of the backend supervisor against a FAKE llama-server (python script).
//! Runs on Linux/macOS dev machines; Windows CI runs the unit tests only.
#![cfg(unix)]
use mg_core::{backend, config::Config, hw::Hardware, http, plan::Bandwidth, GIB};
use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::Path, sync::atomic::AtomicBool, time::Duration};

const NVME: Bandwidth = Bandwidth { ram_gbps: 40.0, disk_mbps: 3000.0 };
const USB: Bandwidth = Bandwidth { ram_gbps: 40.0, disk_mbps: 200.0 };

const FAKE: &str = r###"#!/usr/bin/env python3
import sys, os, json, time
from http.server import BaseHTTPRequestHandler, HTTPServer
a = sys.argv[1:]
open(os.environ["FAKE_ARGS_OUT"], "w").write(" ".join(a))
if os.environ.get("FAKE_REJECT") and "--cache-ram" in a:
    sys.stderr.write("error: invalid argument: --cache-ram\n"); sys.exit(1)
port = int(a[a.index("--port") + 1]); key = a[a.index("--api-key") + 1]
time.sleep(0.5)
class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *x): pass
    def do_GET(self):
        self.send_response(200); self.send_header("Content-Length", "15"); self.end_headers(); self.wfile.write(b'{"status":"ok"}')
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0)); body = json.loads(self.rfile.read(n))
        if self.headers.get("Authorization") != "Bearer " + key:
            self.send_response(401); self.send_header("Content-Length", "2"); self.end_headers(); self.wfile.write(b"{}"); return
        self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.send_header("Transfer-Encoding", "chunked"); self.end_headers()
        has_img = "image_url" in json.dumps(body)
        for w in ["## Findings\n", "- img=%s\n" % has_img, "## Impression\n", "- ok\n"]:
            d = ("data: " + json.dumps({"choices": [{"delta": {"content": w}}]}) + "\n\n").encode()
            self.wfile.write(b"%x\r\n%s\r\n" % (len(d), d)); self.wfile.flush()
        d = b"data: [DONE]\n\n"; self.wfile.write(b"%x\r\n%s\r\n0\r\n\r\n" % (len(d), d)); self.wfile.flush()
HTTPServer(("127.0.0.1", port), H).serve_forever()
"###;

const MODEL_HDR: &[u8] = include_bytes!("data/tiny-gemma3.gguf");
const CLIP_HDR: &[u8] = include_bytes!("data/tiny-clip-64.gguf");
/// A large sparse file that starts with a REAL GGUF header (the validator reads headers), tail = zeros.
fn sparse(p: &Path, bytes: u64) {
    let hdr = if p.file_name().unwrap().to_string_lossy().contains("mmproj") { CLIP_HDR } else { MODEL_HDR };
    let mut f = fs::File::create(p).unwrap(); f.write_all(hdr).unwrap(); f.set_len(bytes).unwrap();
}

fn setup(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("mg_it_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let models = root.join("models"); let app = root.join("app"); let be = app.join("backend/cpu");
    fs::create_dir_all(&models).unwrap(); fs::create_dir_all(&be).unwrap();
    sparse(&models.join("medgemma-27b-it-Q4_K_M.gguf"), 16 * GIB); sparse(&models.join("medgemma-27b-it-Q3_K_M.gguf"), 13 * GIB);
    sparse(&models.join("mmproj-medgemma-27b-it-F16.gguf"), GIB); sparse(&models.join("medgemma-4b-it-Q4_K_M.gguf"), 3 * GIB);
    sparse(&models.join("mmproj-medgemma-4b-it-F16.gguf"), GIB);
    let exe = be.join("llama-server"); fs::File::create(&exe).unwrap().write_all(FAKE.as_bytes()).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    (models, app)
}

fn run(name: &str, free_gb: u64, reject: bool, bw: Bandwidth) -> (String, String, Vec<String>) {
    let (models, app) = setup(name);
    let args_out = app.join("args.txt");
    std::env::set_var("FAKE_ARGS_OUT", &args_out);
    if reject { std::env::set_var("FAKE_REJECT", "1"); } else { std::env::remove_var("FAKE_REJECT"); }
    let cfg = Config { compute: "cpu".into(), model_dir: models.display().to_string(), ..Default::default() };
    let hw = Hardware { total_ram: 32 * GIB, free_ram: free_gb * GIB, ..Default::default() };
    let (plan, compute, _) = backend::prepare_with(&cfg, &hw, &app, bw).expect("prepare");
    let log = app.join("backend.log");
    let mut l = backend::start_ready(&plan, compute, &app, &log, "secret", &AtomicBool::new(false), Duration::from_secs(20)).expect("start_ready");
    let mut ev = vec![];
    http::post_sse(l.port, "secret", r#"{"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/jpeg;base64,AAAA"}}]}]}"#, &AtomicBool::new(false), |e| ev.push(e)).expect("chat");
    assert!(http::post_sse(l.port, "WRONG", "{}", &AtomicBool::new(false), |_| {}).unwrap_err().contains("401"));
    backend::kill(&mut l.child);
    // The fixtures are ONE tiny F32 header renamed to fake quantization levels. The header (content) decides the precision label,
    // so the label would read "F32" for every file; what these tests prove is WHICH FILE is chosen, so they compare file names.
    (plan.model.file_name().unwrap().to_string_lossy().to_string(), fs::read_to_string(&args_out).unwrap(), ev)
}

#[test] fn picks_27b_launches_with_tuning_flags_and_streams() {
    let (file, args, ev) = run("a", 20, false, NVME);   // 20 GB free -> budget 19 -> Q4_K_M (16+1+1.5=18.5) fits
    assert_eq!(file, "medgemma-27b-it-Q4_K_M.gguf");
    for f in ["-np 1", "--mmproj", "--no-webui", "--no-warmup", "--cache-ram 0", "--offline", "--host 127.0.0.1", "-ngl 0"] { assert!(args.contains(f), "{f} missing in {args}"); }
    assert!(!args.contains("--no-mmap") && !args.contains("--mlock"));
    assert_eq!(ev.len(), 5); assert!(ev[1].contains("img=True")); assert_eq!(ev[4], "[DONE]");
}
#[test] fn smaller_free_ram_steps_down_the_quant() {
    assert_eq!(run("b", 17, false, USB).0, "medgemma-27b-it-Q3_K_M.gguf");   // slow drive: Q4 would stream 2.5 GB/token (0.08 tok/s) -> Q3 stays resident
}
#[test] fn retries_without_optional_flags_when_engine_rejects_them() {
    let (_, args, ev) = run("c", 20, true, NVME);
    assert!(!args.contains("--cache-ram") && !args.contains("--no-warmup"), "{args}");
    assert_eq!(ev.len(), 5);
}
