//! Opt-in LOSSY compaction of large JSON arrays — Cull's aggressive mode, off by default.
//!
//! Lossless columnar compaction (`json_crush`) keeps every row; on very large uniform arrays that
//! leaves more tokens than incumbents (e.g. Headroom's SmartCrusher) which DROP rows. This module
//! provides the same lossy lever, opt-in and explicit (`cull compact-lossy`), so Cull can match or
//! beat their ratio when the caller accepts loss — while the default `compress` stays lossless.
//!
//! Strategy (mirrors SmartCrusher's stated policy, but keeps MORE that matters): keep the first and
//! last `boundary` rows (schema + recency), keep every row that is anomalous (a key-set differing
//! from the modal shape, or containing an alert keyword like error/fatal/critical), drop the rest of
//! the uniform bulk, and append an explicit `[cull-lossy: M of N rows elided]` marker. Kept rows are
//! emitted compactly. Works on a top-level array or the dominant nested array.

use serde_json::Value;

const ALERTS: &[&str] = &["error", "fail", "critical", "fatal", "warn", "exception", "denied", "timeout"];

fn is_anomaly(row: &Value, modal_keys: &[String]) -> bool {
    let Some(obj) = row.as_object() else { return true };
    // shape anomaly: key-set differs from the modal row
    if obj.len() != modal_keys.len() || !obj.keys().zip(modal_keys).all(|(k, m)| k == m) {
        return true;
    }
    // content anomaly: an alert keyword anywhere in the row
    let s = row.to_string().to_ascii_lowercase();
    ALERTS.iter().any(|a| s.contains(a))
}

fn modal_keys(arr: &[Value]) -> Vec<String> {
    let mut counts: std::collections::HashMap<Vec<String>, usize> = std::collections::HashMap::new();
    for o in arr {
        if let Some(m) = o.as_object() {
            *counts.entry(m.keys().cloned().collect()).or_insert(0) += 1;
        }
    }
    counts.into_iter().max_by_key(|(_, c)| *c).map(|(k, _)| k).unwrap_or_default()
}

/// Aggressively (lossily) compact `text`: a JSON array of objects, or — failing that — line-based
/// (logs, tabular commands) or sentence-based (prose) content. When `task` is given, units relevant
/// to the task (sharing a task symbol) are always kept — query-aware pruning, which base LLMLingua-2
/// (query-agnostic) does not do. Returns `None` if not worth it / not smaller.
pub fn compact(text: &str, boundary: usize, task: Option<&str>) -> Option<String> {
    let syms = task.map(crate::task::extract_symbols).unwrap_or_default();
    match serde_json::from_str::<Value>(text) {
        Ok(v) if v.is_array() => compact_json_array(&v, boundary, &syms).or_else(|| compact_text(text, boundary, &syms)),
        _ => compact_text(text, boundary, &syms),
    }
}

type Syms = std::collections::HashSet<String>;

fn relevant(unit: &str, syms: &Syms) -> bool {
    if syms.is_empty() {
        return false;
    }
    let l = unit.to_ascii_lowercase();
    syms.iter().any(|s| l.contains(s.as_str()))
}

/// Split prose into sentences, each slice keeping its terminal `.`/`!`/`?` (lossy-join safe).
fn split_sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    for i in 0..bytes.len() {
        let end = matches!(bytes[i], b'.' | b'!' | b'?') && (i + 1 >= bytes.len() || bytes[i + 1] == b' ' || bytes[i + 1] == b'\n');
        if end {
            out.push(text[start..=i].trim_start());
            start = i + 1;
        }
    }
    if start < text.len() {
        let tail = text[start..].trim_start();
        if !tail.is_empty() {
            out.push(tail);
        }
    }
    out
}

/// Lossy compaction of free text: line units (logs/tables) when multi-line, else sentence units
/// (prose). Keeps the first/last `boundary` units, any unit with an alert keyword, and any unit
/// relevant to the task; drops the rest with an explicit marker.
fn compact_text(text: &str, boundary: usize, syms: &Syms) -> Option<String> {
    let multiline = text.matches('\n').count() >= 4;
    let (units, joiner, label): (Vec<&str>, &str, &str) = if multiline {
        (text.split('\n').collect(), "\n", "lines")
    } else {
        (split_sentences(text), " ", "sentences")
    };
    let n = units.len();
    if n <= 2 * boundary + 4 {
        return None;
    }
    let mut keep = vec![false; n];
    for k in keep.iter_mut().take(boundary.min(n)) { *k = true; }
    for k in keep.iter_mut().skip(n.saturating_sub(boundary)) { *k = true; }
    for (i, u) in units.iter().enumerate() {
        let l = u.to_ascii_lowercase();
        if ALERTS.iter().any(|a| l.contains(a)) || relevant(u, syms) {
            keep[i] = true;
        }
    }
    let kept: Vec<&str> = units.iter().zip(&keep).filter(|(_, k)| **k).map(|(u, _)| *u).collect();
    let dropped = n - kept.len();
    if dropped == 0 {
        return None;
    }
    let out = format!("{}\n[cull-lossy: {dropped} of {n} {label} elided; kept boundary+alerts+relevant]",
        kept.join(joiner));
    if out.len() < text.len() { Some(out) } else { None }
}

fn compact_json_array(value: &Value, boundary: usize, syms: &Syms) -> Option<String> {
    let arr = value.as_array()?;
    let n = arr.len();
    if n <= 2 * boundary + 2 || !arr.iter().all(Value::is_object) {
        return None; // too small, or not an array of objects
    }
    let modal = modal_keys(arr);

    // keep set: boundary head + boundary tail + all anomalies
    let mut keep = vec![false; n];
    for k in keep.iter_mut().take(boundary.min(n)) {
        *k = true;
    }
    for k in keep.iter_mut().skip(n.saturating_sub(boundary)) {
        *k = true;
    }
    for (i, row) in arr.iter().enumerate() {
        if is_anomaly(row, &modal) || relevant(&row.to_string(), syms) {
            keep[i] = true;
        }
    }
    let kept: Vec<&Value> = arr.iter().zip(&keep).filter(|(_, k)| **k).map(|(r, _)| r).collect();
    let dropped = n - kept.len();
    if dropped == 0 {
        return None; // nothing elided (all anomalous) — the lossless path is better
    }

    // render: compact kept rows + an explicit elision marker
    let mut out = String::new();
    for (j, row) in kept.iter().enumerate() {
        if j > 0 {
            out.push('\n');
        }
        out.push_str(&serde_json::to_string(row).ok()?);
    }
    out.push_str(&format!("\n[cull-lossy: {dropped} of {n} uniform rows elided; kept boundary+anomalies]"));

    // worth it only if smaller than the compact original
    if out.len() < value.to_string().len() {
        Some(out)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_uniform_bulk_keeps_anomalies_and_boundaries() {
        let mut rows: Vec<Value> = (0..100).map(|i| serde_json::json!(
            {"id":i,"name":format!("u{i}"),"status":"ok"})).collect();
        rows[50] = serde_json::json!({"id":50,"name":"u50","status":"critical","error":"ERR_X"});
        let text = serde_json::to_string(&Value::Array(rows)).unwrap();
        let out = compact(&text, 3, None).expect("should compact");
        assert!(out.len() < text.len());
        // anomaly kept
        assert!(out.contains("ERR_X") && out.contains("critical"));
        // boundaries kept
        assert!(out.contains("\"id\":0") && out.contains("\"id\":99"));
        // bulk dropped + marker
        assert!(out.contains("rows elided"));
        assert!(!out.contains("\"id\":40"), "a uniform middle row was dropped");
    }

    #[test]
    fn refuses_small_arrays() {
        let text = serde_json::to_string(&serde_json::json!([{"a":1},{"a":2},{"a":3}])).unwrap();
        assert!(compact(&text, 3, None).is_none());
    }
}
