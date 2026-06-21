//! Opt-in, behavior-preserving JSON-Schema annotation slimming for tool/function definitions.
//!
//! This is the one place Cull does a **lossy** transform, so it is deliberately separate from the
//! lossless `compress` pipeline and exposed only via an explicit entry point (`cull slim-schema`).
//! It drops keys that are pure JSON-Schema *metadata* — `$schema`, `$id`, `$comment`, `title`,
//! `examples` — which have no effect on how a model selects or calls a tool. It NEVER touches
//! anything behavioral: property names (keys under `properties`), `type`, `required`, `enum`,
//! `items`, or `description` (which models read). Annotation keys are dropped only in schema
//! position, never when they appear as a property *name* under `properties`.

use serde_json::{Map, Value};

/// Schema-annotation keys with no effect on tool invocation. (`description` is intentionally NOT
/// here — models use it; `default`/`$ref` are behavioral and also excluded.)
const ANNOTATIONS: &[&str] = &["$schema", "$id", "$comment", "title", "examples"];

fn walk(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                if k == "properties" {
                    // keys here are property NAMES — preserve every one; recurse into sub-schemas.
                    if let Value::Object(props) = v {
                        let mut new_props = Map::new();
                        for (name, sub) in props {
                            new_props.insert(name.clone(), walk(sub));
                        }
                        out.insert(k.clone(), Value::Object(new_props));
                        continue;
                    }
                }
                if ANNOTATIONS.contains(&k.as_str()) {
                    continue; // drop pure metadata in schema position
                }
                out.insert(k.clone(), walk(v));
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(walk).collect()),
        other => other.clone(),
    }
}

/// Strip JSON-Schema metadata annotations from `text` (compact output). Returns `None` if `text`
/// is not JSON or the result would not be smaller. Behavior-preserving but lossy (annotations are
/// not recoverable) — hence the explicit opt-in entry point.
pub fn slim(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    let slimmed = walk(&value);
    let out = serde_json::to_string(&slimmed).ok()?;
    if out.len() < text.len() {
        Some(out)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_schema_annotations_keeps_property_names_and_semantics() {
        let text = serde_json::to_string(&serde_json::json!({
            "tools": [{
                "type": "function",
                "name": "update_field",
                "description": "Update a field in a record.",
                "parameters": {
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "title": "UpdateFieldParameters",
                    "type": "object",
                    "properties": {
                        "field_name": {"type": "string"},
                        "title": {"type": "string"},      // property NAMED "title" must survive
                        "readOnly": {"type": "boolean"}
                    },
                    "required": ["field_name", "title", "readOnly"]
                }
            }]
        }))
        .unwrap();
        let out = slim(&text).expect("should slim");
        assert!(out.len() < text.len(), "smaller");
        // schema-level annotations gone
        assert!(!out.contains("$schema"), "$schema dropped: {out}");
        assert!(
            !out.contains("UpdateFieldParameters"),
            "schema title dropped"
        );
        // property names + behavior preserved
        for keep in [
            "field_name",
            "\"title\"",
            "readOnly",
            "required",
            "Update a field",
        ] {
            assert!(out.contains(keep), "must preserve {keep}: {out}");
        }
        // required array still references surviving property names
        let v: Value = serde_json::from_str(&out).unwrap();
        let req = &v["tools"][0]["parameters"]["required"];
        assert_eq!(req, &serde_json::json!(["field_name", "title", "readOnly"]));
    }

    #[test]
    fn non_json_is_none() {
        assert!(slim("not json at all").is_none());
    }
}
