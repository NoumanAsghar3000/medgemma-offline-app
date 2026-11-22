//! Input validation for everything the webview (untrusted from the shell's point of view) can hand to native code.
//! Pure functions, no I/O, so they are unit-tested on every platform.
//!
//! Why this exists: the webview talks to the local inference server through the shell (`chat_stream`). Without a gate, anything that
//! runs in the page (a future XSS, a malicious dependency) could make the shell forward ARBITRARY JSON to llama-server: remote or
//! `file://` image URLs, grammars, LoRA/slot controls, unbounded `max_tokens`, multi-GB bodies. The gate below forwards only what the
//! UI legitimately sends (text + embedded images + two sampling knobs) and rejects the rest.
use serde_json::{json, Map, Value};

/// Largest request body the shell will forward (base64 images + document text).
pub const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
const MAX_MESSAGES: usize = 256;
const MAX_IMAGES: usize = 16;
const MAX_IMAGE_B64: usize = 24 * 1024 * 1024;
const MAX_TOKENS_CAP: u64 = 8192;
const DEFAULT_MAX_TOKENS: u64 = 2048;

/// Validate the OpenAI-style chat body built by the UI and return a re-serialised, allow-listed copy.
/// Allowed: `messages` (roles system/user/assistant; text parts and `data:image/{jpeg,png,webp,gif};base64,` images only),
/// `temperature` (clamped 0..2) and `max_tokens` (clamped 1..8192). `stream` is always forced to `true`. Everything else is dropped.
pub fn sanitize_chat_body(body: &str) -> Result<String, String> {
    if body.len() > MAX_BODY_BYTES { return Err("The request is too large.".into()); }
    let v: Value = serde_json::from_str(body).map_err(|_| "The request is not valid JSON.".to_string())?;
    let obj = v.as_object().ok_or("The request must be a JSON object.")?;
    let msgs = obj.get("messages").and_then(Value::as_array).ok_or("The request has no message list.")?;
    if msgs.is_empty() || msgs.len() > MAX_MESSAGES { return Err("The request has an invalid number of messages.".into()); }

    let mut images = 0usize;
    let mut clean = Vec::with_capacity(msgs.len());
    for m in msgs {
        let mo = m.as_object().ok_or("Each message must be an object.")?;
        let role = mo.get("role").and_then(Value::as_str).unwrap_or("");
        if !matches!(role, "system" | "user" | "assistant") { return Err("A message has a role that is not allowed.".into()); }
        let content = match mo.get("content") {
            Some(Value::String(s)) => Value::String(s.clone()),
            Some(Value::Array(parts)) => {
                let mut out = Vec::with_capacity(parts.len());
                for p in parts {
                    let po = p.as_object().ok_or("Each content part must be an object.")?;
                    match po.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let t = po.get("text").and_then(Value::as_str).ok_or("A text part has no text.")?;
                            out.push(json!({ "type": "text", "text": t }));
                        }
                        Some("image_url") => {
                            let url = po.get("image_url").and_then(|i| i.get("url")).and_then(Value::as_str).ok_or("An image part has no URL.")?;
                            check_image_data_url(url)?;
                            images += 1;
                            if images > MAX_IMAGES { return Err("Too many images in one request.".into()); }
                            out.push(json!({ "type": "image_url", "image_url": { "url": url } }));
                        }
                        _ => return Err("A message contains an unsupported content part.".into()),
                    }
                }
                Value::Array(out)
            }
            _ => return Err("A message has no usable content.".into()),
        };
        clean.push(json!({ "role": role, "content": content }));
    }

    let mut out = Map::new();
    out.insert("messages".into(), Value::Array(clean));
    out.insert("stream".into(), Value::Bool(true));
    if let Some(t) = obj.get("temperature").and_then(Value::as_f64).filter(|t| t.is_finite()) {
        out.insert("temperature".into(), json!(t.clamp(0.0, 2.0)));
    }
    let mt = obj.get("max_tokens").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_TOKENS).clamp(1, MAX_TOKENS_CAP);
    out.insert("max_tokens".into(), json!(mt));
    serde_json::to_string(&Value::Object(out)).map_err(|e| e.to_string())
}

/// Only images that travel INSIDE the request are accepted. `http(s):` and `file:` URLs would make the server fetch or read
/// something on the model's behalf (SSRF / local file read, depending on the llama.cpp build and flags).
fn check_image_data_url(url: &str) -> Result<(), String> {
    let rest = url.strip_prefix("data:image/").ok_or("Only embedded images (data: URLs) are allowed.")?;
    let (mime, b64) = rest.split_once(";base64,").ok_or("Only base64 image data is allowed.")?;
    if !matches!(mime, "jpeg" | "png" | "webp" | "gif") { return Err("This image type is not allowed.".into()); }
    if b64.is_empty() || b64.len() > MAX_IMAGE_B64 { return Err("An image has an invalid size.".into()); }
    if !b64.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'=') { return Err("An image is not valid base64.".into()); }
    Ok(())
}

/// Largest report the shell will write to disk.
pub const MAX_REPORT_BYTES: usize = 100 * 1024 * 1024;

/// The report is a PDF produced by the UI. Refuse to write anything else to disk on the webview's behalf.
pub fn check_report_bytes(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_REPORT_BYTES { return Err("The report is too large.".into()); }
    if !bytes.starts_with(b"%PDF-") { return Err("The report data is not a PDF document.".into()); }
    Ok(())
}

/// Suggested file name for the Save dialog: a bare, printable name that ends in `.pdf` (no directories, no reserved characters).
pub fn safe_report_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base.chars().filter(|c| !c.is_control() && !"<>:\"|?*".contains(*c)).take(80).collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    let stem = if cleaned.is_empty() { "MedicalReport".to_string() } else { cleaned };
    if stem.to_ascii_lowercase().ends_with(".pdf") { stem } else { format!("{stem}.pdf") }
}

/// The CI/e2e hook `MG_TEST_BROWSER_ARGS` is passed to WebView2 (Chromium). Chromium flags can disable its sandbox or run a command
/// (`--renderer-cmd-prefix`, `--utility-cmd-prefix`, ...), so only the exact flags the test suite needs are let through.
pub fn allowed_test_browser_args(raw: &str) -> String {
    let ok = |a: &str| -> bool {
        matches!(a, "--remote-allow-origins=*" | "--use-fake-device-for-media-stream" | "--use-fake-ui-for-media-stream")
            || a.strip_prefix("--remote-debugging-port=").map(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|c| c.is_ascii_digit())).unwrap_or(false)
            || a.strip_prefix("--disable-features=").map(|f| !f.is_empty() && f.bytes().all(|c| c.is_ascii_alphanumeric() || c == b',')).unwrap_or(false)
    };
    raw.split_whitespace().filter(|a| ok(a)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value { serde_json::from_str(s).unwrap() }
    const JPG: &str = "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQ==";

    #[test] fn accepts_what_the_ui_sends_and_forces_streaming() {
        let body = format!(r#"{{"messages":[{{"role":"system","content":"s"}},{{"role":"user","content":[{{"type":"text","text":"hi"}},{{"type":"image_url","image_url":{{"url":"{JPG}"}}}}]}}],"stream":false,"temperature":0.2,"max_tokens":2048}}"#);
        let o = v(&sanitize_chat_body(&body).unwrap());
        assert_eq!(o["stream"], true);
        assert_eq!(o["max_tokens"], 2048);
        assert_eq!(o["temperature"], 0.2);
        assert_eq!(o["messages"][1]["content"][1]["image_url"]["url"], JPG);
    }
    #[test] fn drops_unknown_top_level_fields() {
        let o = v(&sanitize_chat_body(r#"{"messages":[{"role":"user","content":"x"}],"grammar":"root ::= x","lora":[{"id":0}],"cache_prompt":true,"id_slot":3,"model":"/etc/x"}"#).unwrap());
        let keys: Vec<_> = o.as_object().unwrap().keys().cloned().collect();
        assert!(keys.iter().all(|k| ["messages", "stream", "max_tokens", "temperature"].contains(&k.as_str())), "{keys:?}");
    }
    #[test] fn clamps_sampling_and_length() {
        let o = v(&sanitize_chat_body(r#"{"messages":[{"role":"user","content":"x"}],"max_tokens":99999999,"temperature":50}"#).unwrap());
        assert_eq!(o["max_tokens"], 8192); assert_eq!(o["temperature"], 2.0);
        let o = v(&sanitize_chat_body(r#"{"messages":[{"role":"user","content":"x"}],"max_tokens":0}"#).unwrap());
        assert_eq!(o["max_tokens"], 1);
        let o = v(&sanitize_chat_body(r#"{"messages":[{"role":"user","content":"x"}]}"#).unwrap());
        assert_eq!(o["max_tokens"], 2048);
    }
    #[test] fn rejects_remote_and_local_file_image_urls() {
        for url in ["http://127.0.0.1:8080/x.png", "https://example.com/a.png", "file:///C:/Windows/win.ini", "data:text/html;base64,AAAA", "data:image/svg+xml;base64,AAAA", "data:image/jpeg,AAAA", "data:image/jpeg;base64,AA AA", "data:image/jpeg;base64,", "/etc/passwd"] {
            let body = format!(r#"{{"messages":[{{"role":"user","content":[{{"type":"image_url","image_url":{{"url":"{url}"}}}}]}}]}}"#);
            assert!(sanitize_chat_body(&body).is_err(), "{url} must be rejected");
        }
    }
    #[test] fn rejects_bad_roles_parts_and_shapes() {
        for body in [
            r#"{"messages":[{"role":"tool","content":"x"}]}"#,
            r#"{"messages":[{"role":"user","content":[{"type":"input_audio","input_audio":{}}]}]}"#,
            r#"{"messages":[{"role":"user"}]}"#,
            r#"{"messages":[]}"#,
            r#"{"nomessages":1}"#,
            r#"[1,2]"#,
            r#"not json"#,
        ] { assert!(sanitize_chat_body(body).is_err(), "{body}"); }
    }
    #[test] fn caps_image_count_and_body_size() {
        let part = format!(r#"{{"type":"image_url","image_url":{{"url":"{JPG}"}}}}"#);
        let many = vec![part.clone(); MAX_IMAGES + 1].join(",");
        assert!(sanitize_chat_body(&format!(r#"{{"messages":[{{"role":"user","content":[{many}]}}]}}"#)).is_err());
        let ok = vec![part; MAX_IMAGES].join(",");
        assert!(sanitize_chat_body(&format!(r#"{{"messages":[{{"role":"user","content":[{ok}]}}]}}"#)).is_ok());
        let huge = "a".repeat(MAX_BODY_BYTES + 1);
        assert!(sanitize_chat_body(&huge).is_err());
    }
    #[test] fn report_bytes_must_be_a_pdf() {
        assert!(check_report_bytes(b"%PDF-1.7\n...").is_ok());
        assert!(check_report_bytes(b"MZ\x90\x00 this is an exe").is_err());
        assert!(check_report_bytes(b"<script>alert(1)</script>").is_err());
        assert!(check_report_bytes(b"").is_err());
    }
    #[test] fn report_name_is_bare_and_ends_in_pdf() {
        assert_eq!(safe_report_name("MedicalReport_202611221430.pdf"), "MedicalReport_202611221430.pdf");
        assert_eq!(safe_report_name("..\\..\\Windows\\evil"), "evil.pdf");
        assert_eq!(safe_report_name("C:/Users/x/a:b*c?.PDF"), "abc.PDF");
        assert_eq!(safe_report_name("   "), "MedicalReport.pdf");
        assert_eq!(safe_report_name("\u{0}\n"), "MedicalReport.pdf");
        assert!(safe_report_name(&"x".repeat(500)).len() <= 84);
    }
    #[test] fn test_browser_args_are_allow_listed() {
        let ci = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port=9333 --remote-allow-origins=* --use-fake-device-for-media-stream --use-fake-ui-for-media-stream";
        assert_eq!(allowed_test_browser_args(ci), ci);
        let evil = "--remote-debugging-port=9333 --renderer-cmd-prefix=calc.exe --no-sandbox --disable-web-security --remote-debugging-port=abc --remote-debugging-address=0.0.0.0 --disable-features=a;b";
        assert_eq!(allowed_test_browser_args(evil), "--remote-debugging-port=9333");
    }
}
