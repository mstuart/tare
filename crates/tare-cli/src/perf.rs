//! `tare perf` — measure compression savings and wall-clock speed on files or built-in samples.
//!
//! Each input is classified by extension and run through the applicable lossless and lossy passes.
//! Results are printed as a table with original tokens, compressed tokens, ratios, and timing.

use std::io::Read;
use std::time::Instant;
use tare_core::{code_skeleton, json_crush, log_crush, lossy_compact, telegraphic};
use tare_tokenize::{ApproxCounter, TokenCounter};

// ── Built-in sample corpus ───────────────────────────────────────────────────

const SAMPLE_JSON: &str = r#"[{"id":0,"name":"item_0","status":"active","count":100},{"id":1,"name":"item_1","status":"active","count":200},{"id":2,"name":"item_2","status":"inactive","count":50},{"id":3,"name":"item_3","status":"active","count":300},{"id":4,"name":"item_4","status":"active","count":400}]"#;

const SAMPLE_LOG: &str = "\
2024-01-15T10:00:01Z INFO  server started port=8080\n\
2024-01-15T10:00:02Z INFO  server started port=8080\n\
2024-01-15T10:00:03Z INFO  server started port=8080\n\
2024-01-15T10:00:04Z WARN  high memory usage pct=85\n\
2024-01-15T10:00:05Z INFO  server started port=8080\n\
2024-01-15T10:00:06Z INFO  server started port=8080\n\
2024-01-15T10:00:07Z ERROR connection refused addr=db:5432\n\
2024-01-15T10:00:08Z INFO  server started port=8080\n\
2024-01-15T10:00:09Z INFO  server started port=8080\n\
2024-01-15T10:00:10Z INFO  request ok path=/health status=200\n";

const SAMPLE_CODE: &str = "\
fn compute(a: i32, b: i32) -> i32 {\n\
    let x = a * 2;\n\
    let y = b + x;\n\
    let z = y - 1;\n\
    z\n\
}\n\
\n\
fn greet(name: &str) -> String {\n\
    let msg = format!(\"Hello, {}!\", name);\n\
    println!(\"{}\", msg);\n\
    msg\n\
}\n";

const SAMPLE_PROSE: &str = "\
Tare is a query-aware, cache-correct, lossless-by-default context compression system for LLM coding \
agents. It segments the context window into typed blocks, applies a structured pass pipeline, and emits \
only the segments that survive relevance, supersession, dedup, and IVM-delta checks. Lossy passes such \
as skeletonization and tabular compaction are always opt-in and never applied by default.";

// ── Public types ─────────────────────────────────────────────────────────────

pub struct PerfRow {
    pub label: String,
    pub orig_tokens: usize,
    /// Lossless compressed token count (None = no applicable lossless pass).
    pub lossless_tokens: Option<usize>,
    /// Lossy compressed token count (None = no applicable lossy pass or no benefit).
    pub lossy_tokens: Option<usize>,
    /// Wall-clock time for the lossless pass in microseconds.
    pub lossless_us: u128,
}

#[allow(dead_code)]
pub struct PerfReport {
    pub rows: Vec<PerfRow>,
}

impl PerfReport {
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

// ── Internal helpers ─────────────────────────────────────────────────────────

fn tok(s: &str) -> usize {
    ApproxCounter::o200k().count(s)
}

fn ratio(orig: usize, compressed: usize) -> String {
    if compressed == 0 || orig == 0 {
        return "—".into();
    }
    format!("{:.2}x", orig as f64 / compressed as f64)
}

fn measure_json(label: &str, input: &str) -> PerfRow {
    let orig = tok(input);
    let t0 = Instant::now();
    let lossless = json_crush::crush(input).map(|c| tok(&c));
    let elapsed = t0.elapsed().as_micros();
    // lossy: compact_opts is a separate pass — only report if it actually reduces vs lossless
    let lossy = lossy_compact::compact_opts(input, 3, None, 0, 0).map(|c| {
        let ct = tok(&c);
        // only credit it if it beats the lossless baseline
        match lossless {
            Some(ll) if ct >= ll => ll,
            _ => ct,
        }
    });
    PerfRow {
        label: label.to_string(),
        orig_tokens: orig,
        lossless_tokens: lossless,
        lossy_tokens: lossy,
        lossless_us: elapsed,
    }
}

fn measure_log(label: &str, input: &str) -> PerfRow {
    let orig = tok(input);
    let t0 = Instant::now();
    let lossless = log_crush::crush(input).map(|c| tok(&c));
    let elapsed = t0.elapsed().as_micros();
    PerfRow {
        label: label.to_string(),
        orig_tokens: orig,
        lossless_tokens: lossless,
        lossy_tokens: None,
        lossless_us: elapsed,
    }
}

fn measure_code(label: &str, input: &str, path: &str) -> PerfRow {
    let orig = tok(input);
    // no lossless pass for source code; skeletonize is the lossy opt-in
    let t0 = Instant::now();
    let lossy = code_skeleton::skeletonize(input, path).map(|s| tok(&s));
    let elapsed = t0.elapsed().as_micros();
    PerfRow {
        label: label.to_string(),
        orig_tokens: orig,
        lossless_tokens: None,
        lossy_tokens: lossy,
        lossless_us: elapsed,
    }
}

fn measure_prose(label: &str, input: &str) -> PerfRow {
    let orig = tok(input);
    // telegraphic compact is lossy (stopword dropping); no lossless prose pass
    let t0 = Instant::now();
    let lossy = telegraphic::compact(input).map(|s| tok(&s));
    let elapsed = t0.elapsed().as_micros();
    PerfRow {
        label: label.to_string(),
        orig_tokens: orig,
        lossless_tokens: None,
        lossy_tokens: lossy,
        lossless_us: elapsed,
    }
}

// ── Reporting ────────────────────────────────────────────────────────────────

fn print_report(rows: &[PerfRow]) {
    const LBL: usize = 32;
    const NUM: usize = 8;
    println!(
        "{:<LBL$}  {:>NUM$}  {:>NUM$}  {:>8}  {:>NUM$}  {:>10}",
        "Source", "Orig", "Lossless", "LL Ratio", "Lossy", "Time(µs)"
    );
    let sep = "-".repeat(LBL + 4 * (NUM + 2) + 12);
    println!("{sep}");

    let mut total_orig = 0usize;
    let mut total_ll = 0usize;
    let mut ll_count = 0usize;

    for r in rows {
        let ll_s = r
            .lossless_tokens
            .map(|t| t.to_string())
            .unwrap_or_else(|| "—".into());
        let ratio_s = r
            .lossless_tokens
            .map(|t| ratio(r.orig_tokens, t))
            .unwrap_or_else(|| "—".into());
        let lossy_s = r
            .lossy_tokens
            .map(|t| t.to_string())
            .unwrap_or_else(|| "—".into());
        println!(
            "{:<LBL$}  {:>NUM$}  {:>NUM$}  {:>8}  {:>NUM$}  {:>10}",
            r.label, r.orig_tokens, ll_s, ratio_s, lossy_s, r.lossless_us,
        );
        total_orig += r.orig_tokens;
        if let Some(t) = r.lossless_tokens {
            total_ll += t;
            ll_count += 1;
        }
    }

    println!("{sep}");
    let total_ratio = if ll_count > 0 {
        ratio(total_orig, total_ll)
    } else {
        "—".into()
    };
    println!(
        "{:<LBL$}  {:>NUM$}  {:>NUM$}  {:>8}",
        "TOTAL", total_orig, total_ll, total_ratio,
    );
    println!();
    println!("Token counts are approximate (chars/4).");
}

// ── Public entry points ──────────────────────────────────────────────────────

/// Run on the built-in representative sample corpus.
pub fn run_sample() -> PerfReport {
    let rows = vec![
        measure_json("JSON array (built-in sample)", SAMPLE_JSON),
        measure_log("Log lines (built-in sample)", SAMPLE_LOG),
        measure_code("Rust code (built-in sample)", SAMPLE_CODE, "sample.rs"),
        measure_prose("Prose (built-in sample)", SAMPLE_PROSE),
    ];
    print_report(&rows);
    PerfReport { rows }
}

/// Run on a file or directory. Files are classified by extension.
pub fn run_path(path: &std::path::Path) -> PerfReport {
    let paths: Vec<std::path::PathBuf> = if path.is_dir() {
        match std::fs::read_dir(path) {
            Ok(rd) => {
                let mut v: Vec<_> = rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_file())
                    .collect();
                v.sort(); // deterministic order
                v
            }
            Err(e) => {
                eprintln!("error reading directory {}: {e}", path.display());
                vec![]
            }
        }
    } else {
        vec![path.to_path_buf()]
    };

    let mut rows = Vec::new();
    for p in &paths {
        let mut buf = String::new();
        match std::fs::File::open(p) {
            Ok(mut f) => {
                if f.read_to_string(&mut buf).is_err() {
                    continue; // binary or unreadable
                }
            }
            Err(_) => continue,
        }
        if buf.is_empty() {
            continue;
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        let label = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let row = match ext {
            "json" => measure_json(&label, &buf),
            "log" => measure_log(&label, &buf),
            "rs" | "py" | "js" | "ts" | "tsx" | "go" => {
                measure_code(&label, &buf, p.to_str().unwrap_or(""))
            }
            _ => measure_prose(&label, &buf),
        };
        rows.push(row);
    }

    print_report(&rows);
    PerfReport { rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_report_is_non_empty_with_positive_orig_tokens() {
        let report = run_sample();
        assert!(!report.rows.is_empty(), "sample report must have rows");
        for row in &report.rows {
            assert!(
                row.orig_tokens > 0,
                "orig tokens must be > 0 for {:?}",
                row.label
            );
        }
    }
}
