use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use bytes::Bytes;
use futures_util::StreamExt;
use cull_cache::Provider;
use crate::monitor::HitRateMonitor;
use crate::{compress_anthropic_request_reported, compress_openai_request_reported, CompressOpts, FidelityReport};

pub struct ProxyState {
    pub client: reqwest::Client,
    pub upstream: String, // e.g. "https://api.anthropic.com"
    pub opts: CompressOpts,
    pub monitors: Mutex<HashMap<u64, HitRateMonitor>>, // per-session hit-rate monitors (R5)
}

pub fn app(state: Arc<ProxyState>) -> Router {
    Router::new()
        .route("/v1/messages", post(handle_messages))
        .route("/v1/chat/completions", post(handle_chat))
        .with_state(state)
}

const FORWARD_HEADERS: &[&str] = &[
    "x-api-key", "authorization", "anthropic-version", "anthropic-beta", "content-type",
];

type CompressFn = fn(&serde_json::Value, &CompressOpts) -> (serde_json::Value, Option<FidelityReport>);

/// Stable per-session key: hash of `system` + the first message (both stable across a session).
fn session_id(req: &serde_json::Value) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    req.get("system").map(|s| s.to_string()).unwrap_or_default().hash(&mut h);
    req.get("messages").and_then(|m| m.as_array()).and_then(|a| a.first())
        .map(|m| m.to_string()).unwrap_or_default().hash(&mut h);
    h.finish()
}

/// Anthropic TTL regime from any `cache_control.ttl == "1h"` in the request (default 5m).
fn detect_anthropic_provider(req: &serde_json::Value) -> Provider {
    fn has_1h(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::Object(m) => {
                if m.get("ttl").and_then(|t| t.as_str()) == Some("1h") { return true; }
                m.values().any(has_1h)
            }
            serde_json::Value::Array(a) => a.iter().any(has_1h),
            _ => false,
        }
    }
    if has_1h(req) { Provider::Anthropic1h } else { Provider::Anthropic5m }
}

fn scan_u64(s: &str, key: &str) -> Option<u64> {
    let i = s.find(key)?;
    let rest = &s[i + key.len()..];
    let colon = rest.find(':')?;
    let after = &rest[colon + 1..];
    let digits: String = after.chars().skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Parse Anthropic cache usage from a (capped) response buffer — works for both the non-streaming
/// top-level `usage` and the streaming `message_start` event (both carry these keys).
fn parse_anthropic_usage(buf: &[u8]) -> Option<(u64, u64)> {
    let s = String::from_utf8_lossy(buf);
    Some((scan_u64(&s, "\"cache_read_input_tokens\"")?, scan_u64(&s, "\"cache_creation_input_tokens\"")?))
}

fn hit_rate(read: u64, creation: u64) -> Option<f64> {
    let denom = read + creation;
    if denom == 0 { None } else { Some(read as f64 / denom as f64) }
}

const USAGE_SCAN_CAP: usize = 2 * 1024 * 1024; // 2 MB best-effort scan window (fail-safe)

async fn handle_generic(
    state: Arc<ProxyState>,
    headers: HeaderMap,
    body_bytes: Bytes,
    upstream_path: &str,
    provider: Provider,
    compress_fn: CompressFn,
) -> Response {
    let parsed = serde_json::from_slice::<serde_json::Value>(&body_bytes).ok();
    let sid = parsed.as_ref().map(session_id);

    // R5: if this session is halted, do NOT compress — byte-exact passthrough.
    let halted = match sid {
        Some(id) => state.monitors.lock().ok().and_then(|m| m.get(&id).map(|x| x.halted())).unwrap_or(false),
        None => false,
    };

    let (forward_body, report) = match (&parsed, halted) {
        (Some(req_json), false) => {
            let (compressed, report) = compress_fn(req_json, &state.opts);
            (serde_json::to_vec(&compressed).unwrap_or_else(|_| body_bytes.to_vec()), report)
        }
        _ => (body_bytes.to_vec(), None), // unparseable OR halted -> forward original unchanged
    };

    let url = format!("{}{}", state.upstream.trim_end_matches('/'), upstream_path);
    let mut fwd = state.client.post(&url).body(forward_body);
    for name in FORWARD_HEADERS {
        if let Some(v) = headers.get(*name) { fwd = fwd.header(*name, v); }
    }

    match fwd.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut builder = Response::builder().status(status);
            for (k, v) in resp.headers().iter() {
                let kn = k.as_str();
                if kn == "content-length" || kn == "transfer-encoding" || kn == "connection" { continue; }
                builder = builder.header(k, v);
            }
            if let Some(r) = &report {
                builder = builder
                    .header("x-cull-input-tokens", r.input_tokens.to_string())
                    .header("x-cull-net-tokens", r.net_tokens.to_string())
                    .header("x-cull-dropped", r.dropped.to_string());
            }
            if halted { builder = builder.header("x-cull-halted", "1"); }

            // Tee: forward every chunk bit-exact while accumulating a capped copy to read `usage`.
            let state_tee = Arc::clone(&state);
            let upstream_stream = resp.bytes_stream();
            let body = Body::from_stream(async_stream::stream! {
                let mut buf: Vec<u8> = Vec::new();
                futures_util::pin_mut!(upstream_stream);
                while let Some(item) = upstream_stream.next().await {
                    if let Ok(chunk) = &item {
                        if buf.len() < USAGE_SCAN_CAP { buf.extend_from_slice(chunk); }
                    }
                    yield item;
                }
                if let (Some(id), Some((read, creation))) = (sid, parse_anthropic_usage(&buf)) {
                    if let Some(h) = hit_rate(read, creation) {
                        if let Ok(mut map) = state_tee.monitors.lock() {
                            map.entry(id).or_insert_with(|| HitRateMonitor::new(provider)).observe(h);
                        }
                    }
                }
            });
            builder.body(body)
                .unwrap_or_else(|_| (StatusCode::BAD_GATEWAY, "bad upstream response").into_response())
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("cull-proxy upstream error: {e}")).into_response(),
    }
}

async fn handle_messages(State(state): State<Arc<ProxyState>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let body_bytes: Bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("failed to read body: {e}")).into_response(),
    };
    let provider = serde_json::from_slice::<serde_json::Value>(&body_bytes)
        .map(|v| detect_anthropic_provider(&v)).unwrap_or(Provider::Anthropic5m);
    handle_generic(state, parts.headers, body_bytes, "/v1/messages", provider, compress_anthropic_request_reported).await
}

async fn handle_chat(State(state): State<Arc<ProxyState>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let body_bytes: Bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("failed to read body: {e}")).into_response(),
    };
    handle_generic(state, parts.headers, body_bytes, "/v1/chat/completions", Provider::OpenAi, compress_openai_request_reported).await
}
