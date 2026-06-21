use std::sync::Arc;
use cull_proxy::{server::{app, ProxyState}, CompressOpts};

#[tokio::main]
async fn main() {
    let upstream = std::env::var("CULL_UPSTREAM").unwrap_or_else(|_| "https://api.anthropic.com".into());
    let port: u16 = std::env::var("CULL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);
    let recency_keep: usize = std::env::var("CULL_RECENCY").ok().and_then(|p| p.parse().ok()).unwrap_or(4);
    let enabled = std::env::var("CULL_ENABLED").map(|v| v != "0" && v != "false").unwrap_or(true);

    // Timeouts so a slow/hung upstream can't pin a worker forever (overall + connect).
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("build reqwest client");
    let state = Arc::new(ProxyState {
        client,
        upstream,
        opts: CompressOpts { enabled, recency_keep, min_savings: 0 },
        monitors: Default::default(),
        outputs: Default::default(),
    });

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await.expect("bind");
    eprintln!("[cull-proxy] listening on :{port} -> {}", state.upstream);
    axum::serve(listener, app(state)).await.expect("serve");
}
