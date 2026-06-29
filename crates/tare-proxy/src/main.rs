use std::process::ExitCode;
use std::sync::Arc;
use tare_proxy::{
    server::{app, ProxyState},
    CompressOpts,
};

#[tokio::main]
async fn main() -> ExitCode {
    let upstream =
        std::env::var("TARE_UPSTREAM").unwrap_or_else(|_| "https://api.anthropic.com".into());
    let port: u16 = std::env::var("TARE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8787);
    let recency_keep: usize = std::env::var("TARE_RECENCY")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4);
    let enabled = std::env::var("TARE_ENABLED")
        .map(|v| v != "0" && v != "false")
        .unwrap_or(true);

    // Timeouts so a slow/hung upstream can't pin a worker forever (overall + connect).
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[tare-proxy] fatal: could not build HTTP client: {e}");
            return ExitCode::FAILURE;
        }
    };
    let state = Arc::new(ProxyState {
        client,
        upstream,
        opts: CompressOpts {
            enabled,
            recency_keep,
            min_savings: 0,
        },
        monitors: Default::default(),
        outputs: Default::default(),
    });

    let listener = match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[tare-proxy] fatal: could not bind 0.0.0.0:{port}: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("[tare-proxy] listening on :{port} -> {}", state.upstream);
    if let Err(e) = axum::serve(listener, app(state)).await {
        eprintln!("[tare-proxy] fatal: server error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
