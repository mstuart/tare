// Spin up a mock upstream that records the body it receives, run the tare proxy pointed at it,
// POST a compressible request to the proxy, and assert the upstream got the COMPRESSED body
// while the client got the upstream's canned response.
use axum::{body::Bytes, extract::State, routing::post, Router};
use std::sync::{Arc, Mutex};
use tare_proxy::{
    server::{app, ProxyState, RuntimeCfg},
    CompressOpts,
};

type Recorder = Arc<Mutex<Option<String>>>;

async fn upstream_handler(State(rec): State<Recorder>, body: Bytes) -> &'static str {
    *rec.lock().unwrap() = Some(String::from_utf8_lossy(&body).into_owned());
    "{\"ok\":true}"
}

async fn spawn(router: Router) -> u16 {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    port
}

/// Construct a ProxyState with zero-initialized observability counters and no sessions.
/// `runtime_cfg` mirrors the supplied `opts.enabled` / `opts.recency_keep`.
fn make_state(upstream: String, opts: CompressOpts) -> Arc<ProxyState> {
    let runtime_cfg = RuntimeCfg {
        enabled: opts.enabled,
        recency_keep: opts.recency_keep,
    };
    Arc::new(ProxyState {
        client: reqwest::Client::new(),
        upstream,
        runtime_cfg: Mutex::new(runtime_cfg),
        opts,
        holdout_frac: 0.0,
        start: std::time::Instant::now(),
        monitors: Default::default(),
        outputs: Default::default(),
        seen_sessions: Default::default(),
        cnt_requests: Default::default(),
        cnt_input_tokens: Default::default(),
        cnt_net_tokens: Default::default(),
        cnt_dropped_tokens: Default::default(),
        cnt_halted_sessions: Default::default(),
        cnt_shaped_requests: Default::default(),
        cnt_shaped_output_tokens: Default::default(),
        cnt_holdout_requests: Default::default(),
        cnt_holdout_output_tokens: Default::default(),
    })
}

#[tokio::test]
async fn proxy_compresses_then_forwards_and_returns_upstream_response() {
    // 1. mock upstream
    let rec: Recorder = Arc::new(Mutex::new(None));
    let upstream = Router::new()
        .route("/v1/messages", post(upstream_handler))
        .with_state(rec.clone());
    let up_port = spawn(upstream).await;

    // 2. tare proxy -> mock upstream
    let state = make_state(
        format!("http://127.0.0.1:{up_port}"),
        CompressOpts {
            enabled: true,
            recency_keep: 1,
            min_savings: 0,
        },
    );
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
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();

    // 4a. client got the upstream's canned response
    assert!(body.contains("\"ok\":true"));
    // 4b. upstream received a COMPRESSED body (the irrelevant kafka tool_result was stubbed)
    let received = rec
        .lock()
        .unwrap()
        .clone()
        .expect("upstream received a body");
    assert!(
        received.contains("[tare"),
        "upstream body was compressed: {received}"
    );
    assert!(
        received.contains("jwt authentication middleware"),
        "relevant content preserved"
    );
    // structure intact: still valid JSON with 4 messages
    let v: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(v["messages"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn openai_proxy_compresses_then_forwards_and_returns_upstream_response() {
    // 1. mock upstream for /v1/chat/completions
    let rec: Recorder = Arc::new(Mutex::new(None));
    let upstream = Router::new()
        .route("/v1/chat/completions", post(upstream_handler))
        .with_state(rec.clone());
    let up_port = spawn(upstream).await;

    // 2. tare proxy -> mock upstream
    let state = make_state(
        format!("http://127.0.0.1:{up_port}"),
        CompressOpts {
            enabled: true,
            recency_keep: 0,
            min_savings: 0,
        },
    );
    let proxy_port = spawn(app(state)).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 3. compressible OpenAI request: irrelevant grep result (different class to jwt read, so
    //    supersession doesn't fire; recency_keep:0 lets the relevance pass drop it)
    let req = serde_json::json!({
        "model":"gpt-x",
        "messages":[
            {"role":"assistant","content":null,"tool_calls":[
                {"id":"c1","type":"function","function":{"name":"grep","arguments":"{}"}}]},
            {"role":"tool","tool_call_id":"c1","content":"kubernetes helm registry partitions totally unrelated"},
            {"role":"assistant","content":null,"tool_calls":[
                {"id":"c2","type":"function","function":{"name":"read","arguments":"{\"path\":\"jwt.rs\"}"}}]},
            {"role":"tool","tool_call_id":"c2","content":"jwt authentication middleware verify"},
            {"role":"user","content":"fix the authentication jwt bug"}
        ]
    });
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{proxy_port}/v1/chat/completions"))
        .header("authorization", "Bearer sk-test")
        .json(&req)
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();

    // 4a. client got the upstream's canned response
    assert!(body.contains("\"ok\":true"));
    // 4b. upstream received a COMPRESSED body (irrelevant grep result was stubbed)
    let received = rec
        .lock()
        .unwrap()
        .clone()
        .expect("upstream received a body");
    assert!(
        received.contains("[tare"),
        "upstream body was compressed: {received}"
    );
    assert!(
        received.contains("jwt authentication middleware"),
        "relevant content preserved"
    );
    // structure intact: still valid JSON with 5 messages
    let v: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(v["messages"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn proxy_response_carries_tare_report_headers() {
    // 1. mock upstream
    let rec: Recorder = Arc::new(Mutex::new(None));
    let upstream = Router::new()
        .route("/v1/messages", post(upstream_handler))
        .with_state(rec.clone());
    let up_port = spawn(upstream).await;

    // 2. tare proxy -> mock upstream
    let state = make_state(
        format!("http://127.0.0.1:{up_port}"),
        CompressOpts {
            enabled: true,
            recency_keep: 1,
            min_savings: 0,
        },
    );
    let proxy_port = spawn(app(state)).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 3. compressible request with tool results
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
        .send()
        .await
        .unwrap();

    // upstream received compressed body AND response has x-tare-net-tokens header
    let received = rec
        .lock()
        .unwrap()
        .clone()
        .expect("upstream received a body");
    assert!(
        received.contains("[tare"),
        "upstream body was compressed: {received}"
    );
    assert!(
        resp.headers().get("x-tare-net-tokens").is_some(),
        "response carries x-tare-net-tokens header; got headers: {:?}",
        resp.headers()
    );
}

#[tokio::test]
async fn halts_compression_after_three_low_hit_rate_turns() {
    use axum::{body::Bytes, extract::State, routing::post, Router};
    use std::sync::{Arc, Mutex};
    use tare_proxy::{server::app, CompressOpts};

    // mock upstream: records every received body, returns a LOW cache hit-rate usage (h ~= 0.001)
    type Rec = Arc<Mutex<Vec<String>>>;
    async fn up(State(rec): State<Rec>, body: Bytes) -> &'static str {
        rec.lock()
            .unwrap()
            .push(String::from_utf8_lossy(&body).into_owned());
        "{\"ok\":true,\"usage\":{\"cache_read_input_tokens\":1,\"cache_creation_input_tokens\":1000}}"
    }
    let rec: Rec = Arc::new(Mutex::new(Vec::new()));
    let upstream = Router::new()
        .route("/v1/messages", post(up))
        .with_state(rec.clone());
    let up_port = spawn(upstream).await;

    let state = make_state(
        format!("http://127.0.0.1:{up_port}"),
        CompressOpts {
            enabled: true,
            recency_keep: 1,
            min_savings: 0,
        },
    );
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
        let resp = client
            .post(format!("http://127.0.0.1:{proxy_port}/v1/messages"))
            .header("x-api-key", "sk-test")
            .json(&req)
            .send()
            .await
            .unwrap();
        last_halted = resp.headers().get("x-tare-halted").is_some();
        let _ = resp.text().await.unwrap(); // drain so the tee's monitor update completes
    }
    let bodies = rec.lock().unwrap().clone();
    assert_eq!(bodies.len(), 4);
    // turn 1: compression active -> irrelevant kafka tool_result stubbed away
    assert!(
        !bodies[0].contains("kafka partitions offsets totally unrelated"),
        "turn 1 should be compressed: {}",
        bodies[0]
    );
    // turn 4: monitor halted after 3 sub-floor turns -> byte-exact passthrough (kafka intact)
    assert!(
        bodies[3].contains("kafka partitions offsets totally unrelated"),
        "turn 4 should be uncompressed passthrough: {}",
        bodies[3]
    );
    assert!(last_halted, "turn 4 response carries x-tare-halted");
}

#[tokio::test]
async fn skeletonizes_code_via_controller_under_high_fill() {
    // A large request (fill >= 0.5) drives the controller to level-2 aggression, which skeletonizes
    // kept source-file reads. Proves the fill -> controller -> skeleton path closes end-to-end.
    let rec: Recorder = Arc::new(Mutex::new(None));
    let upstream = Router::new()
        .route("/v1/messages", post(upstream_handler))
        .with_state(rec.clone());
    let up_port = spawn(upstream).await;
    let state = make_state(
        format!("http://127.0.0.1:{up_port}"),
        CompressOpts {
            enabled: true,
            recency_keep: 1,
            min_savings: 0,
        },
    );
    let proxy_port = spawn(app(state)).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // ~110k approx tokens of code (>0.5 of the 200k window) -> controller level 2.
    let code = format!(
        "pub fn big() -> i32 {{\n{}    0\n}}\n",
        "    let v = 1;\n".repeat(32_000)
    );
    let req = serde_json::json!({
        "model":"claude-x","max_tokens":100,
        "messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"r","name":"read","input":{"path":"big.rs"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"r","content": code}]},
            {"role":"user","content":"continue"}
        ]
    });
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{proxy_port}/v1/messages"))
        .header("x-api-key", "sk-test")
        .json(&req)
        .send()
        .await
        .unwrap();
    let aggr = resp
        .headers()
        .get("x-tare-aggression")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let _ = resp.text().await.unwrap();

    let received = rec
        .lock()
        .unwrap()
        .clone()
        .expect("upstream received a body");
    assert_eq!(
        aggr, "tighten",
        "controller reached level-2 'tighten' under high fill"
    );
    assert!(
        received.contains("[tare: code skeleton"),
        "code read was skeletonized"
    );
    assert!(
        received.contains("pub fn big() -> i32"),
        "signature preserved"
    );
    assert!(
        received.len() < code.len(),
        "forwarded body far smaller than the raw code"
    );
}

#[tokio::test]
async fn verbosity_spike_backs_off_compression_end_to_end() {
    // Mock upstream escalates completion_tokens; after the turn-4 spike, turn 5 must BACK OFF (skip
    // relevance) so a normally-pruned irrelevant tool message survives. Proves output -> controller.
    // Uses the OpenAI path (honors recency_keep: 0) so relevance — not recency — gates the grep result.
    type Rec = Arc<Mutex<Vec<String>>>;
    type Cnt = Arc<Mutex<u32>>;
    async fn up(State((rec, cnt)): State<(Rec, Cnt)>, body: Bytes) -> String {
        rec.lock()
            .unwrap()
            .push(String::from_utf8_lossy(&body).into_owned());
        let mut c = cnt.lock().unwrap();
        *c += 1;
        let out = if *c >= 4 { 400 } else { 100 }; // turn 4 onward spikes vs the ~100 baseline
        format!("{{\"ok\":true,\"usage\":{{\"completion_tokens\":{out}}}}}")
    }
    let rec: Rec = Arc::new(Mutex::new(Vec::new()));
    let cnt: Cnt = Arc::new(Mutex::new(0));
    let upstream = Router::new()
        .route("/v1/chat/completions", post(up))
        .with_state((rec.clone(), cnt.clone()));
    let up_port = spawn(upstream).await;
    let state = make_state(
        format!("http://127.0.0.1:{up_port}"),
        CompressOpts {
            enabled: true,
            recency_keep: 0,
            min_savings: 0,
        },
    );
    let proxy_port = spawn(app(state)).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // grep (irrelevant) + a different-tool read (relevant anchor) so supersession doesn't fire and
    // relevance is the only thing pruning the grep result.
    let req = serde_json::json!({
        "model":"gpt-x",
        "messages":[
            {"role":"assistant","content":null,"tool_calls":[
                {"id":"c1","type":"function","function":{"name":"grep","arguments":"{}"}}]},
            {"role":"tool","tool_call_id":"c1","content":"kafka partitions offsets totally unrelated junk"},
            {"role":"assistant","content":null,"tool_calls":[
                {"id":"c2","type":"function","function":{"name":"read","arguments":"{\"path\":\"jwt.rs\"}"}}]},
            {"role":"tool","tool_call_id":"c2","content":"jwt authentication middleware verify token"},
            {"role":"user","content":"fix the authentication jwt bug"}
        ]
    });
    let client = reqwest::Client::new();
    let mut spike_header = false;
    for _ in 0..5 {
        let resp = client
            .post(format!("http://127.0.0.1:{proxy_port}/v1/chat/completions"))
            .header("authorization", "Bearer sk-test")
            .json(&req)
            .send()
            .await
            .unwrap();
        spike_header = resp.headers().get("x-tare-verbosity-spike").is_some();
        let _ = resp.text().await.unwrap(); // drain so the tee's observe() completes
    }
    let bodies = rec.lock().unwrap().clone();
    assert_eq!(bodies.len(), 5);
    // turn 1 (no spike): the irrelevant grep result is pruned by relevance
    assert!(
        !bodies[0].contains("kafka partitions offsets totally unrelated"),
        "turn 1 compressed: {}",
        bodies[0]
    );
    // turn 5 (after the turn-4 verbosity spike): backed off -> grep result kept
    assert!(
        bodies[4].contains("kafka partitions offsets totally unrelated"),
        "turn 5 backed off (relevance skipped): {}",
        bodies[4]
    );
    assert!(
        spike_header,
        "turn 5 response carries x-tare-verbosity-spike"
    );
}

#[tokio::test]
async fn count_tokens_exact_parses_input_tokens_and_falls_back() {
    use axum::{body::Bytes, routing::post, Router};
    use tare_proxy::count::{count_tokens_exact, count_tokens_or_approx};

    // mock Anthropic count_tokens endpoint
    async fn counter(_b: Bytes) -> &'static str {
        "{\"input_tokens\":1234}"
    }
    let upstream = Router::new().route("/v1/messages/count_tokens", post(counter));
    let port = spawn(upstream).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let body = serde_json::json!({"model":"claude-x","messages":[{"role":"user","content":"hi"}]});

    // exact path: mock returns 1234
    let n = count_tokens_exact(&client, &base, "sk-test", "2023-06-01", &body)
        .await
        .unwrap();
    assert_eq!(n, 1234);
    let (c, exact) =
        count_tokens_or_approx(&client, &base, Some("sk-test"), "2023-06-01", &body, 999).await;
    assert_eq!((c, exact), (1234, true));

    // fallback path: no key -> approximate, exact=false (no network call made)
    let (c2, exact2) = count_tokens_or_approx(&client, &base, None, "2023-06-01", &body, 999).await;
    assert_eq!((c2, exact2), (999, false));

    // fallback path: bad base (connection refused) -> approximate, exact=false
    let (c3, exact3) = count_tokens_or_approx(
        &client,
        "http://127.0.0.1:1",
        Some("sk-test"),
        "2023-06-01",
        &body,
        777,
    )
    .await;
    assert_eq!((c3, exact3), (777, false));
}

// ── Admin surface tests ────────────────────────────────────────────────────────

/// GET /admin/stats returns a JSON object with the exact fields specified in the contract,
/// and the values are sensible after a couple of proxy requests.
#[tokio::test]
async fn admin_stats_returns_correct_shape_and_counts_requests() {
    let rec: Recorder = Arc::new(Mutex::new(None));
    let upstream = Router::new()
        .route("/v1/messages", post(upstream_handler))
        .with_state(rec.clone());
    let up_port = spawn(upstream).await;
    let state = make_state(
        format!("http://127.0.0.1:{up_port}"),
        CompressOpts {
            enabled: true,
            recency_keep: 4,
            min_savings: 0,
        },
    );
    let proxy_port = spawn(app(state)).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let req = serde_json::json!({
        "model":"claude-x","max_tokens":100,
        "messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"run","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"kafka unrelated"}]},
            {"role":"user","content":"fix jwt"}
        ]
    });
    let client = reqwest::Client::new();
    // send two requests
    for _ in 0..2 {
        client
            .post(format!("http://127.0.0.1:{proxy_port}/v1/messages"))
            .header("x-api-key", "sk-test")
            .json(&req)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
    }

    let stats: serde_json::Value = client
        .get(format!("http://127.0.0.1:{proxy_port}/admin/stats"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // Required top-level fields are present with correct types
    assert!(stats["requests"].is_u64(), "requests must be u64: {stats}");
    assert!(
        stats["input_tokens"].is_u64(),
        "input_tokens must be u64: {stats}"
    );
    assert!(
        stats["net_tokens"].is_u64(),
        "net_tokens must be u64: {stats}"
    );
    assert!(
        stats["dropped_tokens"].is_u64(),
        "dropped_tokens must be u64: {stats}"
    );
    assert!(
        stats["savings_ratio"].is_f64(),
        "savings_ratio must be f64: {stats}"
    );
    assert!(stats["sessions"].is_u64(), "sessions must be u64: {stats}");
    assert!(
        stats["halted_sessions"].is_u64(),
        "halted_sessions must be u64: {stats}"
    );
    assert!(
        stats["enabled"].is_boolean(),
        "enabled must be bool: {stats}"
    );
    assert!(
        stats["recency_keep"].is_u64(),
        "recency_keep must be u64: {stats}"
    );
    assert!(
        stats["uptime_secs"].is_u64(),
        "uptime_secs must be u64: {stats}"
    );
    // output sub-object
    let out = &stats["output"];
    assert!(
        out["shaped_requests"].is_u64(),
        "output.shaped_requests: {out}"
    );
    assert!(
        out["shaped_output_tokens"].is_u64(),
        "output.shaped_output_tokens: {out}"
    );
    assert!(
        out["holdout_requests"].is_u64(),
        "output.holdout_requests: {out}"
    );
    assert!(
        out["holdout_output_tokens"].is_u64(),
        "output.holdout_output_tokens: {out}"
    );

    // request counter reflects the two requests we sent
    assert_eq!(
        stats["requests"].as_u64().unwrap(),
        2,
        "two requests → requests counter == 2"
    );
    // savings_ratio in [0, 1]
    let sr = stats["savings_ratio"].as_f64().unwrap();
    assert!(
        (0.0..=1.0).contains(&sr),
        "savings_ratio out of range: {sr}"
    );
    // enabled mirrors what we set
    assert!(
        stats["enabled"].as_bool().unwrap(),
        "enabled should be true"
    );
    assert_eq!(stats["recency_keep"].as_u64().unwrap(), 4);
    // with no holdout, holdout_requests == 0 and shaped_requests == 2
    assert_eq!(out["holdout_requests"].as_u64().unwrap(), 0);
    assert_eq!(out["shaped_requests"].as_u64().unwrap(), 2);
}

/// POST /admin/runtime-env with TARE_ENABLED=false → response reports enabled:false, and
/// subsequent proxy requests bypass compression (passthrough).
#[tokio::test]
async fn admin_runtime_env_disables_and_reenables_compression() {
    let rec: Recorder = Arc::new(Mutex::new(None));
    let upstream = Router::new()
        .route("/v1/messages", post(upstream_handler))
        .with_state(rec.clone());
    let up_port = spawn(upstream).await;
    let state = make_state(
        format!("http://127.0.0.1:{up_port}"),
        CompressOpts {
            enabled: true,
            recency_keep: 0,
            min_savings: 0,
        },
    );
    let proxy_port = spawn(app(state)).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    // Two results from the same tool ("run") → SupersessionPass drops the first (kafka).
    // This is the same setup used in the basic compression test, so we know it compresses.
    let compressible_req = serde_json::json!({
        "model":"claude-x","max_tokens":100,
        "messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"run","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"kafka partitions offsets totally unrelated"}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"jwt authentication middleware"}]},
            {"role":"user","content":"fix the authentication jwt bug"}
        ]
    });

    // Before disable: compression is active
    client
        .post(format!("http://127.0.0.1:{proxy_port}/v1/messages"))
        .header("x-api-key", "sk-test")
        .json(&compressible_req)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let before = rec.lock().unwrap().clone().unwrap();
    assert!(
        before.contains("[tare"),
        "before disable: compression active: {before}"
    );

    // Disable via runtime-env
    let disable_resp: serde_json::Value = client
        .post(format!("http://127.0.0.1:{proxy_port}/admin/runtime-env"))
        .json(&serde_json::json!({"TARE_ENABLED": "false"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !disable_resp["enabled"].as_bool().unwrap(),
        "runtime-env response must reflect disabled: {disable_resp}"
    );

    // After disable: passthrough (no [tare stub)
    client
        .post(format!("http://127.0.0.1:{proxy_port}/v1/messages"))
        .header("x-api-key", "sk-test")
        .json(&compressible_req)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let after_disable = rec.lock().unwrap().clone().unwrap();
    assert!(
        !after_disable.contains("[tare"),
        "after disable: passthrough expected: {after_disable}"
    );

    // Re-enable and verify compression returns
    let enable_resp: serde_json::Value = client
        .post(format!("http://127.0.0.1:{proxy_port}/admin/runtime-env"))
        .json(&serde_json::json!({"TARE_ENABLED": "true"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        enable_resp["enabled"].as_bool().unwrap(),
        "runtime-env response must reflect re-enabled: {enable_resp}"
    );
    client
        .post(format!("http://127.0.0.1:{proxy_port}/v1/messages"))
        .header("x-api-key", "sk-test")
        .json(&compressible_req)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let after_enable = rec.lock().unwrap().clone().unwrap();
    assert!(
        after_enable.contains("[tare"),
        "after re-enable: compression active again: {after_enable}"
    );
}

/// POST /admin/runtime-env with TARE_RECENCY updates recency_keep and the response confirms it.
#[tokio::test]
async fn admin_runtime_env_updates_recency_keep() {
    let rec: Recorder = Arc::new(Mutex::new(None));
    let upstream = Router::new()
        .route("/v1/messages", post(upstream_handler))
        .with_state(rec.clone());
    let up_port = spawn(upstream).await;
    let state = make_state(
        format!("http://127.0.0.1:{up_port}"),
        CompressOpts {
            enabled: true,
            recency_keep: 4,
            min_savings: 0,
        },
    );
    let proxy_port = spawn(app(state)).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = reqwest::Client::new();

    // Update recency_keep to 8
    let resp: serde_json::Value = client
        .post(format!("http://127.0.0.1:{proxy_port}/admin/runtime-env"))
        .json(&serde_json::json!({"TARE_RECENCY": "8"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        resp["recency_keep"].as_u64().unwrap(),
        8,
        "recency_keep should update to 8: {resp}"
    );
    assert!(
        resp["enabled"].as_bool().unwrap(),
        "enabled unchanged: {resp}"
    );

    // Verify stats also reflects the new recency_keep
    let stats: serde_json::Value = client
        .get(format!("http://127.0.0.1:{proxy_port}/admin/stats"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        stats["recency_keep"].as_u64().unwrap(),
        8,
        "stats recency_keep should reflect update: {stats}"
    );
}
