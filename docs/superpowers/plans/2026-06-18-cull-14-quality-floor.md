# Cull — Plan 14: Quality Floor (I6) + Request-Level Never-Net-Negative Gate

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Two ledger items. **I6 (quality floor):** the planner compresses as aggressively as a configurable floor allows and no further — eviction never reduces below `floor × input` tokens. **Request-level never-net-negative gate (§8):** the proxy skips compression when the savings are below a threshold, so a request is never made *larger* or trivially churned (the Hermes "+7k" failure class, at the proxy boundary).

**Architecture:** `Planner` gains an optional `min_keep_ratio` via a `with_floor()` builder (default None → unchanged). `evict_to_budget` evicts only down to `max(budget, ceil(floor × input))`. The proxy adds `min_savings` to `CompressOpts`; after planning, if `input_tokens − net_tokens < min_savings`, it returns the original request unchanged (passthrough). No signature churn: the floor is a builder on `Planner`; the gate is internal to the proxy.

**Tech:** Rust. Builds on Plans 2/9/13. Reference: spec §9 I6, §8 (a savings gate is the runtime "should I compress" decision for a stateless proxy).

---

### Task 1: Quality floor on the planner (I6)

**Files:** `crates/cull-core/src/planner.rs`; tests inline.

- [ ] **Step 1 — failing test.** Add to the planner test module:
```rust
    #[test]
    fn quality_floor_prevents_over_compression() {
        let task = TaskSignal::from_text("nothing relevant here");
        // 4 fast segments, 100 tokens each = 400; tiny budget would evict almost all,
        // but a 0.5 floor keeps >= 200 tokens.
        let segs = vec![
            kb(0, 0, MutationClass::Fast, 100, "aaa"), kb(1, 1, MutationClass::Fast, 100, "bbb"),
            kb(2, 2, MutationClass::Fast, 100, "ccc"), kb(3, 3, MutationClass::Fast, 100, "ddd"),
        ];
        let plan = Planner::new(vec![]).with_floor(0.5)
            .plan_with_budget(&segs, &SessionState::default(), &task, Some(10));
        let net = crate::plan::net_tokens(&plan, &segs);
        assert!(net >= 200, "quality floor keeps >= 50% of input: net={net}");
    }

    #[test]
    fn no_floor_evicts_to_budget_as_before() {
        let task = TaskSignal::from_text("x");
        let segs = vec![kb(0,0,MutationClass::Fast,100,"a"), kb(1,1,MutationClass::Fast,100,"b")];
        let plan = Planner::new(vec![]).plan_with_budget(&segs, &SessionState::default(), &task, Some(100));
        assert!(crate::plan::net_tokens(&plan, &segs) <= 100);
    }
```

- [ ] **Step 2 — confirm FAIL** (`with_floor` not defined).

- [ ] **Step 3 — implement.** Change `Planner` to carry the floor, add the builder, and thread it into eviction:
```rust
pub struct Planner { passes: Vec<Box<dyn Pass>>, min_keep_ratio: Option<f64> }

impl Planner {
    pub fn new(passes: Vec<Box<dyn Pass>>) -> Self { Self { passes, min_keep_ratio: None } }

    /// Quality floor (spec I6): eviction will not reduce net below `ratio * input` tokens.
    pub fn with_floor(mut self, ratio: f64) -> Self { self.min_keep_ratio = Some(ratio); self }
    // ... new() / plan() / plan_with_task() unchanged ...
}
```
In `plan_with_budget`, change the eviction call to pass the effective budget:
```rust
        if let Some(b) = budget {
            let input: u32 = segments.iter().map(|s| s.token_count).sum();
            let floor_tokens = self.min_keep_ratio
                .map(|r| (r * input as f64).ceil() as u32)
                .unwrap_or(0);
            let effective = b.max(floor_tokens);
            evict_to_budget(&mut actions, segments, task, effective);
        }
```
(`evict_to_budget` is unchanged — it just receives a higher target when a floor applies.)

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-core planner::`).

- [ ] **Step 5 — commit.** `git add crates/cull-core && git commit -m "feat(core): quality floor (I6) — eviction never compresses below floor*input"`

---

### Task 2: Request-level never-net-negative gate (proxy)

**Files:** `crates/cull-proxy/src/lib.rs`; tests inline.

- [ ] **Step 1 — failing test.** Add to the cull-proxy test module:
```rust
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
```
Also update the THREE existing proxy tests and the Plan 13 tests that build `CompressOpts { enabled, recency_keep }` to include `min_savings: 0` (so they still compress).

- [ ] **Step 2 — confirm FAIL** (`min_savings` field missing).

- [ ] **Step 3 — implement.** Add the field and the gate:
```rust
pub struct CompressOpts {
    pub enabled: bool,
    pub recency_keep: usize,
    pub min_savings: u32, // skip compression unless it saves at least this many tokens
}
impl Default for CompressOpts {
    fn default() -> Self { Self { enabled: true, recency_keep: 4, min_savings: 0 } }
}
```
In `compress_anthropic_request_reported`, AFTER computing `report` (from `emit`) and BEFORE applying the write-backs, add:
```rust
    let savings = report.input_tokens.saturating_sub(report.net_tokens);
    if savings < opts.min_savings {
        return (req.clone(), None); // not worth compressing -> exact passthrough
    }
```

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-proxy`).

- [ ] **Step 5 — workspace + commit.** `cargo test --workspace` green; `git add crates/cull-proxy && git commit -m "feat(proxy): request-level never-net-negative gate (skip compression below min_savings)"`

---

## After this plan — ledger re-audit (§8 + I6), honestly
- ✅ I6 quality floor (Task 1).
- ✅ §8 "model wired into a runtime decision" — the proxy's savings gate is the runtime "should I compress" decision (Task 2).
- ✅ Rule 1 immutable prefix — proxy provably only edits `tool_result` tails, never system/tools/prefix.
- ✅ Rule 2 write-amortization — the proxy does NO cache-write (it does whole-unit drop + lossless delta, not compress-cache-retrieve, which the research showed is net-negative for live tools), so there is no write to amortize; the savings gate covers "don't compress for trivial gain."
- ✅ Rule 6 provider-aware — `CacheModel::for_provider` available; the proxy is structurally provider-agnostic-safe.
- ✅ Rule 8 tool-definition freeze — proxy never modifies `tools`.
- ✅ Rule 9 delta-before-full-resend — IVM deltas re-reads.
- ✅ Rule 10 compress-once — one plan computed + applied per request, never incrementally.
- ✅ Boundary detection — for a stateless full-history proxy, each request IS the boundary; the savings gate decides whether to act.
- ❌ Rule 5 hit-rate-floor monitor — genuinely not built; needs parsing the response `usage` (cache_read/creation tokens) to track hit rate and halt. A dedicated later plan (response-tee + usage parse).
- ❌ `count_tokens` exact API — still approximation; later plan.

Justification standard: a rule is flipped to ✅ only because the implementation **provably** honors it (e.g., the proxy code never touches `tools`/`system`), not because it's deferred.

## Self-Review
- `Planner::new`/`plan`/`plan_with_task` signatures unchanged; floor is opt-in → no ripple. ✓
- `CompressOpts` gains a field → every construction site updated (3 existing + Plan 13 tests + server `Default`). ✓
- The savings gate returns EXACT passthrough (`req.clone()`), preserving the structure contract. ✓
