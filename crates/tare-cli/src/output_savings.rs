//! `tare output-savings` — compute mean output token reduction between the shaped and holdout
//! A/B arms with a 95% confidence interval (normal / Poisson approximation).

use crate::admin::admin_get;

pub struct OutputSavingsOpts {
    pub port: u16,
}

pub fn run(opts: OutputSavingsOpts) {
    match admin_get(opts.port, "/admin/stats") {
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
        Ok(v) => compute_and_print(&v),
    }
}

pub fn compute_and_print(v: &serde_json::Value) {
    let shaped_requests = v["output"]["shaped_requests"].as_u64().unwrap_or(0);
    let shaped_output_tokens = v["output"]["shaped_output_tokens"].as_u64().unwrap_or(0);
    let holdout_requests = v["output"]["holdout_requests"].as_u64().unwrap_or(0);
    let holdout_output_tokens = v["output"]["holdout_output_tokens"].as_u64().unwrap_or(0);

    if holdout_requests == 0 {
        println!("Holdout arm is empty — output savings A/B requires TARE_OUTPUT_HOLDOUT > 0.");
        println!("Set TARE_OUTPUT_HOLDOUT=0.1 (or higher) and restart the proxy.");
        return;
    }

    if shaped_requests == 0 {
        println!("Shaped arm is empty — no requests recorded yet.");
        return;
    }

    match compute_reduction(
        shaped_requests,
        shaped_output_tokens,
        holdout_requests,
        holdout_output_tokens,
    ) {
        Some(r) => {
            println!(
                "Output reduction: {:.1}% (95% CI {:.1}%..{:.1}%) [n_shaped={}, n_holdout={}]",
                r.reduction_pct, r.ci_lo_pct, r.ci_hi_pct, shaped_requests, holdout_requests,
            );
        }
        None => {
            println!("Insufficient data to compute reduction (zero tokens in one arm).");
        }
    }
}

pub struct ReductionResult {
    pub reduction_pct: f64,
    pub ci_lo_pct: f64,
    pub ci_hi_pct: f64,
}

/// Compute the output token reduction between shaped (treatment) and holdout (control) arms.
///
/// Reduction is defined as `1 - mean_shaped / mean_holdout` — positive means shaped emits fewer
/// tokens than the uncompressed holdout.
///
/// The 95% CI uses the normal approximation with a Poisson variance assumption
/// (Var(X_i) ≈ mean(X_i)) propagated through the ratio via the delta method:
///
///   Var(1 - s/h) ≈ (1/h)² · (mean_s/n_s) + (mean_s/h²)² · (mean_h/n_h)
///
/// Returns `None` if either arm has zero tokens (division impossible).
pub fn compute_reduction(
    n_s: u64,
    tokens_s: u64,
    n_h: u64,
    tokens_h: u64,
) -> Option<ReductionResult> {
    if n_s == 0 || n_h == 0 || tokens_h == 0 {
        return None;
    }

    let mean_s = tokens_s as f64 / n_s as f64;
    let mean_h = tokens_h as f64 / n_h as f64;

    if mean_h == 0.0 {
        return None;
    }

    let reduction = 1.0 - mean_s / mean_h;

    // Poisson: Var(sample_mean) = mean / n
    // Delta method on f(mean_s, mean_h) = 1 - mean_s / mean_h:
    //   df/d(mean_s) = -1/mean_h
    //   df/d(mean_h) =  mean_s / mean_h²
    //   Var(f) = (1/mean_h)² · (mean_s/n_s) + (mean_s/mean_h²)² · (mean_h/n_h)
    let var_ms = mean_s / n_s as f64;
    let var_mh = mean_h / n_h as f64;
    let var_reduction = var_ms / (mean_h * mean_h) + (mean_s * mean_s) / (mean_h.powi(4)) * var_mh;
    let se = var_reduction.sqrt();

    let margin = 1.96 * se;
    Some(ReductionResult {
        reduction_pct: reduction * 100.0,
        ci_lo_pct: (reduction - margin) * 100.0,
        ci_hi_pct: (reduction + margin) * 100.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_holdout_returns_none() {
        assert!(compute_reduction(100, 5_000, 0, 0).is_none());
    }

    #[test]
    fn zero_holdout_tokens_returns_none() {
        assert!(compute_reduction(100, 5_000, 50, 0).is_none());
    }

    #[test]
    fn identical_arms_zero_reduction() {
        let r = compute_reduction(1000, 50_000, 1000, 50_000).unwrap();
        assert!(
            r.reduction_pct.abs() < 0.01,
            "identical arms → ~0% reduction, got {:.4}",
            r.reduction_pct
        );
    }

    #[test]
    fn thirty_pct_reduction_correct() {
        // shaped: 700 tok/req × 1000 req; holdout: 1000 tok/req × 1000 req → 30% reduction
        let r = compute_reduction(1000, 700_000, 1000, 1_000_000).unwrap();
        assert!(
            (r.reduction_pct - 30.0).abs() < 0.01,
            "expected 30% reduction, got {:.4}",
            r.reduction_pct
        );
        // CI half-width should be well under 5% for n=1000
        let half_width = (r.ci_hi_pct - r.ci_lo_pct) / 2.0;
        assert!(half_width < 5.0, "CI too wide: half_width={half_width:.4}");
        // CI must straddle the point estimate
        assert!(r.ci_lo_pct < r.reduction_pct);
        assert!(r.ci_hi_pct > r.reduction_pct);
    }

    #[test]
    fn parse_stats_json_and_compute() {
        // shaped mean = 400000/800 = 500; holdout mean = 120000/200 = 600 → reduction = 1/6 ≈ 16.67%
        let raw = r#"{
            "requests": 500,
            "input_tokens": 100000,
            "net_tokens": 70000,
            "dropped_tokens": 30000,
            "savings_ratio": 0.30,
            "sessions": 10,
            "halted_sessions": 1,
            "output": {
                "shaped_requests": 800,
                "shaped_output_tokens": 400000,
                "holdout_requests": 200,
                "holdout_output_tokens": 120000
            },
            "enabled": true,
            "recency_keep": 4,
            "uptime_secs": 3600
        }"#;
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();

        let n_s = v["output"]["shaped_requests"].as_u64().unwrap();
        let tok_s = v["output"]["shaped_output_tokens"].as_u64().unwrap();
        let n_h = v["output"]["holdout_requests"].as_u64().unwrap();
        let tok_h = v["output"]["holdout_output_tokens"].as_u64().unwrap();

        let r = compute_reduction(n_s, tok_s, n_h, tok_h).unwrap();
        assert!(
            (r.reduction_pct - 100.0 / 6.0).abs() < 0.01,
            "expected ~16.67%, got {:.4}",
            r.reduction_pct
        );
        assert!(r.ci_lo_pct < r.reduction_pct);
        assert!(r.ci_hi_pct > r.reduction_pct);
    }

    #[test]
    fn compute_and_print_on_empty_holdout_does_not_panic() {
        let raw = r#"{
            "output": {
                "shaped_requests": 100,
                "shaped_output_tokens": 50000,
                "holdout_requests": 0,
                "holdout_output_tokens": 0
            }
        }"#;
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        // Should print a message about needing TARE_OUTPUT_HOLDOUT, not panic.
        compute_and_print(&v);
    }
}
