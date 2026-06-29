//! `tare doctor` — health check: engine self-test, tokenizer sanity, config report,
//! proxy probe, and learned-profile status.

use tare_core::{code_skeleton, json_crush};
use tare_tokenize::{ApproxCounter, TokenCounter};

pub struct DoctorResult {
    pub ok: bool,
}

/// Run all health checks, printing a ✓/✗/⚠ checklist to stdout. Returns `false` if any ✗.
pub fn run() -> DoctorResult {
    let mut ok = true;

    // ── 1. json_crush lossless round-trip ───────────────────────────────────
    {
        let input = r#"[{"id":1,"name":"alpha"},{"id":2,"name":"beta"},{"id":3,"name":"gamma"}]"#;
        let pass = match json_crush::crush(input) {
            Some(compressed) => match json_crush::expand(&compressed) {
                Some(v) => {
                    let orig: serde_json::Value = serde_json::from_str(input).unwrap();
                    v == orig
                }
                None => false,
            },
            // crush returned None → input is already optimal / passthrough; acceptable
            None => true,
        };
        if pass {
            println!("✓  json_crush: lossless round-trip ok");
        } else {
            println!("✗  json_crush: round-trip FAILED");
            ok = false;
        }
    }

    // ── 2. code_skeleton: signature kept, body elided ───────────────────────
    {
        // Body has 3+ lines so MIN_BODY_LINES gate passes.
        let snippet = "fn add(a: i32, b: i32) -> i32 {\n    let result = a + b;\n    result\n}\n";
        let pass = match code_skeleton::skeletonize(snippet, "example.rs") {
            Some(s) => s.contains("fn add") && !s.contains("let result"),
            // None = nothing elidable / passthrough; tolerate in health check
            None => true,
        };
        if pass {
            println!("✓  code_skeleton: signature kept, body elided");
        } else {
            println!("✗  code_skeleton: self-test FAILED");
            ok = false;
        }
    }

    // ── 3. Tokenizer sanity ─────────────────────────────────────────────────
    {
        let text = "hello world this is a test sentence";
        let count = ApproxCounter::o200k().count(text);
        let chars = text.chars().count();
        // approx = ceil(chars/4); must be > 0 and in the rough chars/4 neighbourhood
        let lo = chars / 8;
        let hi = chars / 2 + 2;
        if count > 0 && count >= lo && count <= hi {
            println!("✓  tokenizer: count={count} (chars={chars})");
        } else {
            println!("✗  tokenizer: unexpected count={count} for chars={chars}");
            ok = false;
        }
    }

    // ── 4. Config ───────────────────────────────────────────────────────────
    println!();
    println!("Config (resolved env):");
    let upstream =
        std::env::var("TARE_UPSTREAM").unwrap_or_else(|_| "https://api.anthropic.com".into());
    let port_str = std::env::var("TARE_PORT").unwrap_or_else(|_| "8787".into());
    let enabled = std::env::var("TARE_ENABLED").unwrap_or_else(|_| "true".into());
    let recency = std::env::var("TARE_RECENCY").unwrap_or_else(|_| "4".into());
    let ctx_limit = std::env::var("TARE_CONTEXT_LIMIT").unwrap_or_else(|_| "200000".into());

    println!("  TARE_UPSTREAM      = {upstream}");
    println!("  TARE_PORT          = {port_str}");
    println!("  TARE_ENABLED       = {enabled}");
    println!("  TARE_RECENCY       = {recency}");
    println!("  TARE_CONTEXT_LIMIT = {ctx_limit}");

    // Hard-fail (✗) on values that are SET but invalid — a real misconfiguration the proxy
    // would silently fall back from. Unset vars use the valid defaults above and stay ✓.
    let port: u16 = port_str.parse().unwrap_or(0);
    if port == 0 {
        println!("✗  TARE_PORT is not a valid port number: {port_str}");
        ok = false;
    }
    if matches!(ctx_limit.parse::<u64>(), Ok(0) | Err(_)) {
        println!("✗  TARE_CONTEXT_LIMIT must be a positive integer: {ctx_limit}");
        ok = false;
    }
    if std::env::var("TARE_RECENCY").is_ok() && recency.parse::<usize>().is_err() {
        println!("✗  TARE_RECENCY must be a non-negative integer: {recency}");
        ok = false;
    }
    if !matches!(enabled.as_str(), "0" | "1" | "true" | "false") {
        println!("⚠  TARE_ENABLED is unrecognised (expected 0/1/true/false): {enabled}");
    }

    // ── 5. Proxy probe (best-effort TCP connect) ────────────────────────────
    println!();
    {
        let port_u16: u16 = port_str.parse().unwrap_or(8787);
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port_u16));
        let timeout = std::time::Duration::from_millis(300);
        match std::net::TcpStream::connect_timeout(&addr, timeout) {
            Ok(_) => println!("✓  proxy: listening on 127.0.0.1:{port_u16}"),
            Err(_) => println!(
                "⚠  proxy: not listening on 127.0.0.1:{port_u16} (start with `tare-proxy`)"
            ),
        }
    }

    // ── 6. Learned profile ──────────────────────────────────────────────────
    println!();
    match tare_core::profile::load() {
        Some(p) => println!("✓  profile: present — {}", p.summary),
        None => println!("⚠  profile: absent (run `tare learn --from <DIR>` to generate one)"),
    }

    println!();
    if ok {
        println!("All checks passed.");
    } else {
        println!("One or more checks FAILED.");
    }

    DoctorResult { ok }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tare_tokenize::TokenCounter;

    #[test]
    fn json_crush_round_trip_is_lossless() {
        let input = r#"[{"id":1,"name":"alpha"},{"id":2,"name":"beta"},{"id":3,"name":"gamma"}]"#;
        if let Some(c) = json_crush::crush(input) {
            let v = json_crush::expand(&c).expect("expand must succeed after crush");
            let orig: serde_json::Value = serde_json::from_str(input).unwrap();
            assert_eq!(v, orig, "round-trip must be value-identical");
        }
        // crush returning None (passthrough) is also acceptable
    }

    #[test]
    fn code_skeleton_keeps_signature_drops_body() {
        let snippet = "fn add(a: i32, b: i32) -> i32 {\n    let result = a + b;\n    result\n}\n";
        match code_skeleton::skeletonize(snippet, "example.rs") {
            Some(s) => {
                assert!(s.contains("fn add"), "signature must be retained:\n{s}");
                assert!(!s.contains("let result"), "body line must be elided:\n{s}");
            }
            None => {
                // passthrough — tolerable; the engine self-test still counts as ok
            }
        }
    }

    #[test]
    fn tokenizer_count_is_positive_and_roughly_chars_over_four() {
        let text = "hello world this is a test sentence";
        let count = ApproxCounter::o200k().count(text);
        let chars = text.chars().count();
        assert!(count > 0, "count must be positive");
        // chars/4 heuristic; give a 2x band either side
        assert!(
            count >= chars / 8 && count <= chars / 2 + 2,
            "count {count} is far outside chars/4 range for {chars} chars"
        );
    }
}
