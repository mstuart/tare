use std::sync::Arc;
use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use bytes::Bytes;
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
    req: Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let headers = parts.headers;
    let body_bytes: Bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("failed to read body: {e}")).into_response(),
    };

    // compress if the body is JSON; otherwise forward unchanged (never reject)
    let forward_body: Vec<u8> = match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
        Ok(req_json) => {
            let compressed = compress_anthropic_request(&req_json, &state.opts);
            serde_json::to_vec(&compressed).unwrap_or_else(|_| body_bytes.to_vec())
        }
        Err(_) => body_bytes.to_vec(),
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
