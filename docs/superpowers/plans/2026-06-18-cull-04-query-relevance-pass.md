# Cull — Plan 4: Query-Conditioned Relevance Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the wedge — query-conditioned compression. Extract the current task's technical symbols, expose them to passes via `PlanCtx`, and add a `RelevancePass` that drops droppable tool-result/file-read segments whose symbols don't overlap the task, guarded by recency so recent context is never false-dropped.

**Architecture:** A `TaskSignal` (a set of lowercased technical symbols) is extracted from the task text and threaded into `PlanCtx`. `Planner` keeps its existing `plan(segments, session)` (which uses an empty task signal, so query passes are inert) and gains `plan_with_task(segments, session, task)`. `RelevancePass` is another **Drop-only** pass — it drops *irrelevant* whole units, identical machinery to Plan 3's passes, automatically held to I1/I3/I4. It is deterministic (symbol overlap, zero model calls) and conservative (no task signal → drops nothing; recent segments always kept). PRF query expansion and embedding-salience scoring are explicit future upgrades on this same pass.

**Tech Stack:** Rust, `regex` (symbol extraction). Builds on Plan 1–3 (`Segment`, `SegmentKind`, the `Pass`/`Planner`/`PlanCtx` machinery, `is_droppable`-style kind scoping). Reference: spec §7 B1 (program-slice/taint — this is the deterministic symbol-overlap v1), B2 (PRF — deferred), B3 (embedding — deferred).

---

## File Structure

```
crates/cull-core/src/task.rs              # TaskSignal + extract_symbols (+ stopwords)
crates/cull-core/src/planner.rs           # PlanCtx gains `task`; Planner gains plan_with_task()
crates/cull-core/src/passes/relevance.rs  # RelevancePass
crates/cull-core/src/passes/mod.rs        # add relevance + query_passes()
crates/cull-core/src/lib.rs               # add `pub mod task;` + re-exports
```

---

### Task 1: TaskSignal + thread it through the planner

**Files:** Create `crates/cull-core/src/task.rs`; modify `crates/cull-core/src/planner.rs`; modify `crates/cull-core/src/lib.rs`; tests inline in `task.rs`.

- [ ] **Step 1: Write the failing test**

Put in `crates/cull-core/src/task.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_technical_symbols_and_drops_stopwords() {
        let sig = TaskSignal::from_text("fix the auth bug in src/jwt.rs TokenExpiredError");
        assert!(sig.symbols.contains("auth"));
        assert!(sig.symbols.contains("src/jwt.rs"));
        assert!(sig.symbols.contains("tokenexpirederror")); // lowercased
        assert!(!sig.symbols.contains("the")); // stopword
        assert!(!sig.symbols.contains("fix")); // stopword
    }

    #[test]
    fn empty_text_is_empty_signal() {
        assert!(TaskSignal::from_text("   ").is_empty());
        assert!(TaskSignal::empty().is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core task::`
Expected: FAIL (`TaskSignal` not defined).

- [ ] **Step 3: Implement `task.rs`**

Above the test module:
```rust
use std::collections::HashSet;
use std::sync::OnceLock;
use regex::Regex;

/// The current task's technical symbols (function/type/file names, error codes, paths),
/// lowercased. Drives query-conditioned passes. An empty signal means "no task" → query
/// passes do nothing (safe default).
#[derive(Debug, Clone, Default)]
pub struct TaskSignal { pub symbols: HashSet<String> }

impl TaskSignal {
    pub fn empty() -> Self { Self::default() }
    pub fn from_text(text: &str) -> Self { Self { symbols: extract_symbols(text) } }
    pub fn is_empty(&self) -> bool { self.symbols.is_empty() }
}

fn ident_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // identifiers, dotted names, and path-like tokens (stops at ':' so "jwt.rs:42" -> "jwt.rs")
    R.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_./-]{2,}").unwrap())
}

fn is_stopword(s: &str) -> bool {
    const STOP: &[&str] = &[
        "the","and","for","this","that","with","from","into","not","but","all","can","you","are",
        "let","pub","use","mut","self","return","const","fix","add","make","get","set","new","out",
        "the.","src","run","why","how","fn.","ref",
    ];
    STOP.contains(&s)
}

/// Extract candidate technical symbols, lowercased, length >= 3, minus common stopwords.
/// Heuristic relevance signal; tree-sitter precision and embedding salience are later upgrades.
pub fn extract_symbols(text: &str) -> HashSet<String> {
    ident_re().find_iter(text)
        .map(|m| m.as_str().to_ascii_lowercase())
        .filter(|s| s.len() >= 3 && !is_stopword(s))
        .collect()
}
```

Note on the test: `src/jwt.rs` survives because the regex captures the whole path token and `src/jwt.rs` is not in the stopword list (bare `src` is). If `extract_symbols("src/jwt.rs")` yields `src/jwt.rs` as one token, the test passes. Verify the regex captures the full path; if your regex splits on `/`, widen the char class as shown (`./-` are included).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core task::`
Expected: PASS (both).

- [ ] **Step 5: Thread `TaskSignal` through the planner**

In `crates/cull-core/src/planner.rs`:

(a) Add a `task` field to `PlanCtx`:
```rust
pub struct PlanCtx<'a> {
    pub segments: &'a [Segment],
    pub session: &'a SessionState,
    pub task: &'a crate::task::TaskSignal,
}
```

(b) Split `Planner::plan` so the existing signature is preserved and delegates with an empty task:
```rust
impl Planner {
    pub fn new(passes: Vec<Box<dyn Pass>>) -> Self { Self { passes } }

    /// Plan with no task signal (query-conditioned passes are inert).
    pub fn plan(&self, segments: &[Segment], session: &SessionState) -> CompressionPlan {
        self.plan_with_task(segments, session, &crate::task::TaskSignal::empty())
    }

    /// Plan conditioned on the current task. The body is the previous `plan` body, with
    /// `PlanCtx { segments, session, task }`.
    pub fn plan_with_task(
        &self,
        segments: &[Segment],
        session: &SessionState,
        task: &crate::task::TaskSignal,
    ) -> CompressionPlan {
        let mut actions: Vec<SegmentAction> = vec![SegmentAction::Keep; segments.len()];
        let index: std::collections::HashMap<SegmentId, usize> =
            segments.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
        let ctx = PlanCtx { segments, session, task };
        for pass in &self.passes {
            for entry in pass.propose(&ctx) {
                if let Some(&i) = index.get(&entry.id) { actions[i] = entry.action; }
            }
        }
        enforce_invariants(&mut actions, segments);
        CompressionPlan {
            entries: segments.iter().zip(actions).map(|(s, a)| PlanEntry { id: s.id, action: a }).collect(),
        }
    }
}
```

(c) Add to `crates/cull-core/src/lib.rs`: `pub mod task;` and `pub use task::TaskSignal;` in the re-export block.

Run `cargo test -p cull-core` → Expected: PASS (all existing tests unchanged — they call `plan()`, which now delegates with an empty task signal; existing passes don't read `ctx.task`).

```bash
git add crates/cull-core
git commit -m "feat(core): TaskSignal + plan_with_task threading the query signal into PlanCtx"
```

---

### Task 2: RelevancePass

**Files:** Create `crates/cull-core/src/passes/relevance.rs`; modify `crates/cull-core/src/passes/mod.rs`; test inline.

- [ ] **Step 1: Write the failing test**

Put in `crates/cull-core/src/passes/relevance.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use crate::plan::{SegmentAction, DropReason};
    use crate::planner::Planner;
    use crate::session::SessionState;
    use crate::task::TaskSignal;

    fn seg(id: u64, pos: usize, kind: SegmentKind, text: &str) -> Segment {
        Segment {
            id: SegmentId(id), kind, role: Role::Tool, bytes: text.as_bytes().to_vec(),
            token_count: 10, position: pos, mutation_class: MutationClass::Fast,
            origin: Origin::default(), protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn drops_irrelevant_old_segment_keeps_relevant_and_recent() {
        let task = TaskSignal::from_text("authentication jwt middleware");
        let segs = vec![
            seg(0, 0, SegmentKind::FileRead, "jwt verify middleware token"),  // relevant (jwt, middleware)
            seg(1, 1, SegmentKind::ToolOutput { class: "grep".into() }, "postgres connection pool retries"), // irrelevant, old
            seg(2, 20, SegmentKind::ToolOutput { class: "grep".into() }, "unrelated kafka topic"), // irrelevant BUT recent
        ];
        let plan = Planner::new(vec![Box::new(RelevancePass { recency_keep: 6 })])
            .plan_with_task(&segs, &SessionState::default(), &task);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);                          // relevant
        assert_eq!(plan.entries[1].action, SegmentAction::Drop(DropReason::IrrelevantBySlice)); // irrelevant + old
        assert_eq!(plan.entries[2].action, SegmentAction::Keep);                          // recent (within recency_keep of max pos 20)
    }

    #[test]
    fn no_task_signal_drops_nothing() {
        let segs = vec![seg(0, 0, SegmentKind::FileRead, "anything at all")];
        let plan = Planner::new(vec![Box::new(RelevancePass::default())])
            .plan(&segs, &SessionState::default()); // plan() => empty task
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn non_droppable_kinds_are_never_dropped_by_relevance() {
        let task = TaskSignal::from_text("authentication");
        // a conversation turn with no overlap, old position — must still be kept
        let segs = vec![seg(0, 0, SegmentKind::ConversationTurn, "totally unrelated chatter")];
        let plan = Planner::new(vec![Box::new(RelevancePass { recency_keep: 0 })])
            .plan_with_task(&segs, &SessionState::default(), &task);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core relevance`
Expected: FAIL (`RelevancePass` not defined).

- [ ] **Step 3: Implement the pass**

Above the test module:
```rust
use crate::plan::{DropReason, PlanEntry, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::SegmentKind;
use crate::task::extract_symbols;

/// Query-conditioned relevance pruning (spec §7 B1, deterministic v1). Drops droppable
/// tool-result/file-read segments whose symbols are disjoint from the task query symbols,
/// keeping the most recent `recency_keep` positions regardless (guards against false drops).
/// No task signal → drops nothing. PRF expansion (B2) and embedding salience (B3) are future
/// upgrades that would enrich `ctx.task.symbols` / replace the disjoint check.
pub struct RelevancePass { pub recency_keep: usize }

impl Default for RelevancePass { fn default() -> Self { Self { recency_keep: 6 } } }

fn is_droppable_kind(kind: &SegmentKind) -> bool {
    matches!(
        kind,
        SegmentKind::FileRead | SegmentKind::DirListing | SegmentKind::ToolOutput { .. }
            | SegmentKind::StackTrace | SegmentKind::TestOutput | SegmentKind::Diff
    )
}

impl Pass for RelevancePass {
    fn name(&self) -> &'static str { "query-relevance" }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        if ctx.task.is_empty() { return Vec::new(); }
        let max_pos = ctx.segments.iter().map(|s| s.position).max().unwrap_or(0);
        ctx.segments.iter().filter_map(|s| {
            if !is_droppable_kind(&s.kind) { return None; }
            if max_pos.saturating_sub(s.position) < self.recency_keep { return None; }
            let text = String::from_utf8_lossy(&s.bytes);
            let seg_symbols = extract_symbols(&text);
            if seg_symbols.is_disjoint(&ctx.task.symbols) {
                Some(PlanEntry { id: s.id, action: SegmentAction::Drop(DropReason::IrrelevantBySlice) })
            } else {
                None
            }
        }).collect()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core relevance`
Expected: PASS (all three).

- [ ] **Step 5: Wire + commit**

Add to `crates/cull-core/src/passes/mod.rs`:
```rust
pub mod relevance;
pub use relevance::RelevancePass;
```
Add `pub use passes::RelevancePass;` to `crates/cull-core/src/lib.rs` re-exports. Run `cargo build -p cull-core` → PASS.
```bash
git add crates/cull-core
git commit -m "feat(core): query-conditioned RelevancePass (deterministic symbol-overlap slice)"
```

---

### Task 3: Query pass pipeline + integration test

**Files:** Modify `crates/cull-core/src/passes/mod.rs`; test inline.

- [ ] **Step 1: Write the failing test**

Add to `crates/cull-core/src/passes/mod.rs` test module (create one if absent, or extend the existing `mod tests`):
```rust
#[cfg(test)]
mod query_tests {
    use crate::segment::*;
    use crate::plan::{SegmentAction, net_tokens, input_tokens};
    use crate::planner::Planner;
    use crate::session::SessionState;
    use crate::task::TaskSignal;

    fn seg(id: u64, pos: usize, text: &str) -> Segment {
        Segment {
            id: SegmentId(id), kind: SegmentKind::FileRead, role: Role::Tool,
            bytes: text.as_bytes().to_vec(), token_count: 10, position: pos,
            mutation_class: MutationClass::Fast, origin: Origin::default(),
            protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn query_pipeline_drops_irrelevant_and_never_increases() {
        let task = TaskSignal::from_text("authentication jwt");
        let segs = vec![
            seg(0, 0, "jwt authentication handler"),  // relevant
            seg(1, 1, "kubernetes deployment yaml"),  // irrelevant, old
            seg(2, 2, "grafana dashboard metrics"),   // irrelevant, old
        ];
        let before = input_tokens(&segs);
        let plan = Planner::new(super::query_passes()).plan_with_task(&segs, &SessionState::default(), &task);
        let after = net_tokens(&plan, &segs);
        assert!(after < before);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn query_passes_returns_relevance_pass() {
        assert_eq!(super::query_passes().len(), 1);
    }
}
```
(Use a distinct `query_tests` module name so it does not collide with the existing `tests` module in `mod.rs`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core query_tests`
Expected: FAIL (`query_passes` not defined).

- [ ] **Step 3: Implement the helper**

Add to `crates/cull-core/src/passes/mod.rs` (below the existing `structural_passes`):
```rust
use crate::planner::Pass as _PassForQuery; // ensure trait in scope if not already

/// The default query-conditioned pass pipeline. Currently the deterministic RelevancePass;
/// PRF and embedding-salience passes are added here in a later plan.
pub fn query_passes() -> Vec<Box<dyn crate::planner::Pass>> {
    vec![Box::new(RelevancePass::default())]
}
```
(If `structural_passes` already imported `crate::planner::Pass`, drop the redundant `use` line above and reuse it — keep one import, no duplicates.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core query_tests`
Expected: PASS (both).

- [ ] **Step 5: Full workspace test + commit**

Run: `cargo test --workspace`
Expected: PASS (all prior + new query tests).

```bash
git add crates/cull-core
git commit -m "feat(core): query_passes() pipeline + integration test"
```

---

## Self-Review

**1. Spec coverage:**
- §7 B1 program-slice/taint → Task 2 `RelevancePass` (deterministic symbol-overlap v1; full tree-sitter taint DAG is a flagged future upgrade). ✓
- Task signal extraction (latest message → query symbols) → Task 1 `TaskSignal::from_text`. ✓
- B2 PRF expansion and B3 embedding salience → explicitly deferred, with `RelevancePass` documented as the extension point. Not in scope.

**2. Placeholder scan:** No vague steps. The deferred B2/B3 are design notes, not in-plan placeholders. ✓

**3. Type consistency:** `TaskSignal`/`extract_symbols` (Task 1) used by `RelevancePass` (Task 2) and the pipeline test (Task 3). `PlanCtx.task` added Task 1, read by `RelevancePass`. `Planner::plan_with_task` (Task 1) called by Tasks 2–3 tests; `plan()` still works for Plan 2/3 callers (delegates with empty task). `Pass`/`PlanEntry`/`SegmentAction`/`DropReason` from Plan 2; `is_droppable_kind` is local to relevance.rs (mirrors dedup's eligibility but for the relevance kind set, which additionally includes `Diff`). ✓

**4. Ambiguity check:** Empty task → no drops (safety). Recency guard uses `max_pos - position < recency_keep` (keeps the newest `recency_keep` positions). Disjoint symbol sets → drop; any overlap → keep. Non-droppable kinds (conversation, reasoning, system, schema, compact-summary) are never dropped by relevance regardless of overlap or age. The planner's I1/I3 still apply on top. ✓

**Outcome:** Query-aware compression is live — the engine now drops *task-irrelevant* whole units, the core differentiator no shipping proxy has. Plan 5 adds the Replace-based passes (file-read IVM/delta, RePair dedup) together with the emitter that expands them, plus the eviction policies.
