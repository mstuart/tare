//! Tiny std-only HTTP helper for the local proxy admin endpoints.
//!
//! Uses `TcpStream` directly — no external HTTP crates required.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// GET `path` from the proxy admin surface on `127.0.0.1:port`.
///
/// Returns the parsed JSON body or an error string suitable for direct user display.
pub fn admin_get(port: u16, path: &str) -> Result<serde_json::Value, String> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|_| format!("proxy not running on :{port}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| format!("set_read_timeout: {e}"))?;

    let request = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write: {e}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("read: {e}"))?;

    // Split headers / body at the first blank line (HTTP/1.x separator).
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .ok_or_else(|| "invalid HTTP response: no body separator".to_string())?;

    serde_json::from_str(body.trim()).map_err(|e| format!("JSON parse: {e}"))
}
