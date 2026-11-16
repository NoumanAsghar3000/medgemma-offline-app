//! Minimal local HTTP client used to talk to llama-server over loopback.
//! Doing this in Rust (instead of fetch() in the webview) means: no CORS dependency, the API key never
//! reaches page JavaScript, and cancellation is under our control.
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// POST `body` (JSON) to /v1/chat/completions and call `on_event` with every SSE `data:` payload
/// (JSON text or "[DONE]"). Handles chunked transfer-encoding. Returns Err on HTTP/connection errors.
pub fn post_sse(port: u16, token: &str, body: &str, cancel: &AtomicBool, mut on_event: impl FnMut(String)) -> Result<(), String> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| format!("Cannot reach the local model server: {e}"))?;
    s.set_read_timeout(Some(Duration::from_millis(400))).ok();
    s.set_write_timeout(Some(Duration::from_secs(60))).ok();
    let req = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len());
    // ONE write (headers + body): fewer TCP segments, so a server that answers early (e.g. 401) never races a half-sent request
    let mut all = req.into_bytes(); all.extend_from_slice(body.as_bytes());
    s.write_all(&all).map_err(|e| format!("Send failed: {e}"))?;

    let mut raw: Vec<u8> = Vec::new();   // bytes after headers, still chunk-framed if chunked
    let mut headers_done = false; let mut status = 0u16; let mut chunked = false;
    let mut decoded: Vec<u8> = Vec::new();
    let mut line_buf: Vec<u8> = Vec::new();
    let mut err_body = String::new();
    let mut idle_since = Instant::now();
    let mut buf = [0u8; 16 * 1024];
    let mut done = false;

    while !done {
        if cancel.load(Ordering::SeqCst) { return Ok(()); }
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => { idle_since = Instant::now(); raw.extend_from_slice(&buf[..n]); }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                // prompt processing of large images/PDFs on CPU can take minutes before the first token
                if idle_since.elapsed() > Duration::from_secs(30 * 60) { return Err("The model stopped responding.".into()); }
                continue;
            }
            Err(e) => return Err(format!("Connection error: {e}")),
        }
        if !headers_done {
            if let Some(p) = find(&raw, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&raw[..p]).to_string();
                status = head.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                chunked = head.to_ascii_lowercase().contains("transfer-encoding: chunked");
                raw.drain(..p + 4); headers_done = true;
            } else { continue; }
        }
        // de-chunk
        if chunked {
            loop {
                let Some(p) = find(&raw, b"\r\n") else { break };
                let size_str = String::from_utf8_lossy(&raw[..p]).to_string();
                let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
                if size == 0 { done = true; raw.clear(); break; }
                if raw.len() < p + 2 + size + 2 { break; }          // wait for the whole chunk
                decoded.extend_from_slice(&raw[p + 2..p + 2 + size]);
                raw.drain(..p + 2 + size + 2);
            }
        } else { decoded.append(&mut raw); }

        // split into lines
        for b in decoded.drain(..) {
            if b == b'\n' {
                let line = String::from_utf8_lossy(&line_buf).trim().to_string(); line_buf.clear();
                if status != 200 { err_body.push_str(&line); continue; }
                if let Some(d) = line.strip_prefix("data:") { on_event(d.trim().to_string()); }
            } else { line_buf.push(b); }
        }
    }
    if status != 200 {
        if !line_buf.is_empty() { err_body.push_str(&String::from_utf8_lossy(&line_buf)); }
        return Err(format!("Model server error {status}: {}", err_body.chars().take(400).collect::<String>()));
    }
    if !line_buf.is_empty() { if let Some(d) = String::from_utf8_lossy(&line_buf).trim().strip_prefix("data:") { on_event(d.trim().to_string()); } }
    Ok(())
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> { h.windows(n.len()).position(|w| w == n) }

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::net::TcpListener;
    /// Read the WHOLE request (headers + Content-Length body) like a real server. Closing a socket with unread request bytes
    /// makes the kernel send a reset that can discard the response - which made these tests flaky under CPU load.
    pub(crate) fn drain_request(c: &mut TcpStream) {
        let mut buf = Vec::new(); let mut b = [0u8; 4096];
        loop {
            let n = c.read(&mut b).unwrap_or(0); if n == 0 { return; } buf.extend_from_slice(&b[..n]);
            if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..p]).to_ascii_lowercase();
                let want = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                if buf.len() >= p + 4 + want { return; }
            }
        }
    }
    fn serve(resp: Vec<Vec<u8>>) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap(); let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap(); drain_request(&mut c);
            for part in resp { c.write_all(&part).unwrap(); c.flush().unwrap(); std::thread::sleep(Duration::from_millis(30)); }
        });
        port
    }
    fn chunk(s: &str) -> Vec<u8> { format!("{:x}\r\n{}\r\n", s.len(), s).into_bytes() }

    #[test] fn decodes_chunked_sse_even_when_split_mid_event_and_mid_utf8() {
        let ev1 = "data: {\"a\":\"h\u{e9}llo\"}\n\n"; let ev2 = "data: [DONE]\n\n";
        let all = format!("{ev1}{ev2}"); let bytes = all.as_bytes(); let cut = bytes.iter().position(|b| *b == 0xC3).unwrap() + 1; // split inside the é
        let mut c1 = format!("{:x}\r\n", cut).into_bytes(); c1.extend_from_slice(&bytes[..cut]); c1.extend_from_slice(b"\r\n");
        let rest = &bytes[cut..]; let mut c2 = format!("{:x}\r\n", rest.len()).into_bytes(); c2.extend_from_slice(rest); c2.extend_from_slice(b"\r\n");
        let port = serve(vec![b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/event-stream\r\n\r\n".to_vec(), c1, c2, b"0\r\n\r\n".to_vec()]);
        let mut got = vec![]; post_sse(port, "t", "{}", &AtomicBool::new(false), |e| got.push(e)).unwrap();
        assert_eq!(got, vec!["{\"a\":\"h\u{e9}llo\"}".to_string(), "[DONE]".to_string()]);
    }
    #[test] fn surfaces_http_errors() {
        let body = "{\"error\":\"bad image\"}";
        let port = serve(vec![format!("HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).into_bytes()]);
        let e = post_sse(port, "t", "{}", &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(e.contains("400") && e.contains("bad image"), "{e}");
    }
    #[test] fn plain_non_chunked_stream() {
        let port = serve(vec![b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\ndata: {\"x\":1}\n\ndata: [DONE]\n\n".to_vec()]);
        let mut got = vec![]; post_sse(port, "t", "{}", &AtomicBool::new(false), |e| got.push(e)).unwrap();
        assert_eq!(got.len(), 2);
    }
}
