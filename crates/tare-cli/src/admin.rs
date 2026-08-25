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

    let token = std::env::var("TARE_ADMIN_TOKEN")
        .map_err(|_| "TARE_ADMIN_TOKEN is required for admin requests".to_string())?;
    if token.is_empty() || token.contains(['\r', '\n']) {
        return Err("TARE_ADMIN_TOKEN must be non-empty and contain no newlines".to_string());
    }
    let request =
        format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nx-tare-admin-token: {token}\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write: {e}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("read: {e}"))?;

    // Split headers / body at the first blank line (HTTP/1.x separator).
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| "invalid HTTP response: no body separator".to_string())?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| "invalid HTTP response: malformed status line".to_string())?;
    match status {
        200..=299 => {}
        401 => return Err("proxy rejected TARE_ADMIN_TOKEN".to_string()),
        404 => {
            return Err(
                "proxy admin API is disabled; set TARE_ADMIN_TOKEN on the proxy".to_string(),
            )
        }
        _ => return Err(format!("proxy admin API returned HTTP {status}")),
    }

    serde_json::from_str(body.trim()).map_err(|e| format!("JSON parse: {e}"))
}
