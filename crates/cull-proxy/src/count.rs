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
    let resp = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", anthropic_version)
        .header("content-type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| format!("count_tokens request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("count_tokens HTTP {}", resp.status().as_u16()));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("count_tokens decode failed: {e}"))?;
    v.get("input_tokens")
        .and_then(Value::as_u64)
        .map(|n| n as u32)
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
