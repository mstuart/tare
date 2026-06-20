pub mod server;
pub mod monitor;
pub mod count;

use std::collections::HashMap;
use serde_json::Value;
use cull_core::segmenter::{segment, RawBlock};
use cull_core::segment::{Role, SegmentKind};
use cull_core::planner::Planner;
use cull_core::passes::{structural_passes, query_passes, RelevancePass, ReasoningTracePass};
use cull_core::emit::emit;
pub use cull_core::emit::FidelityReport;
use cull_core::session::SessionState;
use cull_core::task::TaskSignal;
use cull_core::plan::SegmentAction;
use cull_tokenize::ApproxCounter;

/// Proxy compression options.
pub struct CompressOpts {
    pub enabled: bool,
    pub recency_keep: usize,
    pub min_savings: u32, // skip compression unless it saves at least this many tokens
}
impl Default for CompressOpts {
    fn default() -> Self { Self { enabled: true, recency_keep: 4, min_savings: 0 } }
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

/// Compress an Anthropic Messages request: run the full pass set (supersession + IVM/delta +
/// dedup + relevance) over `tool_result` string contents, keyed by real tool name + file path
/// extracted from the matching `tool_use` block via `tool_use_id`. Delegates to the reported
/// variant and discards the report. Preserves all structure (blocks, pairing, system, tools,
/// model, order). `tool_result`s with non-string content are left untouched (v1).
pub fn compress_anthropic_request(req: &Value, opts: &CompressOpts) -> Value {
    compress_anthropic_request_reported(req, opts).0
}

/// Core variant returning `(compressed_request, Option<FidelityReport>)`. The report is `None`
/// when compression is disabled or the request has no `tool_result` string content.
pub fn compress_anthropic_request_reported(req: &Value, opts: &CompressOpts) -> (Value, Option<FidelityReport>) {
    if !opts.enabled { return (req.clone(), None); }
    let mut out = req.clone();

    // pass 1: build tool_use_id -> (tool_name, optional_path) from all tool_use blocks
    let mut meta: HashMap<String, (String, Option<String>)> = HashMap::new();
    if let Some(msgs) = out.get("messages").and_then(Value::as_array) {
        for m in msgs {
            let Some(blocks) = m.get("content").and_then(Value::as_array) else { continue; };
            for b in blocks {
                if b.get("type").and_then(Value::as_str) == Some("tool_use") {
                    if let Some(id) = b.get("id").and_then(Value::as_str) {
                        let name = b.get("name").and_then(Value::as_str).unwrap_or("tool").to_string();
                        let path = b.get("input").and_then(|i| {
                            i.get("path").or_else(|| i.get("file")).or_else(|| i.get("file_path"))
                        }).and_then(Value::as_str).map(String::from);
                        meta.insert(id.to_string(), (name, path));
                    }
                }
            }
        }
    }

    // cached-prefix boundary: last (mi, bi) carrying cache_control (block-level or message-level)
    let mut boundary: Option<(usize, usize)> = None;
    if let Some(msgs) = out.get("messages").and_then(Value::as_array) {
        for (mi, m) in msgs.iter().enumerate() {
            if m.get("cache_control").is_some() { boundary = Some((mi, usize::MAX)); }
            if let Some(blocks) = m.get("content").and_then(Value::as_array) {
                for (bi, b) in blocks.iter().enumerate() {
                    if b.get("cache_control").is_some() { boundary = Some((mi, bi)); }
                }
            }
        }
    }
    let in_cached_prefix = |mi: usize, bi: usize| -> bool {
        match boundary { Some((bm, bb)) => (mi, bi) <= (bm, bb), None => false }
    };

    // pass 2: collect tool_result contents (string or array-of-text) with rich kind + path, and their locations
    let mut raws: Vec<RawBlock> = Vec::new();
    let mut locs: Vec<(usize, usize, Option<usize>)> = Vec::new();
    if let Some(msgs) = out.get("messages").and_then(Value::as_array) {
        for (mi, m) in msgs.iter().enumerate() {
            let Some(blocks) = m.get("content").and_then(Value::as_array) else { continue; };
            for (bi, b) in blocks.iter().enumerate() {
                if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                    if in_cached_prefix(mi, bi) { continue; }
                    let (class, path) = b.get("tool_use_id").and_then(Value::as_str)
                        .and_then(|id| meta.get(id)).cloned()
                        .unwrap_or_else(|| ("tool_result".to_string(), None));
                    let kind = if path.is_some() { SegmentKind::FileRead }
                               else { SegmentKind::ToolOutput { class: class.clone() } };
                    match b.get("content") {
                        Some(Value::String(text)) => {
                            raws.push(RawBlock { role: Role::Tool, kind, text: text.clone(), path });
                            locs.push((mi, bi, None));
                        }
                        Some(Value::Array(inner)) => {
                            for (k, ib) in inner.iter().enumerate() {
                                if ib.get("type").and_then(Value::as_str) == Some("text") {
                                    if let Some(text) = ib.get("text").and_then(Value::as_str) {
                                        raws.push(RawBlock { role: Role::Tool, kind: kind.clone(),
                                            text: text.to_string(), path: path.clone() });
                                        locs.push((mi, bi, Some(k)));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    if raws.is_empty() { return (out, None); }

    let counter = ApproxCounter::o200k();
    let segs = segment(&raws, &counter);
    let mut passes = structural_passes(); // supersession + IVM + dedup
    passes.extend(query_passes());        // relevance
    let task = TaskSignal::from_text(&last_user_text(req));
    let plan = Planner::new(passes).plan_with_task(&segs, &SessionState::default(), &task);
    let (_emitted, report) = emit(&segs, &plan);

    let savings = report.input_tokens.saturating_sub(report.net_tokens);
    if savings < opts.min_savings {
        return (req.clone(), None); // not worth compressing -> exact passthrough
    }

    // write-back: apply Drop and Replace actions in place (panic-safe via get_mut chain)
    debug_assert_eq!(plan.entries.len(), locs.len(), "one plan entry per collected tool_result");
    for (entry, (mi, bi, inner)) in plan.entries.iter().zip(locs.iter()) {
        let replacement = match &entry.action {
            SegmentAction::Drop(reason) =>
                Some(format!("[cull: tool output elided — {reason:?}]")),
            SegmentAction::Replace { rendered, .. } =>
                Some(format!("[cull: delta vs an earlier read]\n{}", String::from_utf8_lossy(rendered))),
            SegmentAction::Keep => None,
        };
        let Some(text) = replacement else { continue; };
        let base = out.get_mut("messages").and_then(Value::as_array_mut)
            .and_then(|ms| ms.get_mut(*mi))
            .and_then(|m| m.get_mut("content")).and_then(Value::as_array_mut)
            .and_then(|bs| bs.get_mut(*bi));
        let Some(block) = base else { continue; };
        match inner {
            None => {
                if let Some(c) = block.get_mut("content") { *c = Value::String(text); }
            }
            Some(k) => {
                if let Some(t) = block.get_mut("content").and_then(Value::as_array_mut)
                    .and_then(|arr| arr.get_mut(*k)).and_then(|tb| tb.get_mut("text"))
                { *t = Value::String(text); }
            }
        }
    }
    (out, Some(report))
}

/// Compress an OpenAI Chat Completions request: run the full pass set over `role:"tool"` message
/// `content` strings, keyed by tool name + optional file path extracted from the matching
/// assistant `tool_calls[].function.{name,arguments}` via `tool_call_id`. Delegates to the
/// reported variant and discards the report. Preserves all structure (roles, order, `tool_calls`,
/// `system`, `tools`, model). Only `role:"tool"` content strings change.
pub fn compress_openai_request(req: &Value, opts: &CompressOpts) -> Value {
    compress_openai_request_reported(req, opts).0
}

/// Core variant returning `(compressed_request, Option<FidelityReport>)`. The report is `None`
/// when compression is disabled or the request has no `role:"tool"` string content.
pub fn compress_openai_request_reported(req: &Value, opts: &CompressOpts) -> (Value, Option<FidelityReport>) {
    if !opts.enabled { return (req.clone(), None); }
    let mut out = req.clone();

    // pass 1: build tool_call_id -> (name, optional_path) from all assistant tool_calls
    let mut meta: HashMap<String, (String, Option<String>)> = HashMap::new();
    if let Some(msgs) = out.get("messages").and_then(Value::as_array) {
        for m in msgs {
            let Some(calls) = m.get("tool_calls").and_then(Value::as_array) else { continue; };
            for c in calls {
                let Some(id) = c.get("id").and_then(Value::as_str) else { continue; };
                let f = c.get("function");
                let name = f.and_then(|f| f.get("name")).and_then(Value::as_str).unwrap_or("tool").to_string();
                let path = f.and_then(|f| f.get("arguments")).and_then(Value::as_str)
                    .and_then(|a| serde_json::from_str::<Value>(a).ok())
                    .and_then(|args| args.get("path").or_else(|| args.get("file")).or_else(|| args.get("file_path"))
                        .and_then(Value::as_str).map(String::from));
                meta.insert(id.to_string(), (name, path));
            }
        }
    }

    // pass 2: collect role:"tool" message contents with rich kind + path, and their locations
    let mut raws: Vec<RawBlock> = Vec::new();
    let mut locs: Vec<usize> = Vec::new();
    let mut task = String::new();
    if let Some(msgs) = out.get("messages").and_then(Value::as_array) {
        for (mi, m) in msgs.iter().enumerate() {
            match m.get("role").and_then(Value::as_str) {
                Some("tool") => {
                    if let Some(text) = m.get("content").and_then(Value::as_str) {
                        let (class, path) = m.get("tool_call_id").and_then(Value::as_str)
                            .and_then(|id| meta.get(id)).cloned()
                            .unwrap_or_else(|| ("tool".to_string(), None));
                        let kind = if path.is_some() { SegmentKind::FileRead }
                                   else { SegmentKind::ToolOutput { class } };
                        raws.push(RawBlock { role: Role::Tool, kind, text: text.to_string(), path });
                        locs.push(mi);
                    }
                }
                Some("user") => { if let Some(t) = m.get("content").and_then(Value::as_str) { task = t.to_string(); } }
                _ => {}
            }
        }
    }
    if raws.is_empty() { return (out, None); }

    let counter = ApproxCounter::o200k();
    let segs = segment(&raws, &counter);
    let mut passes = structural_passes(); // supersession + IVM + dedup
    passes.push(Box::new(RelevancePass { recency_keep: opts.recency_keep })); // relevance
    passes.push(Box::new(ReasoningTracePass::default()));
    let plan = Planner::new(passes).plan_with_task(&segs, &SessionState::default(), &TaskSignal::from_text(&task));
    let (_e, report) = emit(&segs, &plan);

    let savings = report.input_tokens.saturating_sub(report.net_tokens);
    if savings < opts.min_savings { return (req.clone(), None); }

    // write-back: apply Drop and Replace actions in place (panic-safe via get_mut chain)
    debug_assert_eq!(plan.entries.len(), locs.len(), "one plan entry per collected tool message");
    for (entry, mi) in plan.entries.iter().zip(locs.iter()) {
        let replacement = match &entry.action {
            SegmentAction::Drop(reason) => Some(format!("[cull: tool output elided — {reason:?}]")),
            SegmentAction::Replace { rendered, .. } =>
                Some(format!("[cull: delta vs an earlier read]\n{}", String::from_utf8_lossy(rendered))),
            SegmentAction::Keep => None,
        };
        if let Some(text) = replacement {
            if let Some(c) = out.get_mut("messages").and_then(Value::as_array_mut)
                .and_then(|ms| ms.get_mut(*mi)).and_then(|m| m.get_mut("content"))
            { *c = Value::String(text); }
        }
    }
    (out, Some(report))
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
        let out = compress_anthropic_request(&req, &CompressOpts { enabled: true, recency_keep: 1, min_savings: 0 });

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
        let out = compress_anthropic_request(&req, &CompressOpts { enabled: false, recency_keep: 4, min_savings: 0 });
        assert_eq!(out, req);
    }

    #[test]
    fn no_tool_results_is_passthrough() {
        let req = json!({"model":"x","messages":[{"role":"user","content":"hello"}]});
        let out = compress_anthropic_request(&req, &CompressOpts::default());
        assert_eq!(out, req);
    }

    #[test]
    fn supersession_drops_older_same_tool_output() {
        // two results from the SAME tool (grep) — older one superseded; both irrelevant-safe via class
        let req = serde_json::json!({"messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"g1","name":"grep","input":{"q":"x"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1","content":"OLD grep run alpha beta gamma delta"}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"g2","name":"grep","input":{"q":"x"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g2","content":"NEW grep run alpha beta gamma delta"}]},
            {"role":"user","content":"continue with alpha beta gamma"}
        ]});
        let out = compress_anthropic_request(&req, &CompressOpts { enabled: true, recency_keep: 0, min_savings: 0 });
        let older = out["messages"][1]["content"][0]["content"].as_str().unwrap();
        let newer = out["messages"][3]["content"][0]["content"].as_str().unwrap();
        assert!(older.contains("[cull"), "older same-tool output superseded: {older}");
        assert!(newer.contains("NEW grep run"), "newest kept: {newer}");
    }

    #[test]
    fn ivm_deltas_a_changed_file_reread() {
        // Files must be long enough that the unified diff is smaller in tokens than the full content.
        // A one-line change in a 20-line file produces a patch (header + ~8 context lines + 2 diff
        // lines) that is clearly smaller than the full 20-line re-read.
        let base = "fn a() { let x = 1; }\nfn b() { let x = 2; }\nfn c() { let x = 3; }\n\
                    fn d() { let x = 4; }\nfn e() { let x = 5; }\nfn f() { let x = 6; }\n\
                    fn g() { let x = 7; }\nfn h() { let x = 8; }\nfn i() { let x = 9; }\n\
                    fn j() { let x = 10; }\nfn k() { let x = 11; }\nfn l() { let x = 12; }\n\
                    fn m() { let x = 13; }\nfn n() { let x = 14; }\nfn o() { let x = 15; }\n\
                    fn p() { let x = 16; }\nfn q() { let x = 17; }\nfn r() { let x = 18; }\n\
                    fn s() { let x = 19; }\nfn t() { let x = 20; }";
        let changed = "fn a() { let x = 1; }\nfn b() { let x = 2; }\nfn c() { let x = 3; }\n\
                    fn d() { let x = 4; }\nfn e() { let x = 5; }\nfn f() { let x = 6; }\n\
                    fn g() { let x = 7; }\nfn h() { let x = 8; }\nfn i() { let x = 9; }\n\
                    fn CHANGED() { let x = 99; }\nfn k() { let x = 11; }\nfn l() { let x = 12; }\n\
                    fn m() { let x = 13; }\nfn n() { let x = 14; }\nfn o() { let x = 15; }\n\
                    fn p() { let x = 16; }\nfn q() { let x = 17; }\nfn r() { let x = 18; }\n\
                    fn s() { let x = 19; }\nfn t() { let x = 20; }";
        let req = serde_json::json!({"messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"r1","name":"read","input":{"path":"src/x.rs"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"r1","content": base}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"r2","name":"read","input":{"path":"src/x.rs"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"r2","content": changed}]},
            {"role":"user","content":"keep working on src/x.rs CHANGED"}
        ]});
        let out = compress_anthropic_request(&req, &CompressOpts { enabled: true, recency_keep: 0, min_savings: 0 });
        // first read kept verbatim; second becomes a delta marker (smaller)
        assert_eq!(out["messages"][1]["content"][0]["content"].as_str().unwrap(), base);
        let second = out["messages"][3]["content"][0]["content"].as_str().unwrap();
        assert!(second.contains("[cull: delta"), "re-read became a delta: {second}");
    }

    #[test]
    fn reported_returns_a_fidelity_report() {
        let req = sample_req();
        let (_out, report) = compress_anthropic_request_reported(&req, &CompressOpts { enabled: true, recency_keep: 1, min_savings: 0 });
        assert!(report.is_some());
        assert!(report.unwrap().input_tokens > 0);
    }

    #[test]
    fn compresses_text_inside_array_tool_result() {
        let req = serde_json::json!({"messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"g1","name":"grep","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1","content":[
                {"type":"text","text":"kubernetes helm chart registry totally unrelated junk one two"}
            ]}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1","content":[
                {"type":"text","text":"jwt authentication middleware verify token"}
            ]}]},
            {"role":"user","content":"fix authentication jwt"}
        ]});
        let out = compress_anthropic_request(&req, &CompressOpts { enabled: true, recency_keep: 0, min_savings: 0 });
        // structure preserved: array content still an array with one block of type text
        assert_eq!(out["messages"][1]["content"][0]["content"][0]["type"], "text");
        let elided = out["messages"][1]["content"][0]["content"][0]["text"].as_str().unwrap();
        let kept   = out["messages"][2]["content"][0]["content"][0]["text"].as_str().unwrap();
        assert!(elided.contains("[cull"), "irrelevant array-content text elided: {elided}");
        assert!(kept.contains("jwt authentication middleware"), "relevant kept: {kept}");
    }

    #[test]
    fn openai_compresses_tool_message_keeps_structure() {
        let req = serde_json::json!({
            "model":"gpt-x",
            "messages":[
                {"role":"assistant","content":null,"tool_calls":[
                    {"id":"c1","type":"function","function":{"name":"grep","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"c1","content":"kubernetes helm registry unrelated junk alpha"},
                {"role":"assistant","content":null,"tool_calls":[
                    {"id":"c2","type":"function","function":{"name":"read","arguments":"{\"path\":\"jwt.rs\"}"}}]},
                {"role":"tool","tool_call_id":"c2","content":"jwt authentication middleware verify"},
                {"role":"user","content":"fix authentication jwt"}
            ]
        });
        let out = compress_openai_request(&req, &CompressOpts { enabled: true, recency_keep: 0, min_savings: 0 });
        // structure: same message count, roles, tool_calls untouched
        assert_eq!(out["messages"].as_array().unwrap().len(), 5);
        assert_eq!(out["messages"][0]["tool_calls"], req["messages"][0]["tool_calls"]);
        assert_eq!(out["model"], req["model"]);
        // irrelevant tool message elided; relevant kept
        assert!(out["messages"][1]["content"].as_str().unwrap().contains("[cull"));
        assert!(out["messages"][3]["content"].as_str().unwrap().contains("jwt authentication middleware"));
        // tool_call_id preserved
        assert_eq!(out["messages"][1]["tool_call_id"], "c1");
    }

    #[test]
    fn openai_disabled_is_passthrough() {
        let req = serde_json::json!({"model":"x","messages":[{"role":"user","content":"hi"}]});
        assert_eq!(compress_openai_request(&req, &CompressOpts { enabled: false, recency_keep: 4, min_savings: 0 }), req);
    }

    #[test]
    fn does_not_compress_tool_results_inside_cached_prefix() {
        // breakpoint on the FIRST tool_result block -> it (and everything before) is cached and untouchable;
        // several post-breakpoint tool_results follow (enough to age below the recency window),
        // making the oldest post-breakpoint irrelevant block eligible for relevance pruning.
        let req = serde_json::json!({"messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"g1","name":"grep","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1",
                "content":"kafka registry unrelated junk in the CACHED prefix one two three",
                "cache_control":{"type":"ephemeral"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1",
                "content":"also unrelated kafka content AFTER the breakpoint zero"}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1",
                "content":"also unrelated kafka content AFTER the breakpoint one"}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1",
                "content":"also unrelated kafka content AFTER the breakpoint two"}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1",
                "content":"also unrelated kafka content AFTER the breakpoint three"}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1",
                "content":"also unrelated kafka content AFTER the breakpoint four"}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1",
                "content":"also unrelated kafka content AFTER the breakpoint five"}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1",
                "content":"also unrelated kafka content AFTER the breakpoint six"}]},
            {"role":"user","content":"do something about authentication jwt"}
        ]});
        let out = compress_anthropic_request(&req, &CompressOpts { enabled: true, recency_keep: 0, min_savings: 0 });
        // the cached (pre-breakpoint) tool_result is byte-identical
        assert_eq!(out["messages"][1]["content"][0]["content"], req["messages"][1]["content"][0]["content"]);
        // the oldest post-breakpoint irrelevant block (aged out of recency window) is elided
        let after = out["messages"][2]["content"][0]["content"].as_str().unwrap();
        assert!(after.contains("[cull"), "post-breakpoint irrelevant output compressed: {after}");
    }

    #[test]
    fn skips_compression_when_savings_below_threshold() {
        // one tiny tool_result — stubbing it would save ~nothing; with a high min_savings, passthrough.
        let req = serde_json::json!({"messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"g1","name":"grep","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1","content":"x"}]},
            {"role":"user","content":"do something unrelated entirely"}
        ]});
        let out = compress_anthropic_request(&req, &CompressOpts { enabled: true, recency_keep: 0, min_savings: 1000 });
        assert_eq!(out, req, "below the savings threshold -> exact passthrough");
    }
}
