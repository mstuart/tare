//! Lossless columnar compaction for repetitive JSON arrays — the structured-tool-output case.
//!
//! A JSON array of similar objects wastes most of its bytes repeating key names and constant
//! fields. Two lossless levers: **key elision** (modal key-set emitted once; uniform rows become
//! bare value arrays) and **constant-column factoring** (modal keys whose value is identical across
//! every uniform row are emitted once). Anomaly/needle rows (different shape) are emitted verbatim.
//!
//! The compactable array can be the whole input (`⟪jc1⟫`) OR the dominant array nested somewhere
//! inside a wrapper object (`⟪jc2⟫`, e.g. `{"status":"ok","data":{"results":[...]}}` — the common
//! API-response shape). It is **value-lossless**: `expand(crush(x))` equals `x` as a
//! `serde_json::Value` — every field recovered, not just flagged "needles".

use serde_json::{Map, Value};

const MARKER1: &str = "\u{27ea}jc1\u{27eb}"; // ⟪jc1⟫ — top-level array
const MARKER2: &str = "\u{27ea}jc2\u{27eb}"; // ⟪jc2⟫ — array nested at a path

/// One step in a path to a nested value: an object key or an array index.
#[derive(Clone)]
enum Seg {
    Key(String),
    Idx(usize),
}

fn path_to_json(path: &[Seg]) -> Value {
    Value::Array(path.iter().map(|s| match s {
        Seg::Key(k) => Value::String(k.clone()),
        Seg::Idx(i) => Value::from(*i),
    }).collect())
}

fn path_from_json(v: &Value) -> Option<Vec<Seg>> {
    v.as_array()?.iter().map(|seg| match seg {
        Value::String(k) => Some(Seg::Key(k.clone())),
        Value::Number(n) => n.as_u64().map(|i| Seg::Idx(i as usize)),
        _ => None,
    }).collect()
}

fn get_mut<'a>(root: &'a mut Value, path: &[Seg]) -> Option<&'a mut Value> {
    let mut cur = root;
    for seg in path {
        cur = match seg {
            Seg::Key(k) => cur.as_object_mut()?.get_mut(k)?,
            Seg::Idx(i) => cur.as_array_mut()?.get_mut(*i)?,
        };
    }
    Some(cur)
}

/// True for an array worth columnar-encoding: >= 3 elements, all objects.
fn is_crushable_array(v: &Value) -> bool {
    matches!(v.as_array(), Some(a) if a.len() >= 3 && a.iter().all(Value::is_object))
}

/// Find the path to the largest (by serialized length) crushable array anywhere in `v`. Returns the
/// empty path if `v` itself is the crushable array.
fn largest_array_path(v: &Value) -> Option<Vec<Seg>> {
    let mut best: Option<(usize, Vec<Seg>)> = None;
    let mut stack: Vec<(Vec<Seg>, &Value)> = vec![(Vec::new(), v)];
    while let Some((path, node)) = stack.pop() {
        if is_crushable_array(node) {
            let size = node.to_string().len();
            if best.as_ref().map_or(true, |(b, _)| size > *b) {
                best = Some((size, path.clone()));
            }
        }
        match node {
            Value::Object(map) => for (k, child) in map {
                let mut p = path.clone();
                p.push(Seg::Key(k.clone()));
                stack.push((p, child));
            },
            Value::Array(arr) => for (i, child) in arr.iter().enumerate() {
                let mut p = path.clone();
                p.push(Seg::Idx(i));
                stack.push((p, child));
            },
            _ => {}
        }
    }
    best.map(|(_, p)| p)
}

/// Columnar-encode an array of objects (no marker). Returns the body: a `var_keys` line, a
/// `constants` line, then one line per element (bare value-array for modal rows, full object for
/// anomalies). Returns `None` if the array isn't crushable.
fn encode_array(arr: &[Value]) -> Option<String> {
    if !(arr.len() >= 3 && arr.iter().all(Value::is_object)) {
        return None;
    }
    // Modal key-signature: the most common ordered key vector.
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
        let m = obj.as_object().unwrap();
        m.len() == modal.len() && m.keys().zip(&modal).all(|(k, mk)| k == mk)
    };

    // Constant columns: modal keys whose value is identical across every modal row.
    let modal_rows: Vec<&Map<String, Value>> =
        arr.iter().filter(|o| is_modal(o)).map(|o| o.as_object().unwrap()).collect();
    let mut constants = Map::new();
    if modal_rows.len() >= 2 {
        for k in &modal {
            let first = &modal_rows[0][k];
            if modal_rows.iter().all(|r| &r[k] == first) {
                constants.insert(k.clone(), first.clone());
            }
        }
    }
    let var_keys: Vec<&String> = modal.iter().filter(|k| !constants.contains_key(*k)).collect();

    let mut body = serde_json::to_string(&var_keys).ok()?;
    body.push('\n');
    body.push_str(&serde_json::to_string(&Value::Object(constants)).ok()?);
    for obj in arr {
        body.push('\n');
        if is_modal(obj) {
            let m = obj.as_object().unwrap();
            let vals: Vec<&Value> = var_keys.iter().map(|k| &m[*k]).collect();
            body.push_str(&serde_json::to_string(&vals).ok()?);
        } else {
            body.push_str(&serde_json::to_string(obj).ok()?);
        }
    }
    Some(body)
}

/// Reverse [`encode_array`] from its body lines.
fn decode_array(lines: &[&str]) -> Option<Value> {
    let mut it = lines.iter();
    let var_keys: Vec<String> = serde_json::from_str(it.next()?).ok()?;
    let constants: Map<String, Value> = serde_json::from_str::<Value>(it.next()?).ok()?.as_object()?.clone();
    let mut out = Vec::new();
    for line in it {
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
            Value::Object(_) => out.push(v),
            _ => return None,
        }
    }
    Some(Value::Array(out))
}

/// Columnar-encode the dominant repetitive JSON array in `text` (top-level or nested) when
/// beneficial. Returns `None` if there's no crushable array or the result isn't smaller.
pub fn crush(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;

    let out = if is_crushable_array(&value) {
        // top-level array
        let body = encode_array(value.as_array()?)?;
        format!("{MARKER1}{body}")
    } else {
        // nested: crush the largest array-of-objects, keep the wrapper skeleton.
        let path = largest_array_path(&value)?;
        if path.is_empty() {
            return None;
        }
        let arr = {
            let mut cur = &value;
            for seg in &path {
                cur = match seg { Seg::Key(k) => cur.get(k)?, Seg::Idx(i) => cur.get(i)? };
            }
            cur.as_array()?.clone()
        };
        let body = encode_array(&arr)?;
        let mut skeleton = value.clone();
        *get_mut(&mut skeleton, &path)? = Value::Null;
        format!("{MARKER2}{}\n{}\n{}",
            serde_json::to_string(&path_to_json(&path)).ok()?,
            serde_json::to_string(&skeleton).ok()?,
            body)
    };

    if out.len() < text.len() { Some(out) } else { None }
}

/// Reverse [`crush`], returning the reconstructed JSON value (value-lossless).
pub fn expand(crushed: &str) -> Option<Value> {
    if let Some(rest) = crushed.strip_prefix(MARKER2) {
        let mut lines = rest.split('\n');
        let path = path_from_json(&serde_json::from_str(lines.next()?).ok()?)?;
        let mut skeleton: Value = serde_json::from_str(lines.next()?).ok()?;
        let body: Vec<&str> = lines.collect();
        let arr = decode_array(&body)?;
        *get_mut(&mut skeleton, &path)? = arr;
        Some(skeleton)
    } else if let Some(rest) = crushed.strip_prefix(MARKER1) {
        let body: Vec<&str> = rest.split('\n').collect();
        decode_array(&body)
    } else {
        None
    }
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

    fn pretty(v: Value) -> String {
        serde_json::to_string_pretty(&v).unwrap()
    }

    #[test]
    fn crushes_top_level_uniform_array_and_round_trips() {
        let text = pretty(serde_json::json!([
            {"id":0,"name":"item_0","value":0.0,"status":"active"},
            {"id":1,"name":"item_1","value":1.5,"status":"active"},
            {"id":2,"name":"item_2","value":3.0,"status":"active"},
            {"id":3,"name":"item_3","value":4.5,"status":"active"}
        ]));
        let crushed = crush(&text).expect("should crush");
        assert!(crushed.starts_with(MARKER1));
        assert!(crushed.len() < text.len());
        assert!(round_trips(&text, &crushed));
    }

    #[test]
    fn crushes_nested_array_in_wrapper() {
        // the common API shape: a wrapper object whose `data.results` holds the big array.
        let items: Vec<Value> = (0..20).map(|i| serde_json::json!(
            {"id":i,"name":format!("u{i}"),"role":"user","region":"us-east-1"})).collect();
        let text = pretty(serde_json::json!({
            "status":"success","page":1,"data":{"results":items,"count":20}
        }));
        let crushed = crush(&text).expect("nested array should crush");
        assert!(crushed.starts_with(MARKER2), "uses nested marker: {}", &crushed[..20]);
        assert!(crushed.len() < text.len(), "smaller");
        assert!(round_trips(&text, &crushed), "value-lossless nested round trip");
        // wrapper fields preserved
        assert!(crushed.contains("success") && crushed.contains("\"page\""));
    }

    #[test]
    fn factors_constant_column() {
        let text = serde_json::to_string(&serde_json::json!([
            {"id":0,"status":"healthy"},{"id":1,"status":"healthy"},{"id":2,"status":"healthy"}
        ])).unwrap();
        let crushed = crush(&text).expect("crush");
        assert_eq!(crushed.matches("status").count(), 1, "constant emitted once: {crushed}");
        assert!(round_trips(&text, &crushed));
    }

    #[test]
    fn preserves_needle_rows_and_all_fields() {
        let text = pretty(serde_json::json!([
            {"id":0,"name":"item_0","value":0.0,"status":"active"},
            {"id":1,"name":"item_1","value":1.5,"status":"active"},
            {"id":2,"name":"item_2","value":3.0,"status":"active"},
            {"id":3,"name":"item_3","value":999.99,"status":"error","error_code":"ERR-2007","task_id":"task-0007"}
        ]));
        let crushed = crush(&text).expect("crush");
        assert!(crushed.contains("ERR-2007") && crushed.contains("999.99"));
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
