# Cull — Plan 20: Array-form tool_result Content (§10)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** The proxy currently compresses only `tool_result` blocks whose `content` is a STRING. Anthropic also sends `content` as an ARRAY of blocks (e.g. `[{"type":"text","text":"…"}]`). Extend the transform to compress the text blocks inside array-form `tool_result` content too, preserving array structure exactly.

**Architecture:** The collection step gains a location form `(msg, block, Option<inner_index>)`: `None` = string content (existing); `Some(k)` = the k-th text block inside an array content. Write-back targets `…content` (string case) or `…content[k].text` (array case) via panic-safe `get_mut`. Same class/path metadata per `tool_result`. No other behavior changes.

**Tech:** Rust, serde_json. Builds on Plans 10/13. Reference: spec §10.

> **`system`/`tools` compression — deliberate omission (recorded, not skipped):** the spec lists it, but independent research (StackOne MCP study) shows schema compression makes models confuse similarly-named tools, and `system` is load-bearing instructions. Compressing them is net-harmful, so Cull intentionally never touches `system`/`tools`. This is a justified design decision, marked 🚫 in the ledger.

---

### Task 1: Compress array-form tool_result content

**Files:** `crates/cull-proxy/src/lib.rs`; tests inline.

- [ ] **Step 1 — failing test.** Add to the cull-proxy test module:
```rust
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
```
(Keep the existing string-content tests; they must still pass.)

- [ ] **Step 2 — confirm FAIL** (array content currently ignored → not compressed).

- [ ] **Step 3 — implement.** In `compress_anthropic_request_reported`:

(a) change `locs` to hold the inner index: `let mut locs: Vec<(usize, usize, Option<usize>)> = Vec::new();`

(b) in the collection loop, replace the string-only `tool_result` handling with both forms:
```rust
                if b.get("type").and_then(Value::as_str) == Some("tool_result") {
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
```

(c) in the write-back loop, target the right field by location form:
```rust
    for (entry, (mi, bi, inner)) in plan.entries.iter().zip(locs.iter()) {
        let replacement = match &entry.action {
            SegmentAction::Drop(reason) => Some(format!("[cull: tool output elided — {reason:?}]")),
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
```

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-proxy` — new + all existing, including the 3 string-content fidelity tests).

- [ ] **Step 5 — workspace + commit.** `cargo test --workspace` green; `git add crates/cull-proxy && git commit -m "feat(proxy): compress text inside array-form tool_result content"`

---

## After this plan — ledger
- ✅ §10 array `tool_result` content.
- 🚫 §10 `system`/`tools` compression — deliberate, justified omission (record the decision; not a TODO).

## Self-Review
- String-content path unchanged (`None` location) → existing tests hold. ✓
- Array path preserves the block structure (only the inner `text` string changes) → no structure violation. ✓
- Panic-safe `get_mut` throughout. ✓
