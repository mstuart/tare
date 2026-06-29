//! `tare learn` — analyse files under a directory and write a learned compression profile.
//!
//! Classifies each file by extension, measures lossless and lossy compression ratios, then
//! derives and persists a [`tare_core::profile::Profile`]. Learning is fully deterministic
//! (files processed in sorted order, no RNG).

use std::io::Read;
use tare_core::profile::Profile;
use tare_core::{code_skeleton, json_crush, log_crush, lossy_compact, telegraphic};
use tare_tokenize::{ApproxCounter, TokenCounter};

fn tok(s: &str) -> usize {
    ApproxCounter::o200k().count(s)
}

enum FileClass {
    /// Source code; the `&'static str` is a synthetic path hint for language detection.
    Code(&'static str),
    Json,
    Log,
    Prose,
}

fn classify(ext: &str) -> FileClass {
    match ext {
        "rs" => FileClass::Code("x.rs"),
        "py" => FileClass::Code("x.py"),
        "js" => FileClass::Code("x.js"),
        "ts" => FileClass::Code("x.ts"),
        "tsx" => FileClass::Code("x.tsx"),
        "go" => FileClass::Code("x.go"),
        "json" => FileClass::Json,
        "log" => FileClass::Log,
        _ => FileClass::Prose,
    }
}

#[allow(dead_code)]
pub struct LearnResult {
    pub profile: Profile,
    pub profile_path: std::path::PathBuf,
    pub files_processed: usize,
}

/// Analyse files under `dir`, derive a profile, persist it, and print a human summary.
pub fn run(dir: &std::path::Path) -> Result<LearnResult, String> {
    // Collect and sort for determinism.
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read directory {}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    paths.sort();

    // Accumulators
    let mut total_orig: usize = 0;
    let mut total_lossless: usize = 0; // lossless compressed (or orig when no lossless pass)

    let mut code_orig: usize = 0;
    let mut code_skel: usize = 0; // skeletonized tokens

    let mut tab_savings_seen = false; // compact_opts reduced below json_crush baseline

    let mut files_processed: usize = 0;

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
        files_processed += 1;
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        let orig = tok(&buf);
        total_orig += orig;

        match classify(ext) {
            FileClass::Code(path_hint) => {
                code_orig += orig;
                // lossless baseline: fall back to orig if no lossless pass applies to code
                total_lossless += orig;
                if let Some(skel) = code_skeleton::skeletonize(&buf, path_hint) {
                    code_skel += tok(&skel);
                } else {
                    code_skel += orig; // no elision possible → no savings
                }
            }
            FileClass::Json => {
                let ll_toks = json_crush::crush(&buf).map(|c| tok(&c)).unwrap_or(orig);
                total_lossless += ll_toks;

                // Check whether compact_opts adds savings beyond json_crush.
                if let Some(compact) = lossy_compact::compact_opts(&buf, 3, None, 0, 0) {
                    if tok(&compact) < ll_toks {
                        tab_savings_seen = true;
                    }
                }
            }
            FileClass::Log => {
                let ll_toks = log_crush::crush(&buf).map(|c| tok(&c)).unwrap_or(orig);
                total_lossless += ll_toks;
            }
            FileClass::Prose => {
                // telegraphic is lossy; no lossless prose pass → count orig as lossless baseline
                let lossy_toks = telegraphic::compact(&buf).map(|c| tok(&c)).unwrap_or(orig);
                // Use lossy result as the "lossless" baseline for prose (it's the best we have).
                total_lossless += lossy_toks;
            }
        }
    }

    // ── Derive profile fields ────────────────────────────────────────────────

    let measured_ratio = if total_lossless > 0 && total_orig > 0 {
        total_orig as f64 / total_lossless as f64
    } else {
        1.0
    };

    // Recommend skeletonization if it saves ≥20% over original code tokens.
    let lossy_code = code_orig > 0 && code_skel < code_orig * 4 / 5;

    let (lossy_tabular_max_rows, lossy_tabular_max_field) =
        if tab_savings_seen { (50, 120) } else { (0, 0) };

    // Recency: we can't derive this from static files; use the documented sane default.
    let recommended_recency_keep: usize = 4;

    let summary = format!(
        "ratio={measured_ratio:.2}x files={files_processed} lossy_code={lossy_code} tabular={tab_savings_seen}"
    );
    let source = dir.to_string_lossy().into_owned();

    let profile = Profile {
        recommended_recency_keep,
        lossy_code,
        lossy_tabular_max_rows,
        lossy_tabular_max_field,
        measured_ratio,
        summary: summary.clone(),
        source: source.clone(),
    };

    tare_core::profile::save(&profile).map_err(|e| format!("failed to save profile: {e}"))?;

    let profile_path = tare_core::profile::path();

    println!("Learned profile from: {source}");
    println!("  files processed    : {files_processed}");
    println!("  measured ratio     : {measured_ratio:.3}x (lossless baseline)");
    println!("  recommend lossy_code: {lossy_code}");
    println!("  tabular savings     : {tab_savings_seen}");
    println!("  written to         : {}", profile_path.display());

    Ok(LearnResult {
        profile,
        profile_path,
        files_processed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn learn_writes_profile_that_load_reads_back() {
        // Hermetic: point TARE_PROFILE at a temp path so we never touch real config.
        let dir = std::env::temp_dir().join(format!("tare-learn-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // Write a small JSON file.
        {
            let mut f = std::fs::File::create(dir.join("data.json")).unwrap();
            writeln!(f, r#"[{{"id":1,"name":"a","v":1}},{{"id":2,"name":"b","v":2}},{{"id":3,"name":"c","v":3}}]"#).unwrap();
        }

        // Write a small Rust file with an elidable body.
        {
            let mut f = std::fs::File::create(dir.join("lib.rs")).unwrap();
            writeln!(
                f,
                "fn foo(x: i32) -> i32 {{\n    let a = x + 1;\n    let b = a * 2;\n    b\n}}\n"
            )
            .unwrap();
        }

        let profile_file = dir.join("profile.json");
        // SAFETY: test-only env mutation; tests within a process run sequentially per thread.
        std::env::set_var("TARE_PROFILE", &profile_file);

        let result = run(&dir).expect("learn must succeed on a readable directory");

        assert!(
            result.files_processed >= 1,
            "must process at least one file"
        );
        assert!(profile_file.exists(), "profile.json must be written");
        assert!(
            result.profile.measured_ratio > 0.0,
            "ratio must be positive"
        );

        let loaded = tare_core::profile::load().expect("profile must load after save");
        assert_eq!(
            loaded.summary, result.profile.summary,
            "saved and loaded summaries must match"
        );
        assert_eq!(loaded.source, result.profile.source);

        // cleanup
        std::env::remove_var("TARE_PROFILE");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
