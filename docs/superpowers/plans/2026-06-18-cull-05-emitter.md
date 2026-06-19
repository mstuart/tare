# Cull — Plan 5: Emitter + Fidelity Report Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the emitter — the component that applies a `CompressionPlan` to the segments to produce the actual compressed output (Keep → original bytes, Drop → omitted, Replace → rendered bytes), assembled in cache-stable order, together with the self-reporting `FidelityReport` (input/net tokens, ratio, per-segment decisions). This makes the engine produce real compressed context end-to-end for the first time.

**Architecture:** A new `cull-core::emit` module. `emit(segments, plan) -> (Vec<EmittedSegment>, FidelityReport)` applies each entry's action and orders the survivors by `stability_order` (Frozen → Slow → Fast, from Plan 2) so the cached prefix stays stable. The `FidelityReport` is the self-reporting surface from spec §11 — computed on whatever real workload the engine runs, which structurally beats curated-benchmark claims. The emitter uses the existing `SegmentAction` (Keep/Drop/Replace) unchanged; Replace's `bytes` field is the rendered form. The lossless-delta refinement of Replace (rendered vs reconstruct) lands in Plan 6 where the IVM/RePair passes need it.

**Tech Stack:** Rust. Builds on Plan 1–4 (`Segment`, `CompressionPlan`/`SegmentAction`/`DropReason`, `stability_order`, `input_tokens`, the pass pipelines). No new deps. Reference: spec §11 (self-reporting emitter), §8 Rule 3 (stability ordering).

---

## File Structure

```
crates/cull-core/src/emit.rs   # EmittedSegment, FidelityReport, emit()
crates/cull-core/src/lib.rs    # add `pub mod emit;` + re-exports
```

---

### Task 1: emit() + FidelityReport

**Files:** Create `crates/cull-core/src/emit.rs`; modify `crates/cull-core/src/lib.rs`; tests inline.

- [ ] **Step 1: Write the failing test**

Put in `crates/cull-core/src/emit.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use crate::plan::{CompressionPlan, PlanEntry, SegmentAction, DropReason};

    fn seg(id: u64, pos: usize, class: MutationClass, tok: u32, text: &str) -> Segment {
        Segment {
            id: SegmentId(id), kind: SegmentKind::FileRead, role: Role::Tool,
            bytes: text.as_bytes().to_vec(), token_count: tok, position: pos,
            mutation_class: class, origin: Origin::default(),
            protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn applies_keep_drop_replace_and_orders_by_stability() {
        // positions: fast(0), frozen(1), fast(2). Stability order => frozen(1), fast(0), fast(2).
        let segs = vec![
            seg(0, 0, MutationClass::Fast, 10, "AAAA"),
            seg(1, 1, MutationClass::Frozen, 5, "SYS"),
            seg(2, 2, MutationClass::Fast, 10, "CCCC"),
        ];
        let plan = CompressionPlan { entries: vec![
            PlanEntry { id: SegmentId(0), action: SegmentAction::Drop(DropReason::Superseded) },
            PlanEntry { id: SegmentId(1), action: SegmentAction::Keep },
            PlanEntry { id: SegmentId(2), action: SegmentAction::Replace { bytes: b"cc".to_vec(), token_count: 3, reason: DropReason::Duplicate } },
        ]};
        let (emitted, report) = emit(&segs, &plan);

        // order: frozen seg 1 first, then surviving fast seg 2 (seg 0 dropped)
        assert_eq!(emitted.len(), 2);
        assert_eq!(emitted[0].id, SegmentId(1));
        assert_eq!(emitted[0].bytes, b"SYS");          // kept original
        assert_eq!(emitted[1].id, SegmentId(2));
        assert_eq!(emitted[1].bytes, b"cc");           // replaced rendered
        assert_eq!(emitted[1].token_count, 3);

        assert_eq!(report.input_tokens, 25);           // 10+5+10
        assert_eq!(report.net_tokens, 8);              // kept 5 + replaced 3
        assert_eq!(report.kept, 1);
        assert_eq!(report.dropped, 1);
        assert_eq!(report.replaced, 1);
        assert_eq!(report.drops, vec![(SegmentId(0), DropReason::Superseded)]);
        assert!((report.ratio() - (8.0/25.0)).abs() < 1e-9);
    }

    #[test]
    fn missing_entry_defaults_to_keep() {
        let segs = vec![seg(0, 0, MutationClass::Fast, 10, "X")];
        let plan = CompressionPlan { entries: vec![] }; // no decision for seg 0
        let (emitted, report) = emit(&segs, &plan);
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].bytes, b"X");
        assert_eq!(report.net_tokens, 10);
        assert_eq!(report.kept, 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core emit::`
Expected: FAIL (`emit` / `EmittedSegment` / `FidelityReport` not defined).

- [ ] **Step 3: Implement the emitter**

Above the test module:
```rust
use std::collections::HashMap;
use crate::plan::{input_tokens, CompressionPlan, DropReason, SegmentAction};
use crate::planner::stability_order;
use crate::segment::{Segment, SegmentId};

/// One segment in the compressed output, in cache-stable order.
#[derive(Debug, Clone)]
pub struct EmittedSegment { pub id: SegmentId, pub bytes: Vec<u8>, pub token_count: u32 }

/// Self-reporting fidelity surface (spec §11): what the engine did, on the real workload.
#[derive(Debug, Clone)]
pub struct FidelityReport {
    pub input_tokens: u32,
    pub net_tokens: u32,
    pub kept: usize,
    pub dropped: usize,
    pub replaced: usize,
    pub drops: Vec<(SegmentId, DropReason)>,
}

impl FidelityReport {
    /// net / input (1.0 means no compression; lower is more compressed).
    pub fn ratio(&self) -> f64 {
        if self.input_tokens == 0 { 1.0 } else { self.net_tokens as f64 / self.input_tokens as f64 }
    }
}

/// Apply a plan to segments and assemble the compressed output in cache-stable order.
/// Keep → original bytes; Replace → rendered bytes; Drop → omitted. A segment with no plan
/// entry defaults to Keep (safe).
pub fn emit(segments: &[Segment], plan: &CompressionPlan) -> (Vec<EmittedSegment>, FidelityReport) {
    let action_of: HashMap<SegmentId, &SegmentAction> =
        plan.entries.iter().map(|e| (e.id, &e.action)).collect();
    let seg_of: HashMap<SegmentId, &Segment> = segments.iter().map(|s| (s.id, s)).collect();

    let mut emitted = Vec::new();
    let (mut kept, mut dropped, mut replaced, mut net) = (0usize, 0usize, 0usize, 0u32);
    let mut drops = Vec::new();

    for id in stability_order(segments) {
        let s = seg_of[&id];
        match action_of.get(&id) {
            None | Some(SegmentAction::Keep) => {
                emitted.push(EmittedSegment { id, bytes: s.bytes.clone(), token_count: s.token_count });
                kept += 1;
                net += s.token_count;
            }
            Some(SegmentAction::Drop(reason)) => {
                dropped += 1;
                drops.push((id, reason.clone()));
            }
            Some(SegmentAction::Replace { bytes, token_count, .. }) => {
                emitted.push(EmittedSegment { id, bytes: bytes.clone(), token_count: *token_count });
                replaced += 1;
                net += *token_count;
            }
        }
    }

    let report = FidelityReport {
        input_tokens: input_tokens(segments),
        net_tokens: net,
        kept, dropped, replaced, drops,
    };
    (emitted, report)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core emit::`
Expected: PASS (both).

- [ ] **Step 5: Wire + commit**

Add to `crates/cull-core/src/lib.rs`: `pub mod emit;` and `pub use emit::{emit, EmittedSegment, FidelityReport};`. Run `cargo build -p cull-core` → PASS.
```bash
git add crates/cull-core
git commit -m "feat(core): emitter (apply plan, cache-stable order) + self-reporting FidelityReport"
```

---

### Task 2: End-to-end pipeline integration test

**Files:** Create `crates/cull-core/tests/pipeline.rs` (an integration test crate-level file); no production changes.

- [ ] **Step 1: Write the failing test**

Create `crates/cull-core/tests/pipeline.rs`:
```rust
// End-to-end: segment a context, plan with the full pass set, emit, check the report.
use cull_core::segment::{Role, SegmentKind};
use cull_core::segmenter::{segment, RawBlock};
use cull_core::planner::Planner;
use cull_core::passes::{structural_passes, query_passes};
use cull_core::session::SessionState;
use cull_core::task::TaskSignal;
use cull_core::emit::emit;
use cull_tokenize::ApproxCounter;

fn raw(role: Role, kind: SegmentKind, text: &str) -> RawBlock {
    RawBlock { role, kind, text: text.to_string() }
}

#[test]
fn full_pipeline_compresses_superseded_and_irrelevant_content() {
    let counter = ApproxCounter::o200k();
    let blocks = vec![
        raw(Role::System, SegmentKind::SystemPrompt, "You are a coding agent working on authentication."),
        raw(Role::Tool, SegmentKind::ToolOutput { class: "cargo-test".into() }, "old test run: 3 failed"),
        raw(Role::Tool, SegmentKind::FileRead, "jwt authentication middleware verify token"),
        raw(Role::Tool, SegmentKind::ToolOutput { class: "grep".into() }, "kubernetes helm chart values yaml registry"),
        raw(Role::Tool, SegmentKind::ToolOutput { class: "cargo-test".into() }, "new test run: all passed"),
    ];
    let segs = segment(&blocks, &counter);

    // structural (supersession+dedup) + query (relevance) passes
    let mut passes = structural_passes();
    passes.extend(query_passes());
    let task = TaskSignal::from_text("authentication jwt middleware");

    let plan = Planner::new(passes).plan_with_task(&segs, &SessionState::default(), &task);
    let (emitted, report) = emit(&segs, &plan);

    // The old cargo-test output is superseded (dropped); the irrelevant kubernetes grep is dropped
    // if it falls outside the recency window. The system prompt and the relevant jwt read survive.
    assert!(report.net_tokens < report.input_tokens, "must compress: {} < {}", report.net_tokens, report.input_tokens);
    assert!(report.dropped >= 1, "at least the superseded test output is dropped");

    // The relevant jwt read must survive (its bytes appear in the emitted output).
    let survived: Vec<String> = emitted.iter().map(|e| String::from_utf8_lossy(&e.bytes).into_owned()).collect();
    assert!(survived.iter().any(|t| t.contains("jwt authentication middleware")), "relevant read must survive");
    // The system prompt (frozen) must always survive and be first (stability order).
    assert!(survived[0].contains("You are a coding agent"), "frozen system prompt first");
}
```

- [ ] **Step 2: Run test to verify it fails or passes**

Run: `cargo test -p cull-core --test pipeline`
Expected: It may FAIL to compile first (if `cull-tokenize` is not a dev-dependency of `cull-core` for integration tests). If so, add to `crates/cull-core/Cargo.toml` under `[dev-dependencies]`:
```toml
cull-tokenize = { path = "../cull-tokenize" }
```
(It is already a normal dependency, but integration tests need it resolvable; if the normal dep suffices, no change is needed.) Re-run. The assertions should then PASS given the implemented pipeline — this test validates integration, not new code.

- [ ] **Step 3: Make it pass**

If any assertion fails, do NOT weaken the assertion — diagnose. The most likely real issue is the recency guard keeping the kubernetes grep (position near the end). The test is written so the superseded cargo-test (position 1, well before the max position) is the guaranteed drop, and `dropped >= 1` tolerates the recency-kept grep. If the relevant-survives or frozen-first assertions fail, that is a real bug in the pipeline to fix in the relevant component.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p cull-core --test pipeline`
Expected: PASS.

- [ ] **Step 5: Full workspace test + commit**

Run: `cargo test --workspace`
Expected: PASS (all unit + the new integration test).

```bash
git add crates/cull-core
git commit -m "test(core): end-to-end segment->plan->emit pipeline integration test"
```

---

## Self-Review

**1. Spec coverage:**
- §11 self-reporting emitter / FidelityReport → Task 1. ✓
- §8 Rule 3 cache-stable ordering applied at emit time → Task 1 (`stability_order`). ✓
- End-to-end pipeline (segment → plan → emit) proving the parts compose → Task 2. ✓
- Replace lossless-delta refinement + the IVM/RePair passes that produce Replaces → Plan 6 (the emitter already renders any Replace's `bytes`, so it is ready for them). Not in scope here.

**2. Placeholder scan:** No vague steps. Task 2 Step 2/3 give explicit guidance on the one environmental wrinkle (dev-dependency resolution) and forbid weakening assertions. ✓

**3. Type consistency:** `emit`/`EmittedSegment`/`FidelityReport` (Task 1) used in Task 2's integration test. `CompressionPlan`/`SegmentAction`/`DropReason`/`input_tokens`/`stability_order` from Plans 2; `segment`/`RawBlock`/`Segment`/`SegmentKind` from Plan 1; `structural_passes`/`query_passes`/`plan_with_task`/`TaskSignal` from Plans 3–4; `ApproxCounter` from Plan 1's `cull-tokenize`. The Replace match arm uses the existing `{ bytes, token_count, .. }` shape. ✓

**4. Ambiguity check:** Missing plan entry → Keep (explicit). Ordering is by `stability_order` (Frozen, Slow, Fast, then position). `ratio()` guards divide-by-zero (input 0 → 1.0). The integration test asserts only robust invariants (compresses, ≥1 drop, relevant survives, frozen first) rather than brittle exact counts that depend on the recency window. ✓

**Outcome:** The engine now produces real compressed output with an honest, per-workload fidelity report. Plan 6 adds the Replace-producing passes (file-read IVM/delta via unified diff, RePair envelope dedup) with the lossless-Replace refinement, which the emitter already renders.
