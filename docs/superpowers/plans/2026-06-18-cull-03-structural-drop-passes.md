# Cull — Plan 3: Structural Drop Passes (Supersession + Dedup) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the first two real compression passes — **supersession decay** (drop superseded tool outputs) and **exact-content dedup** (drop byte-identical redundant segments) — as `Pass` implementations that plug into the `Planner` and are automatically held to invariants I1/I3/I4.

**Architecture:** Both passes are **Drop-only**: they omit whole semantic units, which needs no emitter expansion and is exempt from the I4 protected-token check (a dropped unit's tokens are intentionally gone — that is how supersession/dedup work; the planner's I3 still protects frozen segments). Passes live under a new `cull-core::passes` module, one file per pass, with a `structural_passes()` helper that returns the default Drop pipeline. Replace-based passes (delta/IVM, RePair) come later, paired with the emitter that expands them.

**Tech Stack:** Rust. Builds on Plan 2's `Pass`/`PlanCtx`/`Planner`/`SegmentAction`/`DropReason` and Plan 1's `Segment`/`SegmentKind`. No new external deps. Reference: spec §7 A1 (supersession), §9 (invariants — Drop is exempt from I4; I3 still applies).

---

## File Structure

```
crates/cull-core/src/passes/mod.rs            # `pub mod` + re-exports + structural_passes()
crates/cull-core/src/passes/supersession.rs   # SupersessionPass
crates/cull-core/src/passes/dedup.rs          # ExactDedupPass
crates/cull-core/src/lib.rs                    # add `pub mod passes;` + re-exports
```

Split rationale: each pass is an independent unit implementing the same `Pass` trait; one file per pass keeps them focused and lets later plans add passes without touching existing ones. `passes/mod.rs` owns the pipeline-assembly helper.

---

### Task 1: Supersession decay pass

**Files:** Create `crates/cull-core/src/passes/mod.rs` and `crates/cull-core/src/passes/supersession.rs`; modify `crates/cull-core/src/lib.rs`; test inline in `supersession.rs`.

- [ ] **Step 1: Create the module wiring**

Create `crates/cull-core/src/passes/mod.rs`:
```rust
pub mod supersession;

pub use supersession::SupersessionPass;
```

Add to `crates/cull-core/src/lib.rs` (after the existing `pub mod` lines):
```rust
pub mod passes;
```

- [ ] **Step 2: Write the failing test**

Put in `crates/cull-core/src/passes/supersession.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use crate::plan::{SegmentAction, DropReason};
    use crate::planner::Planner;
    use crate::session::SessionState;

    fn tool_seg(id: u64, class: &str) -> Segment {
        Segment {
            id: SegmentId(id), kind: SegmentKind::ToolOutput { class: class.into() },
            role: Role::Tool, bytes: format!("output {id}").into_bytes(), token_count: 10,
            position: id as usize, mutation_class: MutationClass::Fast, origin: Origin::default(),
            protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn drops_all_but_latest_per_class() {
        // cargo-test at 0,2,4 ; git-status at 1 ; only the latest cargo-test (4) survives
        let segs = vec![
            tool_seg(0, "cargo-test"),
            tool_seg(1, "git-status"),
            tool_seg(2, "cargo-test"),
            tool_seg(4, "cargo-test"),
        ];
        let plan = Planner::new(vec![Box::new(SupersessionPass)]).plan(&segs, &SessionState::default());
        // entries are in segment order: ids 0,1,2,4
        assert_eq!(plan.entries[0].action, SegmentAction::Drop(DropReason::Superseded)); // cargo-test 0
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);                          // git-status (only one)
        assert_eq!(plan.entries[2].action, SegmentAction::Drop(DropReason::Superseded)); // cargo-test 2
        assert_eq!(plan.entries[3].action, SegmentAction::Keep);                          // cargo-test 4 (latest)
    }

    #[test]
    fn single_output_is_kept() {
        let segs = vec![tool_seg(0, "cargo-test")];
        let plan = Planner::new(vec![Box::new(SupersessionPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn non_tooloutput_segments_are_ignored() {
        let mut s = tool_seg(0, "x");
        s.kind = SegmentKind::ConversationTurn;
        let segs = vec![s, tool_seg(1, "x")]; // different kinds; the ConversationTurn is not a ToolOutput
        let plan = Planner::new(vec![Box::new(SupersessionPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep); // conversation turn untouched
        assert_eq!(plan.entries[1].action, SegmentAction::Keep); // only one ToolOutput of class "x"
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p cull-core supersession`
Expected: FAIL (`SupersessionPass` not defined).

- [ ] **Step 4: Implement the pass**

Above the test module in `supersession.rs`:
```rust
use std::collections::HashMap;
use crate::plan::{DropReason, PlanEntry, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::SegmentKind;

/// Drop superseded tool outputs (spec §7 A1): for each ToolOutput class, every occurrence
/// except the latest (highest `position`) is dropped — an old build/test/lint/status run is
/// obsoleted by a newer run of the same class. Whole-unit Drop (exempt from I4); the planner's
/// I3 still protects any frozen segment.
pub struct SupersessionPass;

impl Pass for SupersessionPass {
    fn name(&self) -> &'static str { "supersession-decay" }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        // latest position per class
        let mut latest: HashMap<&str, usize> = HashMap::new();
        for s in ctx.segments {
            if let SegmentKind::ToolOutput { class } = &s.kind {
                let e = latest.entry(class.as_str()).or_insert(s.position);
                if s.position > *e { *e = s.position; }
            }
        }
        // drop earlier same-class outputs
        ctx.segments.iter().filter_map(|s| {
            if let SegmentKind::ToolOutput { class } = &s.kind {
                if s.position < latest[class.as_str()] {
                    return Some(PlanEntry { id: s.id, action: SegmentAction::Drop(DropReason::Superseded) });
                }
            }
            None
        }).collect()
    }
}
```

- [ ] **Step 5: Run test to verify it passes, then commit**

Run: `cargo test -p cull-core supersession`
Expected: PASS (all three).

Add `pub use passes::SupersessionPass;` to `crates/cull-core/src/lib.rs` re-export block. Run `cargo build -p cull-core` → PASS.
```bash
git add crates/cull-core
git commit -m "feat(core): supersession-decay drop pass"
```

---

### Task 2: Exact-content dedup pass

**Files:** Create `crates/cull-core/src/passes/dedup.rs`; modify `crates/cull-core/src/passes/mod.rs`; test inline.

- [ ] **Step 1: Write the failing test**

Put in `crates/cull-core/src/passes/dedup.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use crate::plan::{SegmentAction, DropReason};
    use crate::planner::Planner;
    use crate::session::SessionState;

    fn data_seg(id: u64, kind: SegmentKind, text: &str) -> Segment {
        Segment {
            id: SegmentId(id), kind, role: Role::Tool, bytes: text.as_bytes().to_vec(),
            token_count: 10, position: id as usize, mutation_class: MutationClass::Fast,
            origin: Origin::default(), protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn drops_later_byte_identical_data_segment() {
        let segs = vec![
            data_seg(0, SegmentKind::FileRead, "same contents"),
            data_seg(1, SegmentKind::FileRead, "different"),
            data_seg(2, SegmentKind::FileRead, "same contents"), // exact dup of id 0
        ];
        let plan = Planner::new(vec![Box::new(ExactDedupPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);                       // first occurrence kept
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);
        assert_eq!(plan.entries[2].action, SegmentAction::Drop(DropReason::Duplicate)); // later dup dropped
    }

    #[test]
    fn does_not_dedup_non_data_kinds() {
        // two identical conversation turns are NOT dedup-eligible
        let segs = vec![
            data_seg(0, SegmentKind::ConversationTurn, "hello"),
            data_seg(1, SegmentKind::ConversationTurn, "hello"),
        ];
        let plan = Planner::new(vec![Box::new(ExactDedupPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core dedup`
Expected: FAIL (`ExactDedupPass` not defined).

- [ ] **Step 3: Implement the pass**

Above the test module in `dedup.rs`:
```rust
use std::collections::HashSet;
use crate::plan::{DropReason, PlanEntry, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::SegmentKind;

/// Drop later byte-identical copies of data segments (re-read unchanged file, repeated grep,
/// duplicate directory listing). The first occurrence is kept; later exact duplicates are
/// dropped (the content is still present once). Scoped to "data" kinds — never conversation,
/// reasoning, system prompt, or schemas. Whole-unit Drop (I4-exempt); I3 still protects frozen.
pub struct ExactDedupPass;

fn is_dedup_eligible(kind: &SegmentKind) -> bool {
    matches!(
        kind,
        SegmentKind::FileRead
            | SegmentKind::DirListing
            | SegmentKind::ToolOutput { .. }
            | SegmentKind::StackTrace
            | SegmentKind::TestOutput
    )
}

impl Pass for ExactDedupPass {
    fn name(&self) -> &'static str { "exact-dedup" }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        let mut seen: HashSet<&[u8]> = HashSet::new();
        let mut out = Vec::new();
        for s in ctx.segments {
            if !is_dedup_eligible(&s.kind) { continue; }
            if seen.contains(s.bytes.as_slice()) {
                out.push(PlanEntry { id: s.id, action: SegmentAction::Drop(DropReason::Duplicate) });
            } else {
                seen.insert(s.bytes.as_slice());
            }
        }
        out
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core dedup`
Expected: PASS (both).

- [ ] **Step 5: Wire + commit**

Add to `crates/cull-core/src/passes/mod.rs`:
```rust
pub mod dedup;
pub use dedup::ExactDedupPass;
```
Add `pub use passes::ExactDedupPass;` to `crates/cull-core/src/lib.rs` re-exports. Run `cargo build -p cull-core` → PASS.
```bash
git add crates/cull-core
git commit -m "feat(core): exact-content dedup drop pass"
```

---

### Task 3: Structural pass pipeline + integration test

**Files:** Modify `crates/cull-core/src/passes/mod.rs`; test inline in `mod.rs`.

- [ ] **Step 1: Write the failing test**

Add to `crates/cull-core/src/passes/mod.rs`:
```rust
#[cfg(test)]
mod tests {
    use crate::segment::*;
    use crate::plan::{SegmentAction, net_tokens, input_tokens};
    use crate::planner::Planner;
    use crate::session::SessionState;

    fn seg(id: u64, kind: SegmentKind, class_or_text: &str) -> Segment {
        Segment {
            id: SegmentId(id), kind, role: Role::Tool, bytes: class_or_text.as_bytes().to_vec(),
            token_count: 10, position: id as usize, mutation_class: MutationClass::Fast,
            origin: Origin::default(), protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn structural_pipeline_compresses_and_never_increases() {
        // two superseded builds + one exact-duplicate file read
        let segs = vec![
            seg(0, SegmentKind::ToolOutput { class: "cargo-test".into() }, "build-old"),
            seg(1, SegmentKind::FileRead, "FILEDATA"),
            seg(2, SegmentKind::ToolOutput { class: "cargo-test".into() }, "build-new"),
            seg(3, SegmentKind::FileRead, "FILEDATA"), // exact dup of id 1
        ];
        let before = input_tokens(&segs); // 40
        let plan = Planner::new(super::structural_passes()).plan(&segs, &SessionState::default());
        let after = net_tokens(&plan, &segs);
        assert!(after < before, "pipeline must reduce tokens: {after} < {before}");
        // id 0 superseded, id 3 deduped; ids 1 and 2 kept => 20 tokens
        assert_eq!(after, 20);
        assert_eq!(plan.entries[0].action, SegmentAction::Drop(crate::plan::DropReason::Superseded));
        assert_eq!(plan.entries[3].action, SegmentAction::Drop(crate::plan::DropReason::Duplicate));
    }

    #[test]
    fn structural_passes_returns_both_passes() {
        let passes = super::structural_passes();
        assert_eq!(passes.len(), 2);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core passes::tests`
Expected: FAIL (`structural_passes` not defined).

- [ ] **Step 3: Implement the helper**

Add to `crates/cull-core/src/passes/mod.rs` (above the test module, below the `pub mod`/`pub use` lines):
```rust
use crate::planner::Pass;

/// The default Drop-based structural pass pipeline, in run order. Supersession first (drops
/// obsolete tool outputs), then exact dedup (drops identical leftovers). Replace-based passes
/// (delta/IVM, RePair) are added in a later plan alongside the emitter.
pub fn structural_passes() -> Vec<Box<dyn Pass>> {
    vec![Box::new(SupersessionPass), Box::new(ExactDedupPass)]
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core passes::tests`
Expected: PASS (both).

- [ ] **Step 5: Full workspace test + commit**

Run: `cargo test --workspace`
Expected: PASS (all prior tests + the new pass tests).

```bash
git add crates/cull-core
git commit -m "feat(core): structural_passes() pipeline + integration test"
```

---

## Self-Review

**1. Spec coverage:**
- §7 A1 supersession decay → Task 1. ✓
- Exact-content dedup (the lossless Drop subset of A4/near-duplicate handling; full CDC delta is a later Replace plan) → Task 2. ✓
- Pipeline assembly → Task 3 `structural_passes()`. ✓
- Replace-based A2 (IVM/delta) and A3 (RePair) are deferred to the emitter-paired plan — they require the lossless-Replace model + expansion, called out in this plan's Architecture. Not in scope here.

**2. Placeholder scan:** No "TBD"/vague steps; every pass has complete code and tests. ✓

**3. Type consistency:** `Pass`/`PlanCtx`/`PlanEntry`/`SegmentAction`/`DropReason` from Plan 2; `Planner::new`/`plan`, `net_tokens`/`input_tokens` from Plan 2; `Segment`/`SegmentKind`/`SegmentId` from Plan 1. `SupersessionPass` (Task 1) and `ExactDedupPass` (Task 2) both implement `Pass` and are composed by `structural_passes()` (Task 3). The integration test (Task 3) confirms both run together and the planner's I1 holds (after < before). ✓

**4. Ambiguity check:** Supersession keys on the `ToolOutput` class string and keeps the highest `position` per class (deterministic). Dedup keeps the first occurrence (lowest position, since iteration is in segment order) and is scoped to data kinds via `is_dedup_eligible`. Both only ever propose `Drop`; the planner's invariant pass guarantees frozen segments and net-non-negativity regardless. ✓

**Outcome:** Two real, invariant-safe compression passes that measurably reduce tokens on superseded/duplicate content. Plan 4 adds the query-conditioned Drop passes (task signal, program-slice, PRF, embedding) that drop *irrelevant* whole segments — same Drop mechanism, relevance-driven.
