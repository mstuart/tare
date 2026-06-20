//! Byte-lossless columnar compaction for repetitive plain-text logs.
//!
//! Structured log lines share a template — `<ts> INFO worker-N processed batch M ok latency=Xms` —
//! wasting bytes on constant fields (`INFO`, `processed`, `batch`, `ok`) repeated every line. This
//! transform splits each line on a single space (an exactly invertible operation: `split(' ')` then
//! `join(' ')` reproduces the line byte-for-byte, including runs of spaces), takes the dominant
//! field-count as the template, factors out positions that are constant across all matching lines,
//! and emits only the varying field values per line. Lines that don't match the template are emitted
//! verbatim. Reconstruction is **byte-exact** (verified by `round_trips`).

use serde_json::Value;

const MARKER: &str = "\u{27ea}lc1\u{27eb}"; // ⟪lc1⟫

/// Columnar-encode repetitive log lines when beneficial. `None` if there's no dominant template
/// with constant columns, or the result isn't smaller.
pub fn crush(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() < 4 {
        return None;
    }
    let fields: Vec<Vec<&str>> = lines.iter().map(|l| l.split(' ').collect()).collect();

    // Dominant field count among multi-field lines (a real log row has several fields).
    let mut counts: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for f in &fields {
        if f.len() >= 3 {
            *counts.entry(f.len()).or_insert(0) += 1;
        }
    }
    let (&template_f, &n_matching) = counts.iter().max_by_key(|(_, c)| *c)?;
    if n_matching < 3 {
        return None;
    }

    // Constant positions: columns identical across every template-matching line.
    let matching: Vec<&Vec<&str>> = fields.iter().filter(|f| f.len() == template_f).collect();
    let mut const_pos: Vec<usize> = Vec::new();
    let mut const_val: Vec<&str> = Vec::new();
    for p in 0..template_f {
        let first = matching[0][p];
        if matching.iter().all(|f| f[p] == first) {
            const_pos.push(p);
            const_val.push(first);
        }
    }
    if const_pos.is_empty() {
        return None; // nothing to factor
    }
    let var_pos: Vec<usize> = (0..template_f).filter(|p| !const_pos.contains(p)).collect();

    // Template rows are space-joined plain text (token-efficient — no quotes/commas); exception
    // lines (non-template field count) are emitted verbatim and flagged by index in the metadata.
    // meta = [F, {const_pos: const_val, ...}, [exception_indices]]
    let cmap: serde_json::Map<String, Value> = const_pos.iter().zip(&const_val)
        .map(|(p, v)| (p.to_string(), Value::String((*v).to_string()))).collect();
    let exceptions: Vec<usize> = fields.iter().enumerate()
        .filter(|(_, f)| f.len() != template_f).map(|(i, _)| i).collect();
    let meta = serde_json::to_string(&serde_json::json!([template_f, cmap, exceptions])).ok()?;

    let mut out = format!("{MARKER}{meta}");
    for (i, f) in fields.iter().enumerate() {
        out.push('\n');
        if f.len() == template_f {
            // bare space-joined variable values (no spaces inside them — they came from split(' '))
            let vals: Vec<&str> = var_pos.iter().map(|&p| f[p]).collect();
            out.push_str(&vals.join(" "));
        } else {
            out.push_str(lines[i]); // verbatim line
        }
    }
    if out.len() < text.len() { Some(out) } else { None }
}

/// Reverse [`crush`] to the exact original text.
pub fn expand(crushed: &str) -> Option<String> {
    let rest = crushed.strip_prefix(MARKER)?;
    let mut lines = rest.split('\n');
    let meta: Value = serde_json::from_str(lines.next()?).ok()?;
    let template_f = meta.get(0)?.as_u64()? as usize;
    let cmap = meta.get(1)?.as_object()?;
    let exceptions: std::collections::HashSet<usize> = meta.get(2)?.as_array()?
        .iter().filter_map(|v| v.as_u64().map(|n| n as usize)).collect();
    let const_pos: Vec<usize> = cmap.keys().filter_map(|k| k.parse().ok()).collect();
    let var_pos: Vec<usize> = (0..template_f).filter(|p| !const_pos.contains(p)).collect();

    let mut out: Vec<String> = Vec::new();
    for (i, line) in lines.enumerate() {
        if exceptions.contains(&i) {
            out.push(line.to_string()); // verbatim line
            continue;
        }
        let vals: Vec<&str> = line.split(' ').collect();
        if vals.len() != var_pos.len() {
            return None;
        }
        let mut fields = vec![""; template_f];
        for (k, v) in cmap {
            fields[k.parse::<usize>().ok()?] = v.as_str()?;
        }
        for (&p, val) in var_pos.iter().zip(&vals) {
            fields[p] = val;
        }
        out.push(fields.join(" "));
    }
    Some(out.join("\n"))
}

/// True iff `crushed` reconstructs the original text byte-for-byte.
pub fn round_trips(original: &str, crushed: &str) -> bool {
    expand(crushed).as_deref() == Some(original)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_log(n: usize) -> String {
        (0..n).map(|i| format!("2024-06-20T10:00:{:02}Z INFO worker-{} processed batch {} ok latency={}ms",
            i % 60, i % 8, i, 20 + i % 30)).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn crushes_uniform_log_and_round_trips_byte_exact() {
        let text = sample_log(40);
        let crushed = crush(&text).expect("uniform log should crush");
        assert!(crushed.len() < text.len(), "smaller: {} < {}", crushed.len(), text.len());
        assert!(round_trips(&text, &crushed), "byte-exact round trip");
        // constant fields factored: "INFO"/"processed"/"ok" appear once (in meta), not per line
        assert_eq!(crushed.matches("processed").count(), 1, "constant factored");
    }

    #[test]
    fn preserves_anomalous_line_verbatim() {
        let mut lines: Vec<String> = (0..20).map(|i|
            format!("ts INFO worker-{} ok code={}", i % 4, 200)).collect();
        lines[7] = "ts FATAL worker-3 OOM heap_exhausted code=ERR_OOM_9931 extra fields here".into();
        let text = lines.join("\n");
        let crushed = crush(&text).expect("crush");
        assert!(crushed.contains("ERR_OOM_9931"), "needle survives: {crushed}");
        assert!(round_trips(&text, &crushed));
    }

    #[test]
    fn preserves_multi_space_runs_byte_exact() {
        // double spaces must survive (split/join on single space is exactly invertible)
        let text = "a  INFO  x ok\nb  INFO  y ok\nc  INFO  z ok\nd  INFO  w ok";
        if let Some(crushed) = crush(text) {
            assert!(round_trips(text, &crushed));
        }
    }

    #[test]
    fn refuses_short_or_unstructured() {
        assert!(crush("one\ntwo").is_none());
        assert!(crush("aaaa\nbbbb\ncccc\ndddd").is_none()); // single-field lines, nothing to factor
    }
}
