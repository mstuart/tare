use serde_json::Value;
use cull_core::segmenter::{segment, RawBlock};
use cull_core::segment::{Role, SegmentKind};
use cull_core::planner::{Pass, Planner};
use cull_core::passes::{ExactDedupPass, RelevancePass};
use cull_core::session::SessionState;
use cull_core::task::TaskSignal;
use cull_core::plan::SegmentAction;
use cull_tokenize::ApproxCounter;

/// Proxy compression options.
pub struct CompressOpts {
    pub enabled: bool,
    pub recency_keep: usize,
}
impl Default for CompressOpts {
    fn default() -> Self { Self { enabled: true, recency_keep: 4 } }
}

/// Concatenate the text of the LAST user message (string content or text blocks) — the task signal.
pub fn last_user_text(req: &Value) -> String {
    let Some(msgs) = req.get("messages").and_then(Value::as_array) else { return String::new(); };
    for m in msgs.iter().rev() {
        if m.get("role").and_then(Value::as_str) != Some("user") { continue; }
        return match m.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(blocks)) => blocks.iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>().join(" "),
            _ => String::new(),
        };
    }
    String::new()
}

/// Compress an Anthropic Messages request: run query-relevance + dedup over `tool_result` string
/// contents and stub dropped ones IN PLACE. Preserves all structure (blocks, pairing, system,
/// tools, model, order). `tool_result`s with non-string content are left untouched (v1).
pub fn compress_anthropic_request(req: &Value, opts: &CompressOpts) -> Value {
    if !opts.enabled { return req.clone(); }
    let mut out = req.clone();

    // collect tool_result string contents + their (message, block) locations
    let mut raws: Vec<RawBlock> = Vec::new();
    let mut locs: Vec<(usize, usize)> = Vec::new();
    if let Some(msgs) = out.get("messages").and_then(Value::as_array) {
        for (mi, m) in msgs.iter().enumerate() {
            let Some(blocks) = m.get("content").and_then(Value::as_array) else { continue; };
            for (bi, b) in blocks.iter().enumerate() {
                if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                    if let Some(text) = b.get("content").and_then(Value::as_str) {
                        raws.push(RawBlock {
                            role: Role::Tool,
                            kind: SegmentKind::ToolOutput { class: "tool_result".into() },
                            text: text.to_string(),
                            path: None,
                        });
                        locs.push((mi, bi));
                    }
                }
            }
        }
    }
    if raws.is_empty() { return out; }

    let counter = ApproxCounter::o200k();
    let segs = segment(&raws, &counter);
    let passes: Vec<Box<dyn Pass>> = vec![
        Box::new(RelevancePass { recency_keep: opts.recency_keep }),
        Box::new(ExactDedupPass),
    ];
    let task = TaskSignal::from_text(&last_user_text(req));
    let plan = Planner::new(passes).plan_with_task(&segs, &SessionState::default(), &task);

    // apply: stub the content of any dropped tool_result, in place
    debug_assert_eq!(plan.entries.len(), locs.len(), "one plan entry per collected tool_result");
    for (entry, (mi, bi)) in plan.entries.iter().zip(locs.iter()) {
        if let SegmentAction::Drop(reason) = &entry.action {
            if let Some(content) = out
                .get_mut("messages").and_then(serde_json::Value::as_array_mut)
                .and_then(|msgs| msgs.get_mut(*mi))
                .and_then(|m| m.get_mut("content")).and_then(serde_json::Value::as_array_mut)
                .and_then(|blocks| blocks.get_mut(*bi))
                .and_then(|b| b.get_mut("content"))
            {
                *content = serde_json::Value::String(format!("[cull: tool output elided — {reason:?}]"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn last_user_text_reads_string_and_block_content() {
        let req = json!({"messages":[
            {"role":"assistant","content":[{"type":"text","text":"hi"}]},
            {"role":"user","content":"fix the auth bug"}
        ]});
        assert_eq!(last_user_text(&req), "fix the auth bug");

        let req2 = json!({"messages":[
            {"role":"user","content":[{"type":"text","text":"please fix"},{"type":"text","text":"jwt"}]}
        ]});
        assert!(last_user_text(&req2).contains("jwt"));
    }

    fn sample_req() -> Value {
        json!({
            "model": "claude-x",
            "max_tokens": 1024,
            "system": "You are a coding agent.",
            "tools": [{"name":"run","description":"run","input_schema":{}}],
            "messages": [
                {"role":"user","content":"start working on authentication jwt"},
                {"role":"assistant","content":[
                    {"type":"text","text":"running tool"},
                    {"type":"tool_use","id":"t1","name":"run","input":{"cmd":"grep kafka"}}
                ]},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"t1","content":"kafka broker partitions offsets unrelated to the task at all"}
                ]},
                {"role":"assistant","content":[
                    {"type":"tool_use","id":"t2","name":"run","input":{"cmd":"cat jwt.rs"}}
                ]},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"t2","content":"jwt authentication middleware token verify"}
                ]},
                {"role":"user","content":"now fix the authentication jwt bug"}
            ]
        })
    }

    #[test]
    fn compresses_irrelevant_tool_result_keeps_structure() {
        let req = sample_req();
        let out = compress_anthropic_request(&req, &CompressOpts { enabled: true, recency_keep: 1 });

        // structure preserved: same message count, roles, block counts
        assert_eq!(out["messages"].as_array().unwrap().len(), req["messages"].as_array().unwrap().len());
        for (a, b) in out["messages"].as_array().unwrap().iter().zip(req["messages"].as_array().unwrap()) {
            assert_eq!(a["role"], b["role"]);
            if let (Some(ac), Some(bc)) = (a["content"].as_array(), b["content"].as_array()) {
                assert_eq!(ac.len(), bc.len(), "block count per message unchanged");
            }
        }
        // untouched fields
        assert_eq!(out["system"], req["system"]);
        assert_eq!(out["tools"], req["tools"]);
        assert_eq!(out["model"], req["model"]);
        assert_eq!(out["max_tokens"], req["max_tokens"]);
        // tool_use blocks byte-identical
        assert_eq!(out["messages"][1]["content"][1], req["messages"][1]["content"][1]);
        assert_eq!(out["messages"][3]["content"][0], req["messages"][3]["content"][0]);

        // the irrelevant kafka tool_result (msg 2) is stubbed; the relevant jwt one (msg 4) survives
        let kafka = out["messages"][2]["content"][0]["content"].as_str().unwrap();
        let jwt = out["messages"][4]["content"][0]["content"].as_str().unwrap();
        assert!(kafka.contains("[cull"), "irrelevant tool_result stubbed: {kafka}");
        assert!(jwt.contains("jwt authentication middleware"), "relevant tool_result preserved: {jwt}");
        // tool_use_id linkage preserved on the stubbed block
        assert_eq!(out["messages"][2]["content"][0]["tool_use_id"], "t1");
    }

    #[test]
    fn disabled_is_passthrough() {
        let req = sample_req();
        let out = compress_anthropic_request(&req, &CompressOpts { enabled: false, recency_keep: 4 });
        assert_eq!(out, req);
    }

    #[test]
    fn no_tool_results_is_passthrough() {
        let req = json!({"model":"x","messages":[{"role":"user","content":"hello"}]});
        let out = compress_anthropic_request(&req, &CompressOpts::default());
        assert_eq!(out, req);
    }
}
