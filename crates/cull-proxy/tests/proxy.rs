// Spin up a mock upstream that records the body it receives, run the cull proxy pointed at it,
// POST a compressible request to the proxy, and assert the upstream got the COMPRESSED body
// while the client got the upstream's canned response.
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
        opts: CompressOpts { enabled: true, recency_keep: 1, min_savings: 0 },
        monitors: Default::default(),
        outputs: Default::default(),
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

#[tokio::test]
async fn openai_proxy_compresses_then_forwards_and_returns_upstream_response() {
    // 1. mock upstream for /v1/chat/completions
    let rec: Recorder = Arc::new(Mutex::new(None));
    let upstream = Router::new().route("/v1/chat/completions", post(upstream_handler)).with_state(rec.clone());
    let up_port = spawn(upstream).await;

    // 2. cull proxy -> mock upstream
    let state = Arc::new(ProxyState {
        client: reqwest::Client::new(),
        upstream: format!("http://127.0.0.1:{up_port}"),
        opts: CompressOpts { enabled: true, recency_keep: 0, min_savings: 0 },
        monitors: Default::default(),
        outputs: Default::default(),
    });
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
        .send().await.unwrap();
    let body = resp.text().await.unwrap();

    // 4a. client got the upstream's canned response
    assert!(body.contains("\"ok\":true"));
    // 4b. upstream received a COMPRESSED body (irrelevant grep result was stubbed)
    let received = rec.lock().unwrap().clone().expect("upstream received a body");
    assert!(received.contains("[cull"), "upstream body was compressed: {received}");
    assert!(received.contains("jwt authentication middleware"), "relevant content preserved");
    // structure intact: still valid JSON with 5 messages
    let v: serde_json::Value = serde_json::from_str(&received).unwrap();
    assert_eq!(v["messages"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn proxy_response_carries_cull_report_headers() {
    // 1. mock upstream
    let rec: Recorder = Arc::new(Mutex::new(None));
    let upstream = Router::new().route("/v1/messages", post(upstream_handler)).with_state(rec.clone());
    let up_port = spawn(upstream).await;

    // 2. cull proxy -> mock upstream
    let state = Arc::new(ProxyState {
        client: reqwest::Client::new(),
        upstream: format!("http://127.0.0.1:{up_port}"),
        opts: CompressOpts { enabled: true, recency_keep: 1, min_savings: 0 },
        monitors: Default::default(),
        outputs: Default::default(),
    });
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
        .send().await.unwrap();

    // upstream received compressed body AND response has x-cull-net-tokens header
    let received = rec.lock().unwrap().clone().expect("upstream received a body");
    assert!(received.contains("[cull"), "upstream body was compressed: {received}");
    assert!(resp.headers().get("x-cull-net-tokens").is_some(),
        "response carries x-cull-net-tokens header; got headers: {:?}", resp.headers());
}

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
        outputs: Default::default(),
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
