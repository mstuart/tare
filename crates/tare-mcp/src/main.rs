//! tare-mcp — a Model Context Protocol (stdio) server exposing Tare's compression as tools, plus a
//! reversible `tare_expand` that returns the original content of anything Tare compacted this session
//! (CCR-style). Transport is newline-delimited JSON-RPC 2.0 over stdin/stdout.
//!
//! Tools: `tare_compress` (lossless pipeline), `tare_skeletonize` (drop code bodies),
//! `tare_compact_lossy` (row-cap/field-truncate tabular & JSON), `tare_expand` (retrieve an
//! original by id), `tare_stats` (session savings).

use serde_json::{json, Value};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, Write};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Session state: originals retrievable via `tare_expand`, plus cumulative approximate savings.
#[derive(Default)]
struct State {
    originals: HashMap<String, String>,
    saved_in: u64,
    saved_out: u64,
}

/// Short content-addressed id for the original-store (within-session only; collisions just overwrite).
fn content_id(s: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Approximate token count (chars/4), matching tare's offline counter.
fn approx(s: &str) -> u64 {
    s.chars().count().div_ceil(4) as u64
}

fn tool_specs() -> Value {
    json!([
        {
            "name": "tare_skeletonize",
            "description": "Skeletonize a source file: keep signatures, types, and imports; drop function/method bodies. Reversible via tare_expand. Languages: rust, python, js, ts, go.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "file path (used for language detection)"},
                    "content": {"type": "string", "description": "source code"}
                },
                "required": ["path", "content"]
            }
        },
        {
            "name": "tare_compact_lossy",
            "description": "Aggressively compact a large JSON array, tabular, or log text — keeps head/tail rows, anomalies, alert lines, and query-relevant rows. Reversible via tare_expand.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": {"type": "string"},
                    "max_rows": {"type": "integer", "description": "cap kept lines (0 = uncapped)"},
                    "max_field": {"type": "integer", "description": "truncate each kept line to N chars (0 = off)"},
                    "boundary": {"type": "integer", "description": "head/tail rows always kept (default 3)"},
                    "task": {"type": "string", "description": "keep units relevant to this query"}
                },
                "required": ["content"]
            }
        },
        {
            "name": "tare_compress",
            "description": "Run Tare's lossless compression pipeline over a JSON array of context blocks; returns the compressed context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": {"type": "string", "description": "JSON array of context blocks"},
                    "task": {"type": "string"}
                },
                "required": ["context"]
            }
        },
        {
            "name": "tare_expand",
            "description": "Retrieve the full original content of anything Tare compacted this session, by the id shown in its [tare: ...] marker.",
            "inputSchema": {
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"]
            }
        },
        {
            "name": "tare_stats",
            "description": "Cumulative approximate token savings for this MCP session.",
            "inputSchema": {"type": "object", "properties": {}}
        }
    ])
}

/// Execute one tool call. Returns the text content, or an error message (surfaced as isError).
fn call_tool(state: &mut State, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "tare_skeletonize" => {
            let path = args
                .get("path")
                .and_then(Value::as_str)
                .ok_or("missing 'path'")?;
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .ok_or("missing 'content'")?;
            match tare_core::code_skeleton::skeletonize(content, path) {
                Some(sk) => {
                    let id = content_id(content);
                    state.saved_in += approx(content);
                    state.saved_out += approx(&sk);
                    state.originals.insert(id.clone(), content.to_string());
                    Ok(format!(
                        "{sk}\n[tare: full original available via tare_expand id={id}]"
                    ))
                }
                None => Ok(content.to_string()), // unknown language or nothing to elide
            }
        }
        "tare_compact_lossy" => {
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .ok_or("missing 'content'")?;
            let boundary = args.get("boundary").and_then(Value::as_u64).unwrap_or(3) as usize;
            let max_field = args.get("max_field").and_then(Value::as_u64).unwrap_or(0) as usize;
            let max_rows = args.get("max_rows").and_then(Value::as_u64).unwrap_or(0) as usize;
            let task = args.get("task").and_then(Value::as_str);
            match tare_core::lossy_compact::compact_opts(
                content, boundary, task, max_field, max_rows,
            ) {
                Some(c) => {
                    let id = content_id(content);
                    state.saved_in += approx(content);
                    state.saved_out += approx(&c);
                    state.originals.insert(id.clone(), content.to_string());
                    Ok(format!(
                        "{c}\n[tare: full original available via tare_expand id={id}]"
                    ))
                }
                None => Ok(content.to_string()),
            }
        }
        "tare_compress" => {
            let ctx = args.get("context").ok_or("missing 'context'")?;
            let ctx_str = match ctx {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let task = args.get("task").and_then(Value::as_str).unwrap_or("");
            let out = tare_cli::run_compress(&ctx_str, task)?;
            state.saved_in += out.report.input_tokens as u64;
            state.saved_out += out.report.net_tokens as u64;
            Ok(out.compressed)
        }
        "tare_expand" => {
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .ok_or("missing 'id'")?;
            state.originals.get(id).cloned().ok_or_else(|| {
                format!("unknown id '{id}' (nothing compacted under it this session)")
            })
        }
        "tare_stats" => {
            let saved = state.saved_in.saturating_sub(state.saved_out);
            let pct = if state.saved_in > 0 {
                100.0 * saved as f64 / state.saved_in as f64
            } else {
                0.0
            };
            Ok(format!(
                "tare session: input≈{} tok, output≈{} tok, saved≈{} tok ({pct:.1}%), {} originals retrievable",
                state.saved_in,
                state.saved_out,
                saved,
                state.originals.len()
            ))
        }
        other => Err(format!("unknown tool '{other}'")),
    }
}

/// Handle one JSON-RPC message. Returns the response JSON string, or `None` for notifications.
fn handle(state: &mut State, msg: &Value) -> Option<String> {
    let method = msg.get("method").and_then(Value::as_str)?;
    let id = msg.get("id").cloned();
    let ok = |result: Value| {
        id.as_ref()
            .map(|i| json!({"jsonrpc":"2.0","id":i,"result":result}).to_string())
    };
    let err = |code: i64, message: &str| {
        id.as_ref().map(|i| {
            json!({"jsonrpc":"2.0","id":i,"error":{"code":code,"message":message}}).to_string()
        })
    };
    match method {
        "initialize" => ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "tare-mcp", "version": env!("CARGO_PKG_VERSION")}
        })),
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({"tools": tool_specs()})),
        "tools/call" => {
            let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_tool(state, name, &args) {
                Ok(text) => {
                    ok(json!({"content": [{"type": "text", "text": text}], "isError": false}))
                }
                Err(e) => ok(json!({"content": [{"type": "text", "text": e}], "isError": true})),
            }
        }
        _ => err(-32601, "method not found"),
    }
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut state = State::default();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Value>(line) {
            Ok(msg) => handle(&mut state, &msg),
            Err(_) => Some(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}})
                    .to_string(),
            ),
        };
        if let Some(resp) = resp {
            if writeln!(out, "{resp}").is_err() {
                break;
            }
            let _ = out.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_reports_protocol_and_server() {
        let resp = handle(
            &mut State::default(),
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(v["result"]["serverInfo"]["name"], "tare-mcp");
    }

    #[test]
    fn tools_list_has_all_five() {
        let resp = handle(
            &mut State::default(),
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        let names: Vec<&str> = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for n in [
            "tare_skeletonize",
            "tare_compact_lossy",
            "tare_compress",
            "tare_expand",
            "tare_stats",
        ] {
            assert!(names.contains(&n), "missing tool {n}");
        }
    }

    #[test]
    fn notification_yields_no_response() {
        assert!(handle(
            &mut State::default(),
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .is_none());
    }

    #[test]
    fn unknown_method_is_error() {
        let resp = handle(
            &mut State::default(),
            &json!({"jsonrpc":"2.0","id":9,"method":"bogus"}),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["error"]["code"], -32601);
    }

    #[test]
    fn skeletonize_then_expand_roundtrips() {
        let mut s = State::default();
        let code = "use std::io;\n\npub fn run(x: i32) -> i32 {\n    let a = x + 1;\n    let b = a * 2;\n    let c = b - 3;\n    c\n}\n";
        // skeletonize
        let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"tare_skeletonize","arguments":{"path":"run.rs","content":code}}});
        let resp = handle(&mut s, &call).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("pub fn run(x: i32) -> i32"), "signature kept");
        assert!(!text.contains("let b = a * 2"), "body dropped");
        // grab the expand id from the marker
        let id = text
            .rsplit("id=")
            .next()
            .unwrap()
            .trim_end_matches(']')
            .trim()
            .to_string();
        // expand → original
        let call2 = json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"tare_expand","arguments":{"id":id}}});
        let resp2 = handle(&mut s, &call2).unwrap();
        let v2: Value = serde_json::from_str(&resp2).unwrap();
        assert_eq!(
            v2["result"]["content"][0]["text"].as_str().unwrap(),
            code,
            "expand returns the exact original"
        );
        assert_eq!(v2["result"]["isError"], false);
    }

    #[test]
    fn expand_unknown_id_errors() {
        let mut s = State::default();
        let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"tare_expand","arguments":{"id":"deadbeef"}}});
        let v: Value = serde_json::from_str(&handle(&mut s, &call).unwrap()).unwrap();
        assert_eq!(v["result"]["isError"], true);
    }
}
