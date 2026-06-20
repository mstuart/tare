# Cull — Plan 15: Delta-Base Protection + Envelope Dedup (A3)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** (1) Fix a latent correctness bug: a delta `Replace` whose base segment is later dropped becomes unreconstructable — protect delta bases. (2) Close ledger A3 — lossless dedup of repetitive tool-output **envelopes** (repeated JSON wrappers, stack-frame prefixes) by delta-encoding a tool output against the most-similar earlier one. This reuses the verified `Reconstruct::Delta` model rather than a separate RePair grammar engine — same goal (lossless envelope dedup), simpler mechanism that the planner already verifies.

**Architecture:** Task 1 adds a fixpoint pass in `enforce_invariants`: any `Replace { Delta { base } }` whose `base` is not `Keep` in the final plan reverts to `Keep` (reverting only adds Keeps → converges). Task 2 adds `EnvelopeDedupPass`: for each `ToolOutput` segment, it tries diffing against each earlier `ToolOutput`; if the smallest diff is materially smaller than the original, it proposes a `Delta` Replace against that base. The planner verifies losslessness and (now) base-protection.

**Tech:** Rust, `diffy`. Builds on Plans 2/7/8/13. Reference: spec §7 A3, §9 I5.

---

### Task 1: Delta-base protection in the planner

**Files:** `crates/cull-core/src/planner.rs`; tests inline.

- [ ] **Step 1 — failing test.**
```rust
    use crate::plan::Reconstruct;

    // pass: drop seg0 AND delta seg1 against seg0 — seg1's base is gone, so it must revert to Keep.
    struct DropBaseAndDelta;
    impl Pass for DropBaseAndDelta {
        fn name(&self) -> &'static str { "drop-base-and-delta" }
        fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
            let s0 = &ctx.segments[0]; let s1 = &ctx.segments[1];
            let patch = diffy::create_patch(
                std::str::from_utf8(&s0.bytes).unwrap(), std::str::from_utf8(&s1.bytes).unwrap()).to_string();
            vec![
                PlanEntry { id: s0.id, action: SegmentAction::Drop(DropReason::Evicted) },
                PlanEntry { id: s1.id, action: SegmentAction::Replace {
                    rendered: patch.into_bytes(), token_count: 1,
                    reconstruct: Reconstruct::Delta { base: s0.id }, reason: DropReason::Duplicate } },
            ]
        }
    }

    #[test]
    fn delta_against_dropped_base_reverts_to_keep() {
        let mut a = seg(0, MutationClass::Fast); a.bytes = b"alpha\nbeta\ngamma\n".to_vec();
        let mut b = seg(1, MutationClass::Fast); b.bytes = b"alpha\nBETA\ngamma\n".to_vec();
        let plan = Planner::new(vec![Box::new(DropBaseAndDelta)]).plan(&[a.clone(), b.clone()], &SessionState::default());
        // base seg0 dropped -> seg1's delta has no base -> reverted to Keep (reconstructable)
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);
    }
```

- [ ] **Step 2 — confirm FAIL** (currently seg1 stays a Replace against a dropped base).

- [ ] **Step 3 — implement.** At the END of `enforce_invariants` (after the existing per-entry I1/I3/I4 loop), add base-protection to a fixpoint:
```rust
    // Base protection: a Delta's base must be Keep in the final plan, else it's unreconstructable.
    loop {
        let kept: std::collections::HashSet<SegmentId> = segments.iter().zip(actions.iter())
            .filter(|(_, a)| matches!(a, SegmentAction::Keep))
            .map(|(s, _)| s.id).collect();
        let mut changed = false;
        for (a, _s) in actions.iter_mut().zip(segments.iter()) {
            if let SegmentAction::Replace { reconstruct: crate::plan::Reconstruct::Delta { base }, .. } = a {
                if !kept.contains(base) { *a = SegmentAction::Keep; changed = true; }
            }
        }
        if !changed { break; }
    }
```

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-core planner::`). All prior planner tests still pass.

- [ ] **Step 5 — commit.** `git add crates/cull-core && git commit -m "fix(core): protect delta bases — a Replace whose Delta base isn't kept reverts to Keep"`

---

### Task 2: EnvelopeDedupPass (A3)

**Files:** `crates/cull-core/src/passes/envelope.rs`; modify `passes/mod.rs`; tests inline.

- [ ] **Step 1 — failing test.** In `passes/envelope.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use crate::plan::SegmentAction;
    use crate::planner::Planner;
    use crate::session::SessionState;

    fn out(id: u64, pos: usize, text: &str) -> Segment {
        Segment { id: SegmentId(id), kind: SegmentKind::ToolOutput { class: "grep".into() },
            role: Role::Tool, bytes: text.as_bytes().to_vec(), token_count: 60, position: pos,
            mutation_class: MutationClass::Fast, origin: Origin::default(),
            protected_spans: vec![], refs: RefLedger::default() }
    }

    #[test]
    fn repetitive_envelope_is_deltad_against_earlier_output() {
        // same JSON envelope, one differing field — the second becomes a small delta
        let a = out(0, 0, "{\"tool\":\"grep\",\"version\":1,\"results\":[\"a\",\"b\",\"c\"],\"meta\":{\"ms\":12}}");
        let b = out(1, 1, "{\"tool\":\"grep\",\"version\":1,\"results\":[\"a\",\"b\",\"X\"],\"meta\":{\"ms\":12}}");
        let plan = Planner::new(vec![Box::new(EnvelopeDedupPass::new())]).plan(&[a.clone(), b.clone()], &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert!(matches!(plan.entries[1].action, SegmentAction::Replace { .. }), "near-duplicate envelope deltad");
    }

    #[test]
    fn dissimilar_outputs_are_not_deltad() {
        let a = out(0, 0, "completely different alpha content here and there everywhere");
        let b = out(1, 1, "nothing in common beta gamma delta epsilon zeta eta theta");
        let plan = Planner::new(vec![Box::new(EnvelopeDedupPass::new())]).plan(&[a, b], &SessionState::default());
        assert_eq!(plan.entries[1].action, SegmentAction::Keep, "no shared envelope -> no delta");
    }
}
```

- [ ] **Step 2 — confirm FAIL** (`EnvelopeDedupPass` undefined).

- [ ] **Step 3 — implement.** Above the tests:
```rust
use crate::plan::{DropReason, PlanEntry, Reconstruct, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::SegmentKind;
use cull_tokenize::{ApproxCounter, TokenCounter};

/// Lossless dedup of repetitive tool-output envelopes (spec §7 A3). For each ToolOutput, delta
/// against the most-similar EARLIER ToolOutput when that diff is materially smaller than the
/// original. Reuses Reconstruct::Delta (planner verifies losslessness + base protection).
pub struct EnvelopeDedupPass { counter: ApproxCounter, min_reduction: f64 }

impl EnvelopeDedupPass {
    pub fn new() -> Self { Self { counter: ApproxCounter::o200k(), min_reduction: 0.6 } }
}
impl Default for EnvelopeDedupPass { fn default() -> Self { Self::new() } }

impl Pass for EnvelopeDedupPass {
    fn name(&self) -> &'static str { "envelope-dedup" }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        let outs: Vec<_> = ctx.segments.iter()
            .filter(|s| matches!(s.kind, SegmentKind::ToolOutput { .. }))
            .collect();
        let mut proposals = Vec::new();
        for (idx, s) in outs.iter().enumerate() {
            let Ok(s_str) = std::str::from_utf8(&s.bytes) else { continue; };
            let mut best: Option<(crate::segment::SegmentId, String, u32)> = None;
            for base in &outs[..idx] {
                let Ok(b_str) = std::str::from_utf8(&base.bytes) else { continue; };
                let patch = diffy::create_patch(b_str, s_str).to_string();
                let pt = self.counter.count(&patch) as u32;
                if best.as_ref().map_or(true, |(_, _, bp)| pt < *bp) {
                    best = Some((base.id, patch, pt));
                }
            }
            if let Some((base, patch, pt)) = best {
                if (pt as f64) < self.min_reduction * (s.token_count as f64) {
                    proposals.push(PlanEntry { id: s.id, action: SegmentAction::Replace {
                        rendered: patch.into_bytes(), token_count: pt,
                        reconstruct: Reconstruct::Delta { base }, reason: DropReason::Duplicate } });
                }
            }
        }
        proposals
    }
}
```

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-core envelope`).

- [ ] **Step 5 — wire + commit.** Add to `passes/mod.rs`: `pub mod envelope; pub use envelope::EnvelopeDedupPass;` and insert into `structural_passes()` AFTER ivm, BEFORE dedup:
```rust
    vec![Box::new(SupersessionPass), Box::new(IvmDeltaPass::new()),
         Box::new(EnvelopeDedupPass::new()), Box::new(ExactDedupPass)]
```
Add `pub use passes::EnvelopeDedupPass;` to `lib.rs`. `cargo test --workspace` green, then `git add crates/cull-core && git commit -m "feat(core): envelope-dedup pass (A3) — lossless delta of repetitive tool-output envelopes"`

---

## After this plan — ledger
- ✅ §7 A3 — implemented as content-similarity delta (lossless; same goal as RePair grammar, simpler, model-verified). Note the substitution honestly.
- ✅ (bug fix) delta-base protection — strengthens I5 across IVM + envelope-dedup.

## Self-Review
- Base-protection fixpoint only adds Keeps → terminates in ≤ n iterations; doesn't affect plans without Delta. ✓
- EnvelopeDedup is O(n²) diffs over ToolOutput segments only (modest counts); each delta is planner-verified lossless + base-protected. ✓
- `structural_passes` now length 4 — update the `structural_passes_has_*` count test if present. ✓
