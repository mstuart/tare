//! `tare dashboard` — live savings panel, polling GET /admin/stats every --interval-ms.

use crate::admin::admin_get;

pub struct DashboardOpts {
    pub port: u16,
    pub once: bool,
    pub interval_ms: u64,
}

pub fn run(opts: DashboardOpts) {
    loop {
        match admin_get(opts.port, "/admin/stats") {
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
            Ok(v) => render(&v),
        }

        if opts.once {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(opts.interval_ms));
        // ANSI: clear screen, move cursor to top-left before next frame.
        print!("\x1b[2J\x1b[H");
    }
}

pub fn render(v: &serde_json::Value) {
    let requests = v["requests"].as_u64().unwrap_or(0);
    let input_tokens = v["input_tokens"].as_u64().unwrap_or(0);
    let net_tokens = v["net_tokens"].as_u64().unwrap_or(0);
    let dropped_tokens = v["dropped_tokens"].as_u64().unwrap_or(0);
    let savings_ratio = v["savings_ratio"].as_f64().unwrap_or(0.0);
    let sessions = v["sessions"].as_u64().unwrap_or(0);
    let halted_sessions = v["halted_sessions"].as_u64().unwrap_or(0);
    let enabled = v["enabled"].as_bool().unwrap_or(false);
    let recency_keep = v["recency_keep"].as_u64().unwrap_or(0);
    let uptime_secs = v["uptime_secs"].as_u64().unwrap_or(0);

    let shaped_requests = v["output"]["shaped_requests"].as_u64().unwrap_or(0);
    let shaped_output_tokens = v["output"]["shaped_output_tokens"].as_u64().unwrap_or(0);
    let holdout_requests = v["output"]["holdout_requests"].as_u64().unwrap_or(0);
    let holdout_output_tokens = v["output"]["holdout_output_tokens"].as_u64().unwrap_or(0);

    let uptime_str = format_uptime(uptime_secs);

    println!("── tare proxy ──────────────────────────────────────");
    println!("  uptime        : {:<12}  enabled: {}", uptime_str, enabled);
    println!("  recency_keep  : {}", recency_keep);
    println!("── tokens ──────────────────────────────────────────");
    println!("  requests      : {}", requests);
    println!("  input tokens  : {}", input_tokens);
    println!("  net tokens    : {}", net_tokens);
    println!("  dropped tokens: {}", dropped_tokens);
    println!("  savings       : {:.1}%", savings_ratio * 100.0);
    println!("── sessions ────────────────────────────────────────");
    println!("  sessions      : {}", sessions);
    println!("  halted        : {}", halted_sessions);
    println!("── output A/B holdout ──────────────────────────────");
    println!(
        "  shaped :  {:>8} req  {:>12} tokens",
        shaped_requests, shaped_output_tokens
    );
    println!(
        "  holdout:  {:>8} req  {:>12} tokens",
        holdout_requests, holdout_output_tokens
    );
    println!("────────────────────────────────────────────────────");
}

fn format_uptime(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_uptime_zero() {
        assert_eq!(format_uptime(0), "00:00:00");
    }

    #[test]
    fn format_uptime_nonzero() {
        assert_eq!(format_uptime(3661), "01:01:01");
    }

    #[test]
    fn render_does_not_panic_on_empty_json() {
        // render must never panic on missing or null fields
        render(&serde_json::Value::Object(serde_json::Map::new()));
    }

    #[test]
    fn render_parses_full_stats_json() {
        let raw = r#"{
            "requests": 1234,
            "input_tokens": 500000,
            "net_tokens": 350000,
            "dropped_tokens": 150000,
            "savings_ratio": 0.30,
            "sessions": 42,
            "halted_sessions": 3,
            "output": {
                "shaped_requests": 900,
                "shaped_output_tokens": 450000,
                "holdout_requests": 100,
                "holdout_output_tokens": 60000
            },
            "enabled": true,
            "recency_keep": 4,
            "uptime_secs": 7261
        }"#;
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        // Must not panic
        render(&v);
        assert_eq!(v["requests"].as_u64().unwrap(), 1234);
        assert_eq!(v["uptime_secs"].as_u64().unwrap(), 7261);
    }
}
