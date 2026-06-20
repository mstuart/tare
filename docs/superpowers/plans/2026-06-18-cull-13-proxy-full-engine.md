# Cull — Plan 13: Wire the Full Engine into the Proxy

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Make the proxy use the whole compression engine, not just relevance+dedup. Extract tool name + file path from `tool_use` blocks (via `tool_use_id`), classify `tool_result` segments richly, run the FULL pass set (supersession + IVM/delta + dedup + relevance) over the request's history, handle delta `Replace`s, and surface the `FidelityReport`. Closes ledger lines: §10 supersession+IVM-in-proxy, §10 report-surfaced, §7 A2 cross-turn (the request carries full history), and starts §8 wiring.

**Architecture:** Anthropic resends the full conversation each request, so running supersession/IVM over the request's messages IS cross-turn. `tool_use` blocks carry the tool `name` and `input` (often a path); linking `tool_result.tool_use_id` → that metadata lets supersession key on tool name and IVM key on file path. The core returns `(Value, Option<FidelityReport>)`; `compress_anthropic_request` keeps its `-> Value` signature by delegating. The server adds `x-cull-*` response headers from the report.

**Tech:** Rust, serde_json. Builds on Plans 10–11 + the engine. Reference: spec §10, §7 A1/A2, §11.

---

### Task 1: Rich metadata + full pass set + Replace handling

**Files:** `crates/cull-proxy/src/lib.rs`; tests inline.

- [ ] **Step 1 — failing tests.** Add to the cull-proxy test module:
```rust
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
        let out = compress_anthropic_request(&req, &CompressOpts { enabled: true, recency_keep: 0 });
        let older = out["messages"][1]["content"][0]["content"].as_str().unwrap();
        let newer = out["messages"][3]["content"][0]["content"].as_str().unwrap();
        assert!(older.contains("[cull"), "older same-tool output superseded: {older}");
        assert!(newer.contains("NEW grep run"), "newest kept: {newer}");
    }

    #[test]
    fn ivm_deltas_a_changed_file_reread() {
        let base = "fn a(){}\nfn b(){}\nfn c(){}\nfn d(){}\nfn e(){}\nfn f(){}";
        let changed = "fn a(){}\nfn b(){}\nfn CHANGED(){}\nfn d(){}\nfn e(){}\nfn f(){}";
        let req = serde_json::json!({"messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"r1","name":"read","input":{"path":"src/x.rs"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"r1","content": base}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"r2","name":"read","input":{"path":"src/x.rs"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"r2","content": changed}]},
            {"role":"user","content":"keep working on src/x.rs CHANGED"}
        ]});
        let out = compress_anthropic_request(&req, &CompressOpts { enabled: true, recency_keep: 0 });
        // first read kept verbatim; second becomes a delta marker (smaller)
        assert_eq!(out["messages"][1]["content"][0]["content"].as_str().unwrap(), base);
        let second = out["messages"][3]["content"][0]["content"].as_str().unwrap();
        assert!(second.contains("[cull: delta"), "re-read became a delta: {second}");
    }

    #[test]
    fn reported_returns_a_fidelity_report() {
        let req = sample_req();
        let (_out, report) = compress_anthropic_request_reported(&req, &CompressOpts { enabled: true, recency_keep: 1 });
        assert!(report.is_some());
        assert!(report.unwrap().input_tokens > 0);
    }
```
(The existing `compresses_irrelevant_tool_result_keeps_structure`, `disabled_is_passthrough`, `no_tool_results_is_passthrough` MUST still pass unchanged.)

- [ ] **Step 2 — confirm FAIL** (`cargo test -p cull-proxy` — new symbols/behavior missing).

- [ ] **Step 3 — implement.** Replace the body of `compress_anthropic_request` so the core logic lives in a new `compress_anthropic_request_reported` returning `(Value, Option<FidelityReport>)`, and `compress_anthropic_request` delegates (`.0`). New imports:
```rust
use std::collections::HashMap;
use cull_core::passes::{structural_passes, query_passes};
use cull_core::emit::{emit, FidelityReport};
use cull_core::plan::SegmentAction;
```
Implementation:
```rust
pub fn compress_anthropic_request(req: &Value, opts: &CompressOpts) -> Value {
    compress_anthropic_request_reported(req, opts).0
}

pub fn compress_anthropic_request_reported(req: &Value, opts: &CompressOpts) -> (Value, Option<FidelityReport>) {
    if !opts.enabled { return (req.clone(), None); }
    let mut out = req.clone();

    // tool_use_id -> (tool name, optional path)
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

    // collect tool_result string contents with rich kind + path
    let mut raws: Vec<RawBlock> = Vec::new();
    let mut locs: Vec<(usize, usize)> = Vec::new();
    if let Some(msgs) = out.get("messages").and_then(Value::as_array) {
        for (mi, m) in msgs.iter().enumerate() {
            let Some(blocks) = m.get("content").and_then(Value::as_array) else { continue; };
            for (bi, b) in blocks.iter().enumerate() {
                if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                    if let Some(text) = b.get("content").and_then(Value::as_str) {
                        let (class, path) = b.get("tool_use_id").and_then(Value::as_str)
                            .and_then(|id| meta.get(id)).cloned()
                            .unwrap_or_else(|| ("tool_result".to_string(), None));
                        let kind = if path.is_some() { SegmentKind::FileRead }
                                   else { SegmentKind::ToolOutput { class } };
                        raws.push(RawBlock { role: Role::Tool, kind, text: text.to_string(), path });
                        locs.push((mi, bi));
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

    debug_assert_eq!(plan.entries.len(), locs.len());
    for (entry, (mi, bi)) in plan.entries.iter().zip(locs.iter()) {
        let replacement = match &entry.action {
            SegmentAction::Drop(reason) => Some(format!("[cull: tool output elided — {reason:?}]")),
            SegmentAction::Replace { rendered, .. } =>
                Some(format!("[cull: delta vs an earlier read]\n{}", String::from_utf8_lossy(rendered))),
            SegmentAction::Keep => None,
        };
        if let Some(text) = replacement {
            if let Some(content) = out
                .get_mut("messages").and_then(Value::as_array_mut)
                .and_then(|ms| ms.get_mut(*mi))
                .and_then(|m| m.get_mut("content")).and_then(Value::as_array_mut)
                .and_then(|bs| bs.get_mut(*bi))
                .and_then(|b| b.get_mut("content"))
            { *content = Value::String(text); }
        }
    }
    (out, Some(report))
}
```

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-proxy`, all old + new tests).

- [ ] **Step 5 — commit.** `git add crates/cull-proxy && git commit -m "feat(proxy): full pass set (supersession+IVM via tool metadata) + delta handling + reported variant"`

---

### Task 2: Surface the fidelity report as response headers

**Files:** `crates/cull-proxy/src/server.rs`; extend `crates/cull-proxy/tests/proxy.rs`.

- [ ] **Step 1 — failing test.** Add to `tests/proxy.rs` a check that the proxy response carries `x-cull-net-tokens` (and the upstream still got the compressed body). Reuse the existing integration test harness; assert `resp.headers().get("x-cull-net-tokens").is_some()`.

- [ ] **Step 2 — confirm FAIL.**

- [ ] **Step 3 — implement.** In `server.rs::handle_messages`, switch the JSON path to `compress_anthropic_request_reported`, and when a report is present add headers before streaming the upstream response:
```rust
    let (forward_body, report) = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(req) => {
            let (compressed, report) = cull_proxy::compress_anthropic_request_reported(&req, &state.opts);
            (serde_json::to_vec(&compressed).unwrap_or_else(|_| body.to_vec()), report)
        }
        Err(_) => (body.to_vec(), None),
    };
```
Then on the success branch, after building the response builder from the upstream response, if `let Some(r) = &report`, add:
```rust
            builder = builder
                .header("x-cull-input-tokens", r.input_tokens.to_string())
                .header("x-cull-net-tokens", r.net_tokens.to_string())
                .header("x-cull-dropped", r.dropped.to_string());
```
(Import `cull_proxy::compress_anthropic_request_reported` or call via the crate path; adjust the existing `compress_anthropic_request` call site.)

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-proxy --test proxy`).

- [ ] **Step 5 — workspace test + commit.** `cargo test --workspace` green, then `git add crates/cull-proxy && git commit -m "feat(proxy): surface FidelityReport as x-cull-* response headers"`.

---

## After this plan — update the ledger
Flip to ✅ in `docs/superpowers/COMPLETENESS.md`: §10 "supersession + IVM wired into proxy", §10 "FidelityReport surfaced from proxy", §7 A2 (cross-turn within-request). Note §8 economic-gate remains ❌ pending the stateful compress-cache-retrieve design (the amortization gate is for caching the compressed form across turns, not stateless per-request input compression — a real design fork to address head-on, not silently defer).

## Self-Review
- Existing structural-fidelity tests unchanged & must pass (signature preserved via delegation). ✓
- Supersession now keys on real tool name; IVM keys on real path; both via `tool_use_id` linkage. ✓
- Delta `Replace` is rendered as a labeled diff in the tool_result content (lossless: base read is kept in the same request). ✓
- Write-back stays panic-safe (`get_mut` chain). ✓
