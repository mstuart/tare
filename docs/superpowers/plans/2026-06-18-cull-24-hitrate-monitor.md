# Cull — Plan 24: Provider-Aware Hit-Rate-Floor Monitor (R5 + R6)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Close §8 Rule 5 (hit-rate floor: monitor `cache_read`/`cache_creation`; halt compression after 3+ consecutive turns below the provider floor) and Rule 6 (thresholds parameterized by the detected provider's `W`/`R`/TTL), plus the §10 "State: per-session" requirement.

**Architecture:** The provider floor is exactly `(W−1)/(W−R)` — the break-even hit rate already in `CacheModel`. Add `CacheModel::hit_rate_floor()` (R6) and a per-session `HitRateMonitor` that halts after 3 consecutive sub-floor turns (R5). The proxy gains per-session state (`Mutex<HashMap<u64, HitRateMonitor>>`, keyed by a stable hash of `system` + first message), detects the provider by route (Anthropic via `/v1/messages`, OpenAI via `/v1/chat/completions`; Anthropic 5m-vs-1h from any `cache_control.ttl`), checks the session's halt flag before compressing (halted → byte-exact passthrough), and **tees** the response stream — forwarding every chunk bit-exact while scanning a capped copy for `usage` to feed the monitor.

**Tech:** Rust, serde_json, axum 0.7, reqwest 0.12 stream, `async-stream`, `futures-util`. New dep `cull-cache` in the proxy. Reference: spec §8 Rules 5/6, §10 State, §11 (cache-impact reporting).

**Honest limitation (documented, not silent):** response `usage` is scanned within a 2 MB cap. Anthropic streaming puts cache tokens in the early `message_start` event, and non-streaming bodies are almost always < 2 MB, so this covers the real cases; beyond the cap the turn is simply not counted — **fail-safe (never falsely halts)**. OpenAI reports cached tokens in a different shape (`prompt_tokens_details.cached_tokens`) and has a ~0 floor, so halting effectively never fires for OpenAI; the Anthropic keys are scanned and OpenAI degrades to no-op (noted in the ledger).

---

### Task 1: `hit_rate_floor()` + `HitRateMonitor` (pure logic)

**Files:** modify `crates/cull-cache/src/lib.rs`; create `crates/cull-proxy/src/monitor.rs`; modify `crates/cull-proxy/src/lib.rs` (`pub mod monitor;`) and `crates/cull-proxy/Cargo.toml` (add `cull-cache`).

- [ ] **Step 1 — failing test in cull-cache.** Add to the `tests` module in `crates/cull-cache/src/lib.rs`:
```rust
    #[test]
    fn hit_rate_floor_matches_break_even() {
        let a5 = CacheModel::for_provider(Provider::Anthropic5m);
        assert!((a5.hit_rate_floor() - 0.21739).abs() < 1e-4); // (1.25-1)/(1.25-0.1)
        let a1 = CacheModel::for_provider(Provider::Anthropic1h);
        assert!((a1.hit_rate_floor() - 0.52632).abs() < 1e-4); // (2-1)/(2-0.1)
        let oa = CacheModel::for_provider(Provider::OpenAi);
        assert!(oa.hit_rate_floor().abs() < 1e-9);             // (1-1)/(1-0.1) = 0
    }
```

- [ ] **Step 2 — confirm FAIL** (`cargo test -p cull-cache hit_rate_floor` → no method).

- [ ] **Step 3 — implement in cull-cache.** Add to `impl CacheModel` (next to `caching_net_positive`):
```rust
    /// Provider hit-rate floor = break-even h below which caching is net-negative: (W-1)/(W-R).
    pub fn hit_rate_floor(&self) -> f64 {
        (self.write_mult - 1.0) / (self.write_mult - self.read_mult)
    }
```

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-cache`).

- [ ] **Step 5 — add the dep.** In `crates/cull-proxy/Cargo.toml` `[dependencies]`, add:
```toml
cull-cache = { path = "../cull-cache" }
```

- [ ] **Step 6 — failing test for the monitor.** Create `crates/cull-proxy/src/monitor.rs` with the test module first:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use cull_cache::Provider;

    #[test]
    fn halts_after_three_consecutive_below_floor() {
        let mut m = HitRateMonitor::new(Provider::Anthropic5m); // floor ~0.217
        assert!(!m.halted());
        m.observe(0.0); assert!(!m.halted());   // 1 below
        m.observe(0.1); assert!(!m.halted());   // 2 below
        m.observe(0.05);                          // 3 below -> halt
        assert!(m.halted());
    }

    #[test]
    fn a_hit_resets_the_streak() {
        let mut m = HitRateMonitor::new(Provider::Anthropic5m);
        m.observe(0.0); m.observe(0.0);          // 2 below
        m.observe(0.9);                           // a hit resets
        m.observe(0.0); m.observe(0.0);          // 2 below again
        assert!(!m.halted());                     // never hit 3 consecutive
    }

    #[test]
    fn openai_floor_zero_never_halts_on_positive_rate() {
        let mut m = HitRateMonitor::new(Provider::OpenAi); // floor 0
        for _ in 0..10 { m.observe(0.0001); }
        assert!(!m.halted());                     // 0.0001 is NOT below a 0 floor
    }

    #[test]
    fn stays_halted_once_tripped() {
        let mut m = HitRateMonitor::new(Provider::Anthropic5m);
        m.observe(0.0); m.observe(0.0); m.observe(0.0);
        assert!(m.halted());
        m.observe(0.99);                          // recovery does not auto-resume (spec: halt + diagnose)
        assert!(m.halted());
    }
}
```

- [ ] **Step 7 — confirm FAIL.**

- [ ] **Step 8 — implement the monitor** at the top of `monitor.rs`:
```rust
use cull_cache::{CacheModel, Provider};

/// Per-session cache hit-rate monitor (spec §8 Rule 5). Halts compression after `HALT_STREAK`
/// consecutive turns whose observed hit rate is strictly below the provider floor
/// (`CacheModel::hit_rate_floor`, R6). Once halted it stays halted — the operator diagnoses the
/// invalidation source (spec: "halt compression and diagnose"); it does not auto-resume.
const HALT_STREAK: u32 = 3;

#[derive(Debug)]
pub struct HitRateMonitor {
    floor: f64,
    consecutive_below: u32,
    halted: bool,
}

impl HitRateMonitor {
    pub fn new(provider: Provider) -> Self {
        Self { floor: CacheModel::for_provider(provider).hit_rate_floor(), consecutive_below: 0, halted: false }
    }

    /// Record one turn's observed hit rate.
    pub fn observe(&mut self, hit_rate: f64) {
        if hit_rate < self.floor {
            self.consecutive_below += 1;
            if self.consecutive_below >= HALT_STREAK { self.halted = true; }
        } else {
            self.consecutive_below = 0;
        }
    }

    pub fn halted(&self) -> bool { self.halted }
}
```

- [ ] **Step 9 — wire the module.** Add `pub mod monitor;` to `crates/cull-proxy/src/lib.rs` (near `pub mod server;`).

- [ ] **Step 10 — confirm PASS** (`cargo test -p cull-proxy monitor` + `cargo test --workspace`).

- [ ] **Step 11 — commit.** `git add crates/cull-cache crates/cull-proxy && git commit -m "feat(cache,proxy): hit_rate_floor + HitRateMonitor (R5/R6 core)"`

---

### Task 2: Wire the monitor into the proxy (provider detect, session state, halt-aware compress, stream tee)

**Files:** modify `crates/cull-proxy/src/server.rs`, `crates/cull-proxy/src/main.rs`, `crates/cull-proxy/Cargo.toml`, `crates/cull-proxy/tests/proxy.rs`.

- [ ] **Step 1 — add deps.** In `crates/cull-proxy/Cargo.toml` `[dependencies]`:
```toml
async-stream = "0.3"
futures-util = "0.3"
```

- [ ] **Step 2 — failing integration test.** Append to `crates/cull-proxy/tests/proxy.rs` (own Vec recorder so we can compare turn 1 vs turn 4):
```rust
#[tokio::test]
async fn halts_compression_after_three_low_hit_rate_turns() {
    use std::sync::{Arc, Mutex};
    use axum::{routing::post, Router, extract::State, body::Bytes};
    use cull_proxy::{server::{app, ProxyState}, CompressOpts};

    // mock upstream: records every received body, returns a LOW cache hit-rate usage (h ~= 0.001)
    type Rec = Arc<Mutex<Vec<String>>>;
    async fn up(State(rec): State<Rec>, body: Bytes) -> &'static str {
        rec.lock().unwrap().push(String::from_utf8_lossy(&body).into_owned());
        "{\"ok\":true,\"usage\":{\"cache_read_input_tokens\":1,\"cache_creation_input_tokens\":1000}}"
    }
    let rec: Rec = Arc::new(Mutex::new(Vec::new()));
    let upstream = Router::new().route("/v1/messages", post(up)).with_state(rec.clone());
    let up_port = spawn(upstream).await;

    let state = Arc::new(ProxyState {
        client: reqwest::Client::new(),
        upstream: format!("http://127.0.0.1:{up_port}"),
        opts: CompressOpts { enabled: true, recency_keep: 1, min_savings: 0 },
        monitors: Default::default(),
    });
    let proxy_port = spawn(app(state)).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let req = serde_json::json!({
        "model":"claude-x","max_tokens":100,
        "messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"run","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"kafka partitions offsets totally unrelated junk"}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"jwt authentication middleware verify"}]},
            {"role":"user","content":"fix the authentication jwt bug"}
        ]
    });
    let client = reqwest::Client::new();
    let mut last_halted = false;
    for _ in 0..4 {
        let resp = client.post(format!("http://127.0.0.1:{proxy_port}/v1/messages"))
            .header("x-api-key","sk-test").json(&req).send().await.unwrap();
        last_halted = resp.headers().get("x-cull-halted").is_some();
        let _ = resp.text().await.unwrap(); // drain so the tee's monitor update completes
    }
    let bodies = rec.lock().unwrap().clone();
    assert_eq!(bodies.len(), 4);
    // turn 1: compression active -> irrelevant kafka tool_result stubbed away
    assert!(!bodies[0].contains("kafka partitions offsets totally unrelated"),
        "turn 1 should be compressed: {}", bodies[0]);
    // turn 4: monitor halted after 3 sub-floor turns -> byte-exact passthrough (kafka intact)
    assert!(bodies[3].contains("kafka partitions offsets totally unrelated"),
        "turn 4 should be uncompressed passthrough: {}", bodies[3]);
    assert!(last_halted, "turn 4 response carries x-cull-halted");
}
```

- [ ] **Step 3 — confirm FAIL to compile** (`ProxyState` has no `monitors`; `x-cull-halted` not emitted).

- [ ] **Step 4 — extend `ProxyState` + provider plumbing in `server.rs`.** Replace the imports/struct/app/handlers region (lines 1–97) with:
```rust
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
```

- [ ] **Step 5 — fix the other `ProxyState` construction site.** In `crates/cull-proxy/src/main.rs`, add `monitors: Default::default(),` to the `ProxyState { … }` literal (read main.rs first; match its field order/style). If `Mutex<HashMap<..>>` lacks `Default`, it does — `Mutex<T: Default>` is `Default` — so `Default::default()` works.

- [ ] **Step 6 — fix the existing integration test's construction.** In the FIRST test in `tests/proxy.rs` (`proxy_compresses_then_forwards_and_returns_upstream_response`), add `monitors: Default::default(),` to its `ProxyState { … }` literal.

- [ ] **Step 7 — confirm PASS** (`cargo test -p cull-proxy` — the new halt test + the existing ones). Then `cargo test --workspace`.

- [ ] **Step 8 — self-review.** Confirm: forwarded chunks are yielded unchanged (only copied, never mutated) → fidelity intact; the monitor lock is never held across an `.await`; halted sessions skip compression entirely; `x-cull-halted` only on halted turns.

- [ ] **Step 9 — commit.** `git add crates/cull-proxy && git commit -m "feat(proxy): per-session hit-rate-floor monitor — halt compression after 3 sub-floor turns (R5/R6)"`

---

## After this plan — ledger
- ✅ §8 Rule 5 hit-rate-floor monitor — per-session `HitRateMonitor`, fed by a response-stream tee that reads `cache_read`/`cache_creation`; halts after 3 sub-floor turns.
- ✅ §8 Rule 6 provider-aware costs — provider detected per route (+ 5m/1h from `cache_control.ttl`); the floor and economics come from that provider's `W`/`R` via `CacheModel`.
- ✅ §10 State (per-session) — `ProxyState.monitors` keyed by a stable session hash.
- Note: OpenAI usage shape differs (`prompt_tokens_details.cached_tokens`) and its floor is ~0, so halting effectively never fires for OpenAI — documented, not a silent skip.

## Self-Review
- Floor reuses the existing break-even formula → R5 and R6 share one source of truth. ✓
- Stream tee copies bytes for scanning but yields the original chunk → streaming passthrough stays bit-exact (I3/§10). ✓
- 2 MB scan cap is fail-safe (missing usage → no observation → never falsely halts) and is documented, not silent. ✓
- Monitor mutation happens after the stream drains, lock not held across await → no deadlock, correct happens-before for the next turn. ✓
