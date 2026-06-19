# Cull — Plan 11: HTTP Proxy Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A runnable `cull-proxy` HTTP server. It accepts `POST /v1/messages`, compresses the request body with `compress_anthropic_request` (Plan 10), forwards to the real Anthropic API with auth headers preserved, and streams the response back verbatim. Pointing a coding agent at `ANTHROPIC_BASE_URL=http://localhost:<port>` makes Cull compress its traffic live.

**Architecture:** `axum` server with one route. The handler reads the raw body; if it parses as JSON it is compressed (else forwarded unchanged — never reject), then forwarded via a `reqwest` client to `{upstream}/v1/messages`, copying through the auth/version headers. The upstream response is streamed back unchanged (status + headers + body stream) — responses are never inspected, so streaming SSE and tool-calls pass through untouched. Config comes from env (`CULL_UPSTREAM`, `CULL_PORT`, `CULL_RECENCY`, `CULL_ENABLED`). A two-server integration test proves the upstream receives a *compressed* body and the client gets the upstream's response.

**Tech Stack:** Rust, `axum` 0.7, `tokio`, `reqwest` 0.12 (stream). Builds on Plan 10's `compress_anthropic_request`/`CompressOpts`. Reference: spec §10 (proxy — only a proxy can actually compress a prompt; responses pass through).

> **Version note:** axum/reqwest APIs drift between minor versions. The code below targets axum 0.7 + reqwest 0.12. If a symbol differs, ADAPT minimally to whatever compiles, keeping the behavior identical (compress request → forward with auth headers → stream response). The integration test (Task 2) is the real acceptance gate.

---

## File Structure

```
crates/cull-proxy/Cargo.toml             # add axum, tokio, reqwest, bytes; [[bin]] cull-proxy
crates/cull-proxy/src/server.rs          # ProxyState, app(), handle_messages
crates/cull-proxy/src/lib.rs             # `pub mod server;`
crates/cull-proxy/src/main.rs            # env config + serve
crates/cull-proxy/tests/proxy.rs         # two-server integration test
```

---

### Task 1: The server (handler + app + main)

**Files:** Modify `crates/cull-proxy/Cargo.toml`; create `crates/cull-proxy/src/server.rs` and `crates/cull-proxy/src/main.rs`; modify `crates/cull-proxy/src/lib.rs`.

- [ ] **Step 1: Dependencies + binary**

In `crates/cull-proxy/Cargo.toml`, add to `[dependencies]`:
```toml
axum = "0.7"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net"] }
reqwest = { version = "0.12", default-features = false, features = ["stream", "rustls-tls"] }
bytes = "1"
```
and add the binary + a dev-dep for the test client:
```toml
[[bin]]
name = "cull-proxy"
path = "src/main.rs"

[dev-dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "time"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

- [ ] **Step 2: Implement the server module**

Create `crates/cull-proxy/src/server.rs`:
```rust
use std::sync::Arc;
use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use crate::{compress_anthropic_request, CompressOpts};

pub struct ProxyState {
    pub client: reqwest::Client,
    pub upstream: String, // e.g. "https://api.anthropic.com"
    pub opts: CompressOpts,
}

pub fn app(state: Arc<ProxyState>) -> Router {
    Router::new()
        .route("/v1/messages", post(handle_messages))
        .with_state(state)
}

// headers we forward upstream (auth + protocol); others (host, content-length) are reset by reqwest.
const FORWARD_HEADERS: &[&str] = &[
    "x-api-key", "authorization", "anthropic-version", "anthropic-beta", "content-type",
];

async fn handle_messages(
    State(state): State<Arc<ProxyState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // compress if the body is JSON; otherwise forward unchanged (never reject)
    let forward_body: Vec<u8> = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(req) => {
            let compressed = compress_anthropic_request(&req, &state.opts);
            serde_json::to_vec(&compressed).unwrap_or_else(|_| body.to_vec())
        }
        Err(_) => body.to_vec(),
    };

    let url = format!("{}/v1/messages", state.upstream.trim_end_matches('/'));
    let mut fwd = state.client.post(&url).body(forward_body);
    for name in FORWARD_HEADERS {
        if let Some(v) = headers.get(*name) {
            fwd = fwd.header(*name, v);
        }
    }

    match fwd.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut builder = Response::builder().status(status);
            for (k, v) in resp.headers().iter() {
                // skip hop-by-hop / length headers; let axum recompute
                let kn = k.as_str();
                if kn == "content-length" || kn == "transfer-encoding" || kn == "connection" { continue; }
                builder = builder.header(k, v);
            }
            builder
                .body(Body::from_stream(resp.bytes_stream()))
                .unwrap_or_else(|_| (StatusCode::BAD_GATEWAY, "bad upstream response").into_response())
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("cull-proxy upstream error: {e}")).into_response(),
    }
}
```

Add `pub mod server;` to `crates/cull-proxy/src/lib.rs`.

- [ ] **Step 3: Implement main.rs**

Create `crates/cull-proxy/src/main.rs`:
```rust
use std::sync::Arc;
use cull_proxy::{server::{app, ProxyState}, CompressOpts};

#[tokio::main]
async fn main() {
    let upstream = std::env::var("CULL_UPSTREAM").unwrap_or_else(|_| "https://api.anthropic.com".into());
    let port: u16 = std::env::var("CULL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);
    let recency_keep: usize = std::env::var("CULL_RECENCY").ok().and_then(|p| p.parse().ok()).unwrap_or(4);
    let enabled = std::env::var("CULL_ENABLED").map(|v| v != "0" && v != "false").unwrap_or(true);

    let state = Arc::new(ProxyState {
        client: reqwest::Client::new(),
        upstream,
        opts: CompressOpts { enabled, recency_keep },
    });

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await.expect("bind");
    eprintln!("[cull-proxy] listening on :{port} -> {}", state.upstream);
    axum::serve(listener, app(state)).await.expect("serve");
}
```

- [ ] **Step 4: Build**

Run: `cargo build -p cull-proxy`
Expected: PASS (the `cull-proxy` binary builds). Resolve any axum/reqwest API drift here.

- [ ] **Step 5: Commit**

```bash
git add crates/cull-proxy
git commit -m "feat(proxy): axum server — compress /v1/messages, forward with auth, stream response"
```

---

### Task 2: Two-server integration test

**Files:** Create `crates/cull-proxy/tests/proxy.rs`.

- [ ] **Step 1: Write the failing test**

Create `crates/cull-proxy/tests/proxy.rs`:
```rust
// Spin up a mock "Anthropic" upstream that records the body it receives, run the cull proxy
// pointed at it, POST a compressible request to the proxy, and assert the upstream got the
// COMPRESSED body while the client got the upstream's canned response.
use std::sync::{Arc, Mutex};
use axum::{routing::post, Router, extract::State, body::Bytes};
use cull_proxy::{server::{app, ProxyState}, CompressOpts};

type Recorder = Arc<Mutex<Option<String>>>;

async fn upstream_handler(State(rec): State<Recorder>, body: Bytes) -> &'static str {
    *rec.lock().unwrap() = Some(String::from_utf8_lossy(&body).into_owned());
    "{\"ok\":true}"
}

async fn spawn(router: Router) -> u16 {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
    port
}

#[tokio::test]
async fn proxy_compresses_then_forwards_and_returns_upstream_response() {
    // 1. mock upstream
    let rec: Recorder = Arc::new(Mutex::new(None));
    let upstream = Router::new().route("/v1/messages", post(upstream_handler)).with_state(rec.clone());
    let up_port = spawn(upstream).await;

    // 2. cull proxy -> mock upstream
    let state = Arc::new(ProxyState {
        client: reqwest::Client::new(),
        upstream: format!("http://127.0.0.1:{up_port}"),
        opts: CompressOpts { enabled: true, recency_keep: 1 },
    });
    let proxy_port = spawn(app(state)).await;

    // give the servers a tick to be ready
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 3. compressible request: an irrelevant tool_result that should be stubbed
    let req = serde_json::json!({
        "model":"claude-x","max_tokens":100,
        "messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"run","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"kafka partitions offsets totally unrelated"}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"jwt authentication middleware"}]},
            {"role":"user","content":"fix the authentication jwt bug"}
        ]
    });
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{proxy_port}/v1/messages"))
        .header("x-api-key", "sk-test")
        .json(&req)
        .send().await.unwrap();
    let body = resp.text().await.unwrap();

    // 4a. client got the upstream's canned response
    assert!(body.contains("\"ok\":true"));
    // 4b. upstream received a COMPRESSED body (the irrelevant kafka tool_result was stubbed)
    let received = rec.lock().unwrap().clone().expect("upstream received a body");
    assert!(received.contains("[cull"), "upstream body was compressed: {received}");
    assert!(received.contains("jwt authentication middleware"), "relevant content preserved");
    // structure intact: still valid JSON with 4 messages
    let v: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(v["messages"].as_array().unwrap().len(), 4);
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p cull-proxy --test proxy`
Expected: PASS. If it fails on a timing flake, increase the readiness sleep; if it fails on an axum/reqwest API mismatch, adapt the server code (Task 1) until both the build and this test pass. Do NOT weaken the assertions about compression/structure.

- [ ] **Step 3: Full workspace test + commit**

Run: `cargo test --workspace`
Expected: PASS (all crates + the new proxy integration test).

```bash
git add crates/cull-proxy
git commit -m "test(proxy): two-server integration — compresses request, forwards, returns upstream response"
```

---

## Self-Review

**1. Spec coverage:**
- §10 proxy: intercept `/v1/messages`, compress request, forward with auth, stream response → Tasks 1–2. ✓
- Responses never inspected (streamed verbatim) → handler. ✓
- Transparency via `CULL_ENABLED=0` (passes `enabled:false` → `compress_anthropic_request` is a no-op) → main.rs. ✓
- OpenAI support, request-shape edge cases beyond tool_result strings → future (the transform is the extensible point). Not in scope.

**2. Placeholder scan:** No vague steps. The version-drift note is an explicit instruction to adapt-to-compile, not a placeholder. ✓

**3. Type consistency:** `compress_anthropic_request`/`CompressOpts` (Plan 10) used by the handler; `ProxyState`/`app` (Task 1) used by main.rs and the integration test (Task 2). axum `State`/`Bytes`/`Body::from_stream`, reqwest `Client`/`bytes_stream` per the 0.7/0.12 APIs. ✓

**4. Ambiguity check:** Non-JSON body → forwarded unchanged (never reject). Upstream error → 502 with a message (agent sees a clean error, not a hang). Hop-by-hop/length headers are dropped so axum recomputes them. Only the listed auth/protocol headers are forwarded. The integration test asserts the real contract (compressed upstream body + passthrough response + intact structure). ✓

**Outcome:** Cull is now a real, usable product: point any Anthropic-API client at the proxy and its tool-output traffic is compressed live, structure-preserving, with responses untouched. Final piece: the honest benchmark proving the savings vs. incumbents.
