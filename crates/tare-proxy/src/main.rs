use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use tare_proxy::{
    server::{app, ProxyState, RuntimeCfg},
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
    let recency_keep: usize = if let Ok(v) = std::env::var("TARE_RECENCY") {
        v.parse().unwrap_or(4)
    } else if let Some(profile) = tare_core::profile::load() {
        let n = profile.recommended_recency_keep;
        eprintln!(
            "[tare-proxy] loaded learned profile: {} (recency={n})",
            profile.summary
        );
        n
    } else {
        4
    };
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
    let holdout_frac = std::env::var("TARE_OUTPUT_HOLDOUT")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let state = Arc::new(ProxyState {
        client,
        upstream,
        opts: CompressOpts {
            enabled,
            recency_keep,
            min_savings: 0,
        },
        runtime_cfg: Mutex::new(RuntimeCfg {
            enabled,
            recency_keep,
        }),
        holdout_frac,
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
