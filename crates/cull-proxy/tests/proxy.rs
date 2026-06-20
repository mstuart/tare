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
        opts: CompressOpts { enabled: true, recency_keep: 1 },
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
