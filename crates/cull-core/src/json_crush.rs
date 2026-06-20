//! Lossless columnar compaction for repetitive JSON arrays (the structured-tool-output case).
//!
//! A JSON array of similar objects wastes most of its bytes repeating key names and constant
//! fields. This transform applies two lossless levers:
//! 1. **key elision** — the modal key-set is emitted once; each uniform row becomes a bare value
//!    array (keys dropped).
//! 2. **constant-column factoring** — modal keys whose value is identical across every uniform row
//!    are emitted once in a `constants` object and dropped from each row.
//! Objects that don't match the modal shape (anomalies/needles) are emitted verbatim. It is
//! **value-lossless**: `expand(crush(x))` equals `x` as a `serde_json::Value` — every field is
//! recovered, not just "needles". That is strictly stronger than schema-dedup approaches that drop
//! non-needle fields to hit their ratio.

use serde_json::Value;

const MARKER: &str = "\u{27ea}jc1\u{27eb}"; // ⟪jc1⟫ — unlikely to collide with real content

/// Columnar-encode a JSON array of objects when beneficial (see module docs). Returns `None` if the
/// text is not a JSON array of at least 3 objects, or if the encoding would not be smaller.
pub fn crush(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    let arr = value.as_array()?;
    if arr.len() < 3 || !arr.iter().all(Value::is_object) {
        return None;
    }

    // Modal key-signature: the most common ordered key vector across the objects.
    let mut counts: std::collections::HashMap<Vec<String>, usize> = std::collections::HashMap::new();
    for obj in arr {
        let keys: Vec<String> = obj.as_object().unwrap().keys().cloned().collect();
        *counts.entry(keys).or_insert(0) += 1;
    }
    let modal: Vec<String> = counts.into_iter().max_by_key(|(_, c)| *c)?.0;
    if modal.is_empty() {
        return None;
    }

    let is_modal = |obj: &Value| -> bool {
        let map = obj.as_object().unwrap();
        map.len() == modal.len() && map.keys().zip(&modal).all(|(k, m)| k == m)
    };

    // Constant columns: modal keys whose value is identical across every modal row.
    let modal_rows: Vec<&serde_json::Map<String, Value>> =
        arr.iter().filter(|o| is_modal(o)).map(|o| o.as_object().unwrap()).collect();
    let mut constants = serde_json::Map::new();
    if modal_rows.len() >= 2 {
        for k in &modal {
            let first = &modal_rows[0][k];
            if modal_rows.iter().all(|r| &r[k] == first) {
                constants.insert(k.clone(), first.clone());
            }
        }
    }
    // Varying keys = modal keys that are not constant (preserve modal order).
    let var_keys: Vec<&String> = modal.iter().filter(|k| !constants.contains_key(*k)).collect();

    let mut out = String::from(MARKER);
    out.push_str(&serde_json::to_string(&var_keys).ok()?);
    out.push('\n');
    out.push_str(&serde_json::to_string(&Value::Object(constants)).ok()?); // constants line ({} if none)
    for obj in arr {
        out.push('\n');
        if is_modal(obj) {
            let map = obj.as_object().unwrap();
            let vals: Vec<&Value> = var_keys.iter().map(|k| &map[*k]).collect();
            out.push_str(&serde_json::to_string(&vals).ok()?);
        } else {
            out.push_str(&serde_json::to_string(obj).ok()?); // exception row, verbatim
        }
    }

    if out.len() < text.len() {
        Some(out)
    } else {
        None
    }
}

/// Reverse [`crush`], returning the reconstructed JSON value (value-lossless).
pub fn expand(crushed: &str) -> Option<Value> {
    let rest = crushed.strip_prefix(MARKER)?;
    let mut lines = rest.split('\n');
    let var_keys: Vec<String> = serde_json::from_str(lines.next()?).ok()?;
    let constants: serde_json::Map<String, Value> =
        serde_json::from_str::<Value>(lines.next()?).ok()?.as_object()?.clone();

    let mut out: Vec<Value> = Vec::new();
    for line in lines {
        let v: Value = serde_json::from_str(line).ok()?;
        match v {
            Value::Array(vals) => {
                if vals.len() != var_keys.len() {
                    return None;
                }
                let mut map = constants.clone();
                for (k, val) in var_keys.iter().zip(vals) {
                    map.insert(k.clone(), val);
                }
                out.push(Value::Object(map));
            }
            Value::Object(_) => out.push(v), // exception row, verbatim
            _ => return None,
        }
    }
    Some(Value::Array(out))
}

/// True iff `crushed` reconstructs to the same JSON value as `original` (the losslessness check).
pub fn round_trips(original: &str, crushed: &str) -> bool {
    match (serde_json::from_str::<Value>(original), expand(crushed)) {
        (Ok(orig), Some(rebuilt)) => orig == rebuilt,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crushes_uniform_array_and_round_trips() {
        let text = serde_json::to_string_pretty(&serde_json::json!([
            {"id":0,"name":"item_0","value":0.0,"status":"active"},
            {"id":1,"name":"item_1","value":1.5,"status":"active"},
            {"id":2,"name":"item_2","value":3.0,"status":"active"},
            {"id":3,"name":"item_3","value":4.5,"status":"active"}
        ])).unwrap();
        let crushed = crush(&text).expect("uniform array should crush");
        assert!(crushed.len() < text.len(), "crushed smaller");
        assert!(round_trips(&text, &crushed), "value-lossless round trip");
    }

    #[test]
    fn factors_constant_column() {
        // "status" is constant across all rows -> must be factored into the constants line.
        let text = serde_json::to_string(&serde_json::json!([
            {"id":0,"status":"healthy"},{"id":1,"status":"healthy"},{"id":2,"status":"healthy"}
        ])).unwrap();
        let crushed = crush(&text).expect("crush");
        assert!(crushed.contains("\"status\":\"healthy\""), "constant factored: {crushed}");
        // and "status" appears exactly once (factored, not per-row)
        assert_eq!(crushed.matches("status").count(), 1, "status emitted once: {crushed}");
        assert!(round_trips(&text, &crushed));
    }

    #[test]
    fn preserves_needle_rows_and_all_fields() {
        let text = serde_json::to_string_pretty(&serde_json::json!([
            {"id":0,"name":"item_0","value":0.0,"status":"active"},
            {"id":1,"name":"item_1","value":1.5,"status":"active"},
            {"id":2,"name":"item_2","value":3.0,"status":"active"},
            {"id":3,"name":"item_3","value":999.99,"status":"error","error_code":"ERR-2007","task_id":"task-0007"}
        ])).unwrap();
        let crushed = crush(&text).expect("crush");
        assert!(crushed.contains("ERR-2007"), "needle present: {crushed}");
        assert!(crushed.contains("999.99"));
        assert!(round_trips(&text, &crushed));
    }

    #[test]
    fn refuses_non_array_or_tiny() {
        assert!(crush(r#"{"a":1}"#).is_none());
        assert!(crush(r#"[1,2,3]"#).is_none());
        assert!(crush(r#"[{"a":1}]"#).is_none());
        assert!(crush("not json").is_none());
    }

    #[test]
    fn expand_rejects_garbage() {
        assert!(expand("no marker").is_none());
    }
}
