# Cull — Plan 28: Anthropic `count_tokens` Exact Counting (§6)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Close §6 "Anthropic `count_tokens` API (exact counts) — approximation only." Build a client that calls Anthropic's `POST /v1/messages/count_tokens` for exact `input_tokens`, with graceful fallback to the approximate counter when no key / offline.

**Architecture:** Two async fns in a new `cull-proxy::count` module (cull-proxy already has `reqwest` + `tokio`): `count_tokens_exact(...) -> Result<u32, String>` (calls the endpoint, parses `input_tokens`) and `count_tokens_or_approx(...) -> (u32, bool)` (exact when a key is present and the call succeeds, else the supplied approximate count; the bool says which). Tested against a mock upstream (correctness without a real key). The live call needs `ANTHROPIC_API_KEY` (absent in this env — a reported runtime blocker, not a code gap).

**Tech:** Rust, reqwest, serde_json, axum (test mock). Reference: spec §6; Anthropic Messages `count_tokens` endpoint (returns `{"input_tokens": N}`).

---

### Task 1: count_tokens client + fallback

**Files:** create `crates/cull-proxy/src/count.rs`; modify `crates/cull-proxy/src/lib.rs` (`pub mod count;`); add an integration test to `crates/cull-proxy/tests/proxy.rs`.

- [ ] **Step 1 — implement** `crates/cull-proxy/src/count.rs`:
```rust
use serde_json::Value;

/// Anthropic exact token counting (spec §6). POSTs the request body (model + messages, plus any
/// `system`/`tools`) to `{base}/v1/messages/count_tokens` and returns `input_tokens`. Network/auth/
/// shape errors surface as `Err` so the caller can fall back to the approximate counter.
pub async fn count_tokens_exact(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    anthropic_version: &str,
    body: &Value,
) -> Result<u32, String> {
    let url = format!("{}/v1/messages/count_tokens", base.trim_end_matches('/'));
    let resp = client.post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", anthropic_version)
        .header("content-type", "application/json")
        .json(body)
        .send().await.map_err(|e| format!("count_tokens request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("count_tokens HTTP {}", resp.status().as_u16()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("count_tokens decode failed: {e}"))?;
    v.get("input_tokens").and_then(Value::as_u64).map(|n| n as u32)
        .ok_or_else(|| "count_tokens: missing input_tokens".to_string())
}

/// Exact count when a key is present and the call succeeds; otherwise the supplied approximate
/// count (spec §6: exact when available, approximate otherwise). The bool is `true` iff exact.
pub async fn count_tokens_or_approx(
    client: &reqwest::Client,
    base: &str,
    api_key: Option<&str>,
    anthropic_version: &str,
    body: &Value,
    approx: u32,
) -> (u32, bool) {
    if let Some(key) = api_key {
        if let Ok(n) = count_tokens_exact(client, base, key, anthropic_version, body).await {
            return (n, true);
        }
    }
    (approx, false)
}
```

- [ ] **Step 2 — wire** `pub mod count;` into `crates/cull-proxy/src/lib.rs` (near `pub mod server;`).

- [ ] **Step 3 — failing integration test.** Append to `crates/cull-proxy/tests/proxy.rs` (reuses the existing `spawn` helper):
```rust
#[tokio::test]
async fn count_tokens_exact_parses_input_tokens_and_falls_back() {
    use axum::{routing::post, Router, body::Bytes};
    use cull_proxy::count::{count_tokens_exact, count_tokens_or_approx};

    // mock Anthropic count_tokens endpoint
    async fn counter(_b: Bytes) -> &'static str { "{\"input_tokens\":1234}" }
    let upstream = Router::new().route("/v1/messages/count_tokens", post(counter));
    let port = spawn(upstream).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let body = serde_json::json!({"model":"claude-x","messages":[{"role":"user","content":"hi"}]});

    // exact path: mock returns 1234
    let n = count_tokens_exact(&client, &base, "sk-test", "2023-06-01", &body).await.unwrap();
    assert_eq!(n, 1234);
    let (c, exact) = count_tokens_or_approx(&client, &base, Some("sk-test"), "2023-06-01", &body, 999).await;
    assert_eq!((c, exact), (1234, true));

    // fallback path: no key -> approximate, exact=false (no network call made)
    let (c2, exact2) = count_tokens_or_approx(&client, &base, None, "2023-06-01", &body, 999).await;
    assert_eq!((c2, exact2), (999, false));

    // fallback path: bad base (connection refused) -> approximate, exact=false
    let (c3, exact3) = count_tokens_or_approx(&client, "http://127.0.0.1:1", Some("sk-test"), "2023-06-01", &body, 777).await;
    assert_eq!((c3, exact3), (777, false));
}
```

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-proxy count_tokens`), then `cargo test --workspace`.

- [ ] **Step 5 — commit.** `git add crates/cull-proxy && git commit -m "feat(proxy): Anthropic count_tokens exact client + approximate fallback (§6)"`

---

## After this plan — ledger
- ✅ §6 `count_tokens` exact API — `count::count_tokens_exact` + `count_tokens_or_approx` (exact when keyed, approximate fallback otherwise), verified against a mock upstream. **Runtime blocker:** no `ANTHROPIC_API_KEY` in this env, so the *live* call can't be exercised here — the code is complete and the approximate counter is the fallback by design.

## Self-Review
- Fallback is total: no key OR any request/HTTP/decode error ⇒ approximate, never panics. ✓
- Verified against a mock returning the real `{"input_tokens": N}` shape → correctness proven without a key. ✓
- Lives in cull-proxy (async + reqwest already present); the sync `ApproxCounter` stays the default counter elsewhere. ✓
