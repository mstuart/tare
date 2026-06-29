//! Tare's HTTP proxy: compresses Anthropic/OpenAI requests in-flight via a closed-loop,
//! output-aware controller, then streams the upstream response back byte-for-byte.

pub mod count;
pub mod monitor;
pub mod server;

use serde_json::Value;
use std::collections::HashMap;
use tare_core::emit::emit;
pub use tare_core::emit::FidelityReport;
use tare_core::passes::{structural_passes, ReasoningTracePass, RelevancePass};
use tare_core::plan::SegmentAction;
use tare_core::planner::Planner;
use tare_core::segment::{Role, SegmentKind};
use tare_core::segmenter::{segment, RawBlock};
use tare_core::session::SessionState;
use tare_core::task::TaskSignal;
use tare_tokenize::ApproxCounter;

/// Proxy compression options.
pub struct CompressOpts {
    pub enabled: bool,
    pub recency_keep: usize,
    pub min_savings: u32, // skip compression unless it saves at least this many tokens
}
impl Default for CompressOpts {
    fn default() -> Self {
        Self {
            enabled: true,
            recency_keep: 4,
            min_savings: 0,
        }
    }
}

/// Per-turn compression aggression — the DYNAMIC dial the closed-loop controller turns each turn,
/// distinct from the static [`CompressOpts`] session config. `Default` is byte-identical to the
/// pre-controller behavior (relevance on, path-default recency, no lossy).
#[derive(Clone, Copy, Debug, Default)]
pub struct Aggression {
    /// Back off query-relevance pruning (verbosity-spike response): keep more context.
    pub skip_relevance: bool,
    /// Relevance recency-window override. `None` = each path's default (Anthropic 6 / OpenAI opts);
    /// `Some(n)` tightens it (smaller = prune more) as the context window fills.
    pub recency_keep: Option<usize>,
    /// Skeletonize KEPT source-file reads (drop function bodies, keep signatures/types/imports) —
    /// code reads are the dominant token sink; reversible by re-reading. Engaged as the window fills.
    pub skeletonize_code: bool,
    /// Opt-in lossy compaction of kept tool OUTPUTS at the top tier (0 = lossless). Maps to the
    /// `lossy_compact` row-cap / field-truncate levers; applied only when the window is near full.
    pub lossy_max_rows: usize,
    pub lossy_max_field: usize,
}

/// The closed-loop controller: map live session signals to a per-turn [`Aggression`].
/// `Aggression::default()` (all fields false/None/0, via derive) is the level-1 no-op dial — exactly
/// the pre-controller behavior: relevance on, path-default recency, no skeleton, no lossy.
///
/// - `spiking`: the prior turn's output spiked (compression-paradox cue) — step DOWN one level so the
///   model stops over-generating to compensate.
/// - `fill`: input-tokens / context-window ratio — as the window saturates, step UP (compress more)
///   to fight context rot. (The cache-floor HALT is handled upstream as full passthrough.)
///
/// Levels: 0 = back off (skip relevance) · 1 = default lossless · 2 = tighten recency ·
/// 3 = tighten recency + lossy suffix compaction. Fill sets the base level; a spike subtracts one
/// (gentler). Level 1 with no signals equals the pre-controller default exactly.
pub fn controller(spiking: bool, fill: f64) -> Aggression {
    let mut level: u8 = if fill >= 0.8 {
        3
    } else if fill >= 0.5 {
        2
    } else {
        1
    };
    if spiking {
        level = level.saturating_sub(1);
    }
    match level {
        0 => Aggression {
            skip_relevance: true,
            recency_keep: None,
            skeletonize_code: false,
            lossy_max_rows: 0,
            lossy_max_field: 0,
        },
        1 => Aggression {
            skip_relevance: false,
            recency_keep: None,
            skeletonize_code: false,
            lossy_max_rows: 0,
            lossy_max_field: 0,
        },
        2 => Aggression {
            skip_relevance: false,
            recency_keep: Some(2),
            skeletonize_code: true,
            lossy_max_rows: 0,
            lossy_max_field: 0,
        },
        _ => Aggression {
            skip_relevance: false,
            recency_keep: Some(2),
            skeletonize_code: true,
            lossy_max_rows: 40,
            lossy_max_field: 200,
        },
    }
}

/// Lossy compaction of a KEPT segment when the controller enables it: source-file reads get
/// AST-skeletonized (function bodies dropped, structure kept — reversible by re-reading); tool
/// OUTPUTS get row-cap/field-truncate compaction. Returns a replacement only when it shrinks.
fn lossy_keep(raw: &RawBlock, aggr: &Aggression, task: Option<&str>) -> Option<String> {
    match &raw.kind {
        SegmentKind::FileRead if aggr.skeletonize_code => {
            let path = raw.path.as_deref()?;
            tare_core::code_skeleton::skeletonize(&raw.text, path)
                .map(|s| format!("[tare: code skeleton — bodies elided; re-read to expand]\n{s}"))
        }
        SegmentKind::ToolOutput { .. } if aggr.lossy_max_rows > 0 || aggr.lossy_max_field > 0 => {
            tare_core::lossy_compact::compact_opts(
                &raw.text,
                3,
                task,
                aggr.lossy_max_field,
                aggr.lossy_max_rows,
            )
            .map(|c| format!("[tare: lossy-compacted]\n{c}"))
        }
        _ => None, // lossless tier
    }
}

/// Concatenate the text of the LAST user message (string content or text blocks) — the task signal.
pub fn last_user_text(req: &Value) -> String {
    let Some(msgs) = req.get("messages").and_then(Value::as_array) else {
        return String::new();
    };
    for m in msgs.iter().rev() {
        if m.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        return match m.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(blocks)) => blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" "),
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
    compress_anthropic_request_reported(req, opts, Aggression::default()).0
}

/// Core variant returning `(compressed_request, Option<FidelityReport>)`. The report is `None`
/// when compression is disabled or the request has no `tool_result` string content.
pub fn compress_anthropic_request_reported(
    req: &Value,
    opts: &CompressOpts,
    aggr: Aggression,
) -> (Value, Option<FidelityReport>) {
    if !opts.enabled {
        return (req.clone(), None);
    }
    let mut out = req.clone();

    // pass 1: build tool_use_id -> (tool_name, optional_path) from all tool_use blocks
    let mut meta: HashMap<String, (String, Option<String>)> = HashMap::new();
    if let Some(msgs) = out.get("messages").and_then(Value::as_array) {
        for m in msgs {
            let Some(blocks) = m.get("content").and_then(Value::as_array) else {
                continue;
            };
            for b in blocks {
                if b.get("type").and_then(Value::as_str) == Some("tool_use") {
                    if let Some(id) = b.get("id").and_then(Value::as_str) {
                        let name = b
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let path = b
                            .get("input")
                            .and_then(|i| {
                                i.get("path")
                                    .or_else(|| i.get("file"))
                                    .or_else(|| i.get("file_path"))
                            })
                            .and_then(Value::as_str)
                            .map(String::from);
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
            if m.get("cache_control").is_some() {
                boundary = Some((mi, usize::MAX));
            }
            if let Some(blocks) = m.get("content").and_then(Value::as_array) {
                for (bi, b) in blocks.iter().enumerate() {
                    if b.get("cache_control").is_some() {
                        boundary = Some((mi, bi));
                    }
                }
            }
        }
    }
    let in_cached_prefix = |mi: usize, bi: usize| -> bool {
        match boundary {
            Some((bm, bb)) => (mi, bi) <= (bm, bb),
            None => false,
        }
    };

    // pass 2: collect tool_result contents (string or array-of-text) with rich kind + path, and their locations
    let mut raws: Vec<RawBlock> = Vec::new();
    let mut locs: Vec<(usize, usize, Option<usize>)> = Vec::new();
    if let Some(msgs) = out.get("messages").and_then(Value::as_array) {
        for (mi, m) in msgs.iter().enumerate() {
            let Some(blocks) = m.get("content").and_then(Value::as_array) else {
                continue;
            };
            for (bi, b) in blocks.iter().enumerate() {
                if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                    if in_cached_prefix(mi, bi) {
                        continue;
                    }
                    let (class, path) = b
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .and_then(|id| meta.get(id))
                        .cloned()
                        .unwrap_or_else(|| ("tool_result".to_string(), None));
                    let kind = if path.is_some() {
                        SegmentKind::FileRead
                    } else {
                        SegmentKind::ToolOutput {
                            class: class.clone(),
                        }
                    };
                    match b.get("content") {
                        Some(Value::String(text)) => {
                            raws.push(RawBlock {
                                role: Role::Tool,
                                kind,
                                text: text.clone(),
                                path,
                            });
                            locs.push((mi, bi, None));
                        }
                        Some(Value::Array(inner)) => {
                            for (k, ib) in inner.iter().enumerate() {
                                if ib.get("type").and_then(Value::as_str) == Some("text") {
                                    if let Some(text) = ib.get("text").and_then(Value::as_str) {
                                        raws.push(RawBlock {
                                            role: Role::Tool,
                                            kind: kind.clone(),
                                            text: text.to_string(),
                                            path: path.clone(),
                                        });
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
    if raws.is_empty() {
        return (out, None);
    }

    let counter = ApproxCounter::o200k();
    let segs = segment(&raws, &counter);
    let mut passes = structural_passes(); // supersession + IVM + dedup
                                          // Query-relevance pruning — the controller backs it off on a verbosity spike and tightens its
                                          // recency window as the context fills. Default (skip=false, recency None→6) is byte-unchanged.
    if !aggr.skip_relevance {
        passes.push(Box::new(RelevancePass {
            recency_keep: aggr.recency_keep.unwrap_or(6),
        }));
    }
    passes.push(Box::new(ReasoningTracePass::default()));
    let task_text = last_user_text(req);
    let task = TaskSignal::from_text(&task_text);
    let plan = Planner::new(passes).plan_with_task(&segs, &SessionState::default(), &task);
    let (_emitted, report) = emit(&segs, &plan);

    let savings = report.input_tokens.saturating_sub(report.net_tokens);
    if savings < opts.min_savings {
        return (req.clone(), None); // not worth compressing -> exact passthrough
    }

    // write-back: apply Drop and Replace actions in place (panic-safe via get_mut chain)
    // Invariant: planner produces one entry per collected tool_result. debug_assert is a no-op
    // in --release, so a mismatch would silently zip-truncate and leave some tool_results
    // unreplaced. Check at runtime: log and skip write-back entirely to avoid silent corruption.
    if plan.entries.len() != locs.len() {
        eprintln!(
            "[tare-proxy] plan/locs length mismatch ({} vs {}); skipping write-back to avoid silent corruption",
            plan.entries.len(),
            locs.len()
        );
        return (out, None);
    }
    let lossy_task = if task_text.is_empty() {
        None
    } else {
        Some(task_text.as_str())
    };
    for (i, (entry, (mi, bi, inner))) in plan.entries.iter().zip(locs.iter()).enumerate() {
        let replacement = match &entry.action {
            SegmentAction::Drop(reason) => Some(format!("[tare: tool output elided — {reason:?}]")),
            SegmentAction::Replace { rendered, .. } => Some(format!(
                "[tare: delta vs an earlier read]\n{}",
                String::from_utf8_lossy(rendered)
            )),
            SegmentAction::Keep => lossy_keep(&raws[i], &aggr, lossy_task),
        };
        let Some(text) = replacement else {
            continue;
        };
        let base = out
            .get_mut("messages")
            .and_then(Value::as_array_mut)
            .and_then(|ms| ms.get_mut(*mi))
            .and_then(|m| m.get_mut("content"))
            .and_then(Value::as_array_mut)
            .and_then(|bs| bs.get_mut(*bi));
        let Some(block) = base else {
            continue;
        };
        match inner {
            None => {
                if let Some(c) = block.get_mut("content") {
                    *c = Value::String(text);
                }
            }
            Some(k) => {
                if let Some(t) = block
                    .get_mut("content")
                    .and_then(Value::as_array_mut)
                    .and_then(|arr| arr.get_mut(*k))
                    .and_then(|tb| tb.get_mut("text"))
                {
                    *t = Value::String(text);
                }
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
    compress_openai_request_reported(req, opts, Aggression::default()).0
}

/// Core variant returning `(compressed_request, Option<FidelityReport>)`. The report is `None`
/// when compression is disabled or the request has no `role:"tool"` string content.
pub fn compress_openai_request_reported(
    req: &Value,
    opts: &CompressOpts,
    aggr: Aggression,
) -> (Value, Option<FidelityReport>) {
    if !opts.enabled {
        return (req.clone(), None);
    }
    let mut out = req.clone();

    // pass 1: build tool_call_id -> (name, optional_path) from all assistant tool_calls
    let mut meta: HashMap<String, (String, Option<String>)> = HashMap::new();
    if let Some(msgs) = out.get("messages").and_then(Value::as_array) {
        for m in msgs {
            let Some(calls) = m.get("tool_calls").and_then(Value::as_array) else {
                continue;
            };
            for c in calls {
                let Some(id) = c.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let f = c.get("function");
                let name = f
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                let path = f
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                    .and_then(|a| serde_json::from_str::<Value>(a).ok())
                    .and_then(|args| {
                        args.get("path")
                            .or_else(|| args.get("file"))
                            .or_else(|| args.get("file_path"))
                            .and_then(Value::as_str)
                            .map(String::from)
                    });
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
                        let (class, path) = m
                            .get("tool_call_id")
                            .and_then(Value::as_str)
                            .and_then(|id| meta.get(id))
                            .cloned()
                            .unwrap_or_else(|| ("tool".to_string(), None));
                        let kind = if path.is_some() {
                            SegmentKind::FileRead
                        } else {
                            SegmentKind::ToolOutput { class }
                        };
                        raws.push(RawBlock {
                            role: Role::Tool,
                            kind,
                            text: text.to_string(),
                            path,
                        });
                        locs.push(mi);
                    }
                }
                Some("user") => {
                    if let Some(t) = m.get("content").and_then(Value::as_str) {
                        task = t.to_string();
                    }
                }
                _ => {}
            }
        }
    }
    if raws.is_empty() {
        return (out, None);
    }

    let counter = ApproxCounter::o200k();
    let segs = segment(&raws, &counter);
    let mut passes = structural_passes(); // supersession + IVM + dedup
                                          // relevance pruning — controller backs off on a verbosity spike and tightens recency as the
                                          // window fills (override falls back to the static opts.recency_keep)
    if !aggr.skip_relevance {
        passes.push(Box::new(RelevancePass {
            recency_keep: aggr.recency_keep.unwrap_or(opts.recency_keep),
        }));
    }
    passes.push(Box::new(ReasoningTracePass::default()));
    let plan = Planner::new(passes).plan_with_task(
        &segs,
        &SessionState::default(),
        &TaskSignal::from_text(&task),
    );
    let (_e, report) = emit(&segs, &plan);

    let savings = report.input_tokens.saturating_sub(report.net_tokens);
    if savings < opts.min_savings {
        return (req.clone(), None);
    }

    // write-back: apply Drop and Replace actions in place (panic-safe via get_mut chain)
    // Invariant: planner produces one entry per collected tool message. debug_assert is a no-op
    // in --release, so a mismatch would silently zip-truncate and leave some tool messages
    // unreplaced. Check at runtime: log and skip write-back entirely to avoid silent corruption.
    if plan.entries.len() != locs.len() {
        eprintln!(
            "[tare-proxy] plan/locs length mismatch ({} vs {}); skipping write-back to avoid silent corruption",
            plan.entries.len(),
            locs.len()
        );
        return (out, None);
    }
    let lossy_task = if task.is_empty() {
        None
    } else {
        Some(task.as_str())
    };
    for (i, (entry, mi)) in plan.entries.iter().zip(locs.iter()).enumerate() {
        let replacement = match &entry.action {
            SegmentAction::Drop(reason) => Some(format!("[tare: tool output elided — {reason:?}]")),
            SegmentAction::Replace { rendered, .. } => Some(format!(
                "[tare: delta vs an earlier read]\n{}",
                String::from_utf8_lossy(rendered)
            )),
            SegmentAction::Keep => lossy_keep(&raws[i], &aggr, lossy_task),
        };
        if let Some(text) = replacement {
            if let Some(c) = out
                .get_mut("messages")
                .and_then(Value::as_array_mut)
                .and_then(|ms| ms.get_mut(*mi))
                .and_then(|m| m.get_mut("content"))
            {
                *c = Value::String(text);
            }
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
        let out = compress_anthropic_request(
            &req,
            &CompressOpts {
                enabled: true,
                recency_keep: 1,
                min_savings: 0,
            },
        );

        // structure preserved: same message count, roles, block counts
        assert_eq!(
            out["messages"].as_array().unwrap().len(),
            req["messages"].as_array().unwrap().len()
        );
        for (a, b) in out["messages"]
            .as_array()
            .unwrap()
            .iter()
            .zip(req["messages"].as_array().unwrap())
        {
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
        assert_eq!(
            out["messages"][1]["content"][1],
            req["messages"][1]["content"][1]
        );
        assert_eq!(
            out["messages"][3]["content"][0],
            req["messages"][3]["content"][0]
        );

        // the irrelevant kafka tool_result (msg 2) is stubbed; the relevant jwt one (msg 4) survives
        let kafka = out["messages"][2]["content"][0]["content"]
            .as_str()
            .unwrap();
        let jwt = out["messages"][4]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            kafka.contains("[tare"),
            "irrelevant tool_result stubbed: {kafka}"
        );
        assert!(
            jwt.contains("jwt authentication middleware"),
            "relevant tool_result preserved: {jwt}"
        );
        // tool_use_id linkage preserved on the stubbed block
        assert_eq!(out["messages"][2]["content"][0]["tool_use_id"], "t1");
    }

    #[test]
    fn disabled_is_passthrough() {
        let req = sample_req();
        let out = compress_anthropic_request(
            &req,
            &CompressOpts {
                enabled: false,
                recency_keep: 4,
                min_savings: 0,
            },
        );
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
        let out = compress_anthropic_request(
            &req,
            &CompressOpts {
                enabled: true,
                recency_keep: 0,
                min_savings: 0,
            },
        );
        let older = out["messages"][1]["content"][0]["content"]
            .as_str()
            .unwrap();
        let newer = out["messages"][3]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            older.contains("[tare"),
            "older same-tool output superseded: {older}"
        );
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
        let out = compress_anthropic_request(
            &req,
            &CompressOpts {
                enabled: true,
                recency_keep: 0,
                min_savings: 0,
            },
        );
        // first read kept verbatim; second becomes a delta marker (smaller)
        assert_eq!(
            out["messages"][1]["content"][0]["content"]
                .as_str()
                .unwrap(),
            base
        );
        let second = out["messages"][3]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            second.contains("[tare: delta"),
            "re-read became a delta: {second}"
        );
    }

    #[test]
    fn reported_returns_a_fidelity_report() {
        let req = sample_req();
        let (_out, report) = compress_anthropic_request_reported(
            &req,
            &CompressOpts {
                enabled: true,
                recency_keep: 1,
                min_savings: 0,
            },
            Aggression::default(),
        );
        assert!(report.is_some());
        assert!(report.unwrap().input_tokens > 0);
    }

    #[test]
    fn skip_relevance_backs_off_query_pruning() {
        // OpenAI path honors opts.recency_keep, so recency_keep:0 isolates the relevance lever with
        // two messages: an irrelevant grep output + a jwt-relevant anchor under a jwt task.
        let req = json!({"model":"gpt-x","messages":[
            {"role":"assistant","content":null,"tool_calls":[
                {"id":"a","type":"function","function":{"name":"grep","arguments":"{}"}}]},
            {"role":"tool","tool_call_id":"a",
                "content":"kubernetes helm registry totally unrelated kafka broker partitions offsets"},
            {"role":"assistant","content":null,"tool_calls":[
                {"id":"b","type":"function","function":{"name":"read","arguments":"{\"path\":\"x.rs\"}"}}]},
            {"role":"tool","tool_call_id":"b",
                "content":"jwt authentication middleware verifies the token signature"},
            {"role":"user","content":"work on authentication jwt token verification"}
        ]});
        let opts = CompressOpts {
            enabled: true,
            recency_keep: 0,
            min_savings: 0,
        };
        // relevance ON (normal): the irrelevant grep tool message is pruned
        let pruned = compress_openai_request_reported(&req, &opts, Aggression::default());
        assert!(
            pruned.0["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains("[tare"),
            "relevance ON prunes the irrelevant output"
        );
        // controller back-off (skip_relevance = verbosity-spike response): the SAME output is kept
        let backoff = Aggression {
            skip_relevance: true,
            ..Aggression::default()
        };
        let kept = compress_openai_request_reported(&req, &opts, backoff);
        assert!(
            kept.0["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains("kubernetes"),
            "controller back-off keeps the output — no relevance pruning"
        );
    }

    #[test]
    fn controller_maps_signals_to_aggression_levels() {
        // low fill, no spike = pre-controller default (relevance on, no overrides, no lossy/skeleton)
        let d = controller(false, 0.1);
        assert!(
            !d.skip_relevance
                && d.recency_keep.is_none()
                && !d.skeletonize_code
                && d.lossy_max_rows == 0
                && d.lossy_max_field == 0
        );
        // verbosity spike at low fill = back off (skip relevance)
        assert!(controller(true, 0.1).skip_relevance);
        // window filling = tighten recency + skeletonize code, still no lossy tool-output compaction
        let mid = controller(false, 0.6);
        assert_eq!(mid.recency_keep, Some(2));
        assert!(mid.skeletonize_code && mid.lossy_max_rows == 0 && !mid.skip_relevance);
        // window near full = tighten recency + skeletonize + engage lossy tier
        let hi = controller(false, 0.9);
        assert!(hi.lossy_max_rows > 0 && hi.skeletonize_code && hi.recency_keep == Some(2));
        // a spike steps DOWN one level even when full: drops the lossy tier, keeps skeleton + tighten
        let hi_spike = controller(true, 0.9);
        assert!(
            hi_spike.lossy_max_rows == 0
                && hi_spike.skeletonize_code
                && hi_spike.recency_keep == Some(2)
        );
    }

    #[test]
    fn top_tier_lossy_compacts_large_kept_tool_output() {
        // Long prose tool output: structural passes (JSON/log columnar) don't shrink it, so it stays
        // KEPT (recency-protected) — the top aggression tier telegraphic-compacts it as a last resort.
        let big = "the system processed the request and then it returned a response to the user but \
                   the result was not exactly what we had expected because of a configuration issue \
                   that had been present for quite a while in the deployment pipeline and nobody on \
                   the team had actually noticed it until the incident review happened this morning";
        let req = json!({"messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"a","name":"shell","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"a","content": big}]},
            {"role":"user","content":"continue working"}
        ]});
        let aggr = Aggression {
            lossy_max_rows: 20,
            ..Aggression::default()
        };
        let (out, _r) = compress_anthropic_request_reported(&req, &CompressOpts::default(), aggr);
        let content = out["messages"][1]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            content.contains("[tare: lossy-compacted]"),
            "kept output lossy-compacted: {content:.80}"
        );
        assert!(
            content.len() < big.len(),
            "lossy output is smaller than the original"
        );
    }

    #[test]
    fn skeletonize_code_compacts_kept_file_read() {
        // a kept source-file read (recency-protected); the controller's skeletonize tier drops the
        // function body but keeps the signature/imports — reversible by re-reading.
        let code = "use std::io;\n\npub fn run(x: i32) -> i32 {\n    let a = x + 1;\n    let b = a * 2;\n    let c = b - 3;\n    c\n}\n";
        let req = json!({"messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"r","name":"read","input":{"path":"src/run.rs"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"r","content": code}]},
            {"role":"user","content":"continue"}
        ]});
        let aggr = Aggression {
            skeletonize_code: true,
            ..Aggression::default()
        };
        let (out, _r) = compress_anthropic_request_reported(&req, &CompressOpts::default(), aggr);
        let content = out["messages"][1]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            content.contains("[tare: code skeleton"),
            "code read skeletonized: {content:.80}"
        );
        assert!(
            content.contains("pub fn run(x: i32) -> i32"),
            "signature kept: {content:.120}"
        );
        assert!(
            !content.contains("let b = a * 2"),
            "body elided: {content:.120}"
        );
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
        let out = compress_anthropic_request(
            &req,
            &CompressOpts {
                enabled: true,
                recency_keep: 0,
                min_savings: 0,
            },
        );
        // structure preserved: array content still an array with one block of type text
        assert_eq!(
            out["messages"][1]["content"][0]["content"][0]["type"],
            "text"
        );
        let elided = out["messages"][1]["content"][0]["content"][0]["text"]
            .as_str()
            .unwrap();
        let kept = out["messages"][2]["content"][0]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            elided.contains("[tare"),
            "irrelevant array-content text elided: {elided}"
        );
        assert!(
            kept.contains("jwt authentication middleware"),
            "relevant kept: {kept}"
        );
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
        let out = compress_openai_request(
            &req,
            &CompressOpts {
                enabled: true,
                recency_keep: 0,
                min_savings: 0,
            },
        );
        // structure: same message count, roles, tool_calls untouched
        assert_eq!(out["messages"].as_array().unwrap().len(), 5);
        assert_eq!(
            out["messages"][0]["tool_calls"],
            req["messages"][0]["tool_calls"]
        );
        assert_eq!(out["model"], req["model"]);
        // irrelevant tool message elided; relevant kept
        assert!(out["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("[tare"));
        assert!(out["messages"][3]["content"]
            .as_str()
            .unwrap()
            .contains("jwt authentication middleware"));
        // tool_call_id preserved
        assert_eq!(out["messages"][1]["tool_call_id"], "c1");
    }

    #[test]
    fn openai_disabled_is_passthrough() {
        let req = serde_json::json!({"model":"x","messages":[{"role":"user","content":"hi"}]});
        assert_eq!(
            compress_openai_request(
                &req,
                &CompressOpts {
                    enabled: false,
                    recency_keep: 4,
                    min_savings: 0
                }
            ),
            req
        );
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
        let out = compress_anthropic_request(
            &req,
            &CompressOpts {
                enabled: true,
                recency_keep: 0,
                min_savings: 0,
            },
        );
        // the cached (pre-breakpoint) tool_result is byte-identical
        assert_eq!(
            out["messages"][1]["content"][0]["content"],
            req["messages"][1]["content"][0]["content"]
        );
        // the oldest post-breakpoint irrelevant block (aged out of recency window) is elided
        let after = out["messages"][2]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            after.contains("[tare"),
            "post-breakpoint irrelevant output compressed: {after}"
        );
    }

    #[test]
    fn skips_compression_when_savings_below_threshold() {
        // one tiny tool_result — stubbing it would save ~nothing; with a high min_savings, passthrough.
        let req = serde_json::json!({"messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"g1","name":"grep","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1","content":"x"}]},
            {"role":"user","content":"do something unrelated entirely"}
        ]});
        let out = compress_anthropic_request(
            &req,
            &CompressOpts {
                enabled: true,
                recency_keep: 0,
                min_savings: 1000,
            },
        );
        assert_eq!(out, req, "below the savings threshold -> exact passthrough");
    }
}
