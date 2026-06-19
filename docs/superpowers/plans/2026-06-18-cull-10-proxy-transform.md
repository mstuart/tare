# Cull — Plan 10: Anthropic Request Compression (Transform) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the pure, testable core of the proxy: `compress_anthropic_request(req, opts) -> req'` that compresses an Anthropic Messages API request by running the engine's query-relevance + dedup passes over `tool_result` outputs and **stubbing dropped ones in place** — never removing blocks, never reordering, never altering `tool_use` blocks, pairing, `system`, `tools`, `model`, or `max_tokens`.

**Architecture:** Conservative by design. v1 compresses only `tool_result` blocks whose `content` is a string (the bulk of agent tokens, structurally simplest, lowest risk). Each becomes a `ToolOutput` segment; the planner (relevance + dedup, recency-guarded) decides Keep/Drop; for a Drop we replace that block's `content` string with a short `[cull: …]` stub, preserving the block, its `tool_use_id`, message order, and every other field. The task signal is derived from the last user message. No `Replace`/delta in the proxy v1 (those need file identity Anthropic doesn't carry); no `system`/`tools` compression. The HTTP server that calls this is the next plan.

**Tech Stack:** Rust, `serde_json`. Builds on Plans 1–9 (`segment`/`RawBlock`, `Planner`/`plan_with_task`, `RelevancePass`/`ExactDedupPass`, `TaskSignal`, `SegmentAction`/`DropReason`, `ApproxCounter`). Reference: spec §10 (proxy), §7 B1 (relevance is the wedge applied here).

> **Safety invariants this code MUST hold (tested):** block count per message unchanged; `tool_use` blocks byte-identical; `system`/`tools`/`model`/`max_tokens` byte-identical; message roles + order unchanged; only `tool_result` *string* contents may change (to a stub); with `opts.enabled=false` the output equals the input.

---

## File Structure

```
crates/cull-proxy/Cargo.toml     # deps: cull-core, cull-tokenize, serde, serde_json
crates/cull-proxy/src/lib.rs     # CompressOpts, last_user_text, compress_anthropic_request
```

---

### Task 1: Crate setup + helpers

**Files:** Replace `crates/cull-proxy/Cargo.toml` and `crates/cull-proxy/src/lib.rs`; tests inline.

- [ ] **Step 1: Dependencies**

Replace `crates/cull-proxy/Cargo.toml`:
```toml
[package]
name = "cull-proxy"
version = "0.0.0"
edition.workspace = true

[dependencies]
cull-core = { path = "../cull-core" }
cull-tokenize = { path = "../cull-tokenize" }
serde = { workspace = true }
serde_json = "1"
```

- [ ] **Step 2: Write the failing test**

Put in `crates/cull-proxy/src/lib.rs`:
```rust
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
}
```

- [ ] **Step 3: Implement crate skeleton + `last_user_text`**

Above the test module:
```rust
use serde_json::Value;

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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-proxy`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/cull-proxy
git commit -m "feat(proxy): crate setup + CompressOpts + last_user_text"
```

---

### Task 2: compress_anthropic_request

**Files:** Modify `crates/cull-proxy/src/lib.rs`; tests inline.

- [ ] **Step 1: Write the failing tests (structural fidelity is the point)**

Add to the `cull-proxy` test module:
```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cull-proxy compress`
Expected: FAIL (`compress_anthropic_request` not defined).

- [ ] **Step 3: Implement the transform**

Add to `crates/cull-proxy/src/lib.rs`:
```rust
use cull_core::segmenter::{segment, RawBlock};
use cull_core::segment::{Role, SegmentKind};
use cull_core::planner::{Pass, Planner};
use cull_core::passes::{ExactDedupPass, RelevancePass};
use cull_core::session::SessionState;
use cull_core::task::TaskSignal;
use cull_core::plan::SegmentAction;
use cull_tokenize::ApproxCounter;

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
    for (entry, (mi, bi)) in plan.entries.iter().zip(locs.iter()) {
        if let SegmentAction::Drop(reason) = &entry.action {
            out["messages"][*mi]["content"][*bi]["content"] =
                Value::String(format!("[cull: tool output elided — {reason:?}]"));
        }
    }
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cull-proxy`
Expected: PASS (all). The `recency_keep: 1` in the main test ensures the older kafka result (not in the last 1 position) is eligible to drop while the jwt result (relevant) is kept by relevance regardless.

- [ ] **Step 5: Full workspace test + commit**

Run: `cargo test --workspace`
Expected: PASS.

```bash
git add crates/cull-proxy
git commit -m "feat(proxy): compress_anthropic_request (relevance+dedup over tool_result, structure-preserving)"
```

---

## Self-Review

**1. Spec coverage:**
- §10 proxy request compression (the testable transform) → Task 2. ✓
- §7 B1 relevance (the wedge) applied to tool outputs → Task 2. ✓
- Structure/pairing fidelity (never break the agent) → the safety invariants + tests. ✓
- HTTP server (listen/forward/stream) → next plan. `system`/`tools` compression, array-content tool_results, supersession/IVM in the proxy → deferred (documented), as they need metadata Anthropic doesn't cleanly carry.

**2. Placeholder scan:** No vague steps. The deferred items are explicit scope notes. ✓

**3. Type consistency:** `CompressOpts`/`last_user_text` (Task 1) used by `compress_anthropic_request` (Task 2). `segment`/`RawBlock` (with `path`), `RelevancePass`/`ExactDedupPass`, `Planner::plan_with_task`, `TaskSignal`, `SegmentAction::Drop`, `ApproxCounter` from Plans 1–9. The write-back uses `serde_json::Value` IndexMut on paths that were just confirmed to exist during collection. ✓

**4. Ambiguity check:** Only `tool_result` blocks with STRING content are touched; arrays/other blocks are skipped (safe). Dropped → content replaced by a stub string (block + `tool_use_id` kept). `enabled=false` or no tool_results → exact passthrough (`out == req`). Segment positions follow message order, so the relevance recency guard keeps recent tool outputs. The planner's invariants still hold (these are Fast segments; nothing frozen here). ✓

**Outcome:** The proxy can compress a real Anthropic request safely — irrelevant tool outputs elided, structure and tool-call pairing intact. Next: the HTTP server (axum + reqwest) that calls this and streams responses through verbatim; then the benchmark.
