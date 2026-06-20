# Cull — Plan 22: Cache-Prefix Immutability (R1 + Boundary)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Close §8 Rule 1 (immutable prefix) + Boundary detection. The proxy must NOT compress content that sits inside the agent's **cached prefix** — doing so changes those bytes and busts the prompt cache. Anthropic marks the cached region with `cache_control` breakpoints; the proxy will only compress `tool_result`s that come AFTER the last breakpoint. (No breakpoints → no caching to protect → compress all, as before.)

**Architecture:** Before collecting compressible `tool_result`s, find the cached-prefix boundary = the last `(message_index, block_index)` whose content block (or the message) carries `cache_control`. Then skip any `tool_result` at `(mi, bi) <= boundary` (lexicographic) — it's inside the cached prefix. This is the correct, cache-safe behavior: it compresses only the uncached tail. It compresses *less* than before, but it no longer busts the cache (the bug flagged in the ledger).

**Tech:** Rust, serde_json. Builds on Plans 10/13/20. Reference: spec §8 Rule 1, Boundary detection; Anthropic prompt-caching docs (`cache_control` breakpoints).

---

### Task 1: Respect cache_control breakpoints (Anthropic)

**Files:** `crates/cull-proxy/src/lib.rs`; tests inline.

- [ ] **Step 1 — failing test.**
```rust
    #[test]
    fn does_not_compress_tool_results_inside_cached_prefix() {
        // breakpoint on the FIRST tool_result block -> it (and everything before) is cached and untouchable;
        // the later tool_result is after the breakpoint -> compressible.
        let req = serde_json::json!({"messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"g1","name":"grep","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1",
                "content":"kafka registry unrelated junk in the CACHED prefix one two three",
                "cache_control":{"type":"ephemeral"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"g1",
                "content":"also unrelated kafka content AFTER the breakpoint four five six"}]},
            {"role":"user","content":"do something about authentication jwt"}
        ]});
        let out = compress_anthropic_request(&req, &CompressOpts { enabled: true, recency_keep: 0, min_savings: 0 });
        // the cached (pre-breakpoint) tool_result is byte-identical
        assert_eq!(out["messages"][1]["content"][0]["content"], req["messages"][1]["content"][0]["content"]);
        // the post-breakpoint irrelevant one may be elided
        let after = out["messages"][2]["content"][0]["content"].as_str().unwrap();
        assert!(after.contains("[cull"), "post-breakpoint irrelevant output compressed: {after}");
    }
```
(Keep all existing proxy tests; without `cache_control` they behave exactly as before.)

- [ ] **Step 2 — confirm FAIL** (currently the pre-breakpoint tool_result gets compressed).

- [ ] **Step 3 — implement.** In `compress_anthropic_request_reported`, BEFORE the collection loop, compute the boundary; in the collection loop, skip blocks at/below it.
```rust
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
```
Then in the collection loop, right after computing `(bi, b)` for a `tool_result`, add at the top of its handling:
```rust
                    if in_cached_prefix(mi, bi) { continue; }
```
(So cached-prefix tool_results are never collected → never compressed.)

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-proxy`).

- [ ] **Step 5 — workspace + commit.** `cargo test --workspace` green; `git add crates/cull-proxy && git commit -m "feat(proxy): respect cache_control — never compress inside the cached prefix (R1)"`

---

## After this plan — ledger
- ✅ §8 Rule 1 immutable prefix — proxy respects `cache_control`, only compresses the uncached tail.
- ✅ §8 Boundary detection — the cache breakpoint IS the boundary; compress only after it.
- §8 Rule 5 (hit-rate floor via response `usage` parsing) and OpenAI implicit-prefix awareness remain ⚠️/❌ — noted; OpenAI has no explicit breakpoint signal, so its prefix can't be located by the proxy (documented limitation).

## Self-Review
- No `cache_control` → boundary None → behavior unchanged (existing tests hold). ✓
- Message-level `cache_control` covers the whole message (uses `usize::MAX` for the block index). ✓
- Skipping cached-prefix tool_results means they're byte-identical in the output → cache preserved. ✓
