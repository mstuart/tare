# Cull — Plan 7: Lossless Replace Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refine `SegmentAction::Replace` into a **verifiable lossless** model so the upcoming file-read IVM/delta pass works. A `Replace` carries a `Reconstruct::Delta { base }`, and the planner verifies — at plan time — that applying the rendered diff to the base segment reproduces the original byte-for-byte. This replaces the old byte-subslice I4 check (which wrongly rejects valid deltas, since a diff omits unchanged content).

**Architecture:** `SegmentAction::Replace { rendered, token_count, reconstruct, reason }` where `rendered` is what gets sent (a unified diff) and `reconstruct` says how to recover the original (`Delta { base }`). A new `replace_is_lossless(original, action, by_id)` applies the diff to `base`'s bytes and checks equality with the original. The planner builds an id→segment map and enforces, per Replace: **I1** (`token_count < original`), **I4/I5** (`replace_is_lossless`), reverting to `Keep` on any violation. **I3** (frozen) is unchanged. The emitter renders `rendered`. This isolates the model change from the IVM pass (Plan 8) that produces these Replaces.

**Tech Stack:** Rust, `diffy` (unified diff create/apply). Builds on Plans 1–6. Reference: spec §9 (I1–I5), §7 A2 (IVM/delta — consumer of this model, next plan).

> **NOTE — this refactors merged code** (`plan.rs`, `planner.rs`, `emit.rs` and their tests). The old `Replace { bytes, token_count, reason }` and `replace_preserves_protected` are removed. Every existing `Replace { ... }` construction in tests is updated to the new shape as specified below. After this plan, `cargo test --workspace` must be fully green.

---

## File Structure

```
crates/cull-core/Cargo.toml      # add diffy
crates/cull-core/src/plan.rs     # Reconstruct, new Replace shape, apply_unified_diff, replace_is_lossless
crates/cull-core/src/planner.rs  # enforce_invariants uses replace_is_lossless; update test passes
crates/cull-core/src/emit.rs     # Replace match uses `rendered`; update tests; round-trip test
```

---

### Task 1: Reconstruct + lossless verifier (`plan.rs`)

**Files:** Modify `crates/cull-core/Cargo.toml` and `crates/cull-core/src/plan.rs`; tests inline.

- [ ] **Step 1: Add the diff dependency**

In `crates/cull-core/Cargo.toml` `[dependencies]`, add:
```toml
diffy = "0.4"
```
(If `0.4` does not resolve, use the latest `0.x` that provides `Patch::from_str` and `apply`; note the version used.)

- [ ] **Step 2: Write the failing test**

Replace the existing `i4_validator_rejects_replace_that_drops_a_protected_token` test in `plan.rs` (and add the round-trip test) so the test module's relevant tests read:
```rust
    #[test]
    fn apply_unified_diff_round_trips() {
        let base = b"line one\nline two\nline three\n";
        let modified = b"line one\nline TWO changed\nline three\n";
        let patch = diffy::create_patch(
            std::str::from_utf8(base).unwrap(),
            std::str::from_utf8(modified).unwrap(),
        ).to_string();
        let recovered = apply_unified_diff(base, patch.as_bytes()).unwrap();
        assert_eq!(recovered, modified);
    }

    #[test]
    fn replace_is_lossless_accepts_valid_delta_rejects_bad_one() {
        use std::collections::HashMap;
        let base = mk_seg(0, "alpha\nbeta\ngamma\n");
        let target = mk_seg(1, "alpha\nBETA-X\ngamma\n");
        let by_id: HashMap<SegmentId, &Segment> = [(base.id, &base), (target.id, &target)].into_iter().collect();

        let good_patch = diffy::create_patch("alpha\nbeta\ngamma\n", "alpha\nBETA-X\ngamma\n").to_string();
        let good = SegmentAction::Replace {
            rendered: good_patch.into_bytes(), token_count: 2,
            reconstruct: Reconstruct::Delta { base: SegmentId(0) }, reason: DropReason::Duplicate,
        };
        assert!(replace_is_lossless(&target, &good, &by_id));

        // wrong diff (against base but produces different text) -> not lossless
        let bad_patch = diffy::create_patch("alpha\nbeta\ngamma\n", "totally different\n").to_string();
        let bad = SegmentAction::Replace {
            rendered: bad_patch.into_bytes(), token_count: 1,
            reconstruct: Reconstruct::Delta { base: SegmentId(0) }, reason: DropReason::Duplicate,
        };
        assert!(!replace_is_lossless(&target, &bad, &by_id));

        // missing base -> not lossless
        let orphan = SegmentAction::Replace {
            rendered: b"@@".to_vec(), token_count: 1,
            reconstruct: Reconstruct::Delta { base: SegmentId(99) }, reason: DropReason::Duplicate,
        };
        assert!(!replace_is_lossless(&target, &orphan, &by_id));
    }
```
Add this helper to the `plan.rs` test module (used by the test above):
```rust
    fn mk_seg(id: u64, text: &str) -> Segment {
        use crate::segment::*;
        Segment {
            id: SegmentId(id), kind: SegmentKind::FileRead, role: Role::Tool,
            bytes: text.as_bytes().to_vec(), token_count: 10, position: id as usize,
            mutation_class: MutationClass::Fast, origin: Origin::default(),
            protected_spans: vec![], refs: RefLedger::default(),
        }
    }
```
Keep the existing `net_tokens_accounts_keep_drop_replace` test but update its `Drop` entry stays the same (Drop is unchanged); it does not construct a Replace, so no change needed there.

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p cull-core plan::`
Expected: FAIL (`Reconstruct`, `apply_unified_diff`, `replace_is_lossless`, new `Replace` fields not defined).

- [ ] **Step 4: Implement the model**

In `plan.rs`: change the `SegmentAction` enum and add the new items. The new `SegmentAction`:
```rust
/// How a Replace's `rendered` bytes recover the original. Currently delta-against-a-base-segment;
/// other reversible encodings (e.g. dictionary dedup) add variants here later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconstruct { Delta { base: SegmentId } }

/// Drop removes a WHOLE unit (allowed). Replace substitutes a LOSSLESS smaller representation:
/// `rendered` (a unified diff) is what gets sent; `reconstruct` says how to recover the exact
/// original. Losslessness is verified at plan time (spec I2/I4/I5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentAction {
    Keep,
    Drop(DropReason),
    Replace { rendered: Vec<u8>, token_count: u32, reconstruct: Reconstruct, reason: DropReason },
}
```
Update `net_tokens` (the Replace arm now reads `token_count` the same way):
```rust
        SegmentAction::Replace { token_count, .. } => *token_count,
```
Remove the old `replace_preserves_protected` and `contains_subslice`. Add:
```rust
use std::collections::HashMap;

/// Apply a unified diff (`patch`) to `base`, returning the reconstructed bytes, or None if the
/// inputs are not valid UTF-8 or the patch does not apply.
pub fn apply_unified_diff(base: &[u8], patch: &[u8]) -> Option<Vec<u8>> {
    let base = std::str::from_utf8(base).ok()?;
    let patch = std::str::from_utf8(patch).ok()?;
    let p = diffy::Patch::from_str(patch).ok()?;
    diffy::apply(base, &p).ok().map(String::into_bytes)
}

/// Verify a Replace losslessly reconstructs the original (spec I2/I4/I5). Drop/Keep are exempt.
pub fn replace_is_lossless(
    original: &Segment,
    action: &SegmentAction,
    by_id: &HashMap<SegmentId, &Segment>,
) -> bool {
    match action {
        SegmentAction::Replace { rendered, reconstruct, .. } => match reconstruct {
            Reconstruct::Delta { base } => match by_id.get(base) {
                Some(base_seg) => apply_unified_diff(&base_seg.bytes, rendered)
                    .map_or(false, |r| r == original.bytes),
                None => false,
            },
        },
        _ => true,
    }
}
```
Update the `plan.rs` lib re-exports if `Reconstruct` should be public at the module root (it is `pub`; add `Reconstruct` and `replace_is_lossless`/`apply_unified_diff` to `crate::lib` re-exports if you want them at crate root — at minimum keep them `pub` in `plan`). Add `pub use plan::Reconstruct;` to `crates/cull-core/src/lib.rs` re-export block.

- [ ] **Step 5: Run test + commit**

Run: `cargo test -p cull-core plan::` → PASS.
```bash
git add crates/cull-core
git commit -m "feat(core): verifiable lossless Replace model (Reconstruct::Delta + replace_is_lossless)"
```

---

### Task 2: Enforce losslessness in the planner (`planner.rs`)

**Files:** Modify `crates/cull-core/src/planner.rs`; update its test module.

- [ ] **Step 1: Update the test passes + tests**

In the `planner.rs` test module: the old `BloatPass`, `StripProtectedPass` and the `i1_*`/`i4_*` tests construct the OLD `Replace` shape. Replace them with:
```rust
    use crate::plan::Reconstruct;

    // Replace that increases tokens (violates I1) — uses a real valid delta so only I1 fails.
    struct BloatPass;
    impl Pass for BloatPass {
        fn name(&self) -> &'static str { "bloat" }
        fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
            ctx.segments.iter().map(|s| {
                let patch = diffy::create_patch(
                    std::str::from_utf8(&s.bytes).unwrap_or(""),
                    "anything",
                ).to_string();
                PlanEntry { id: s.id, action: SegmentAction::Replace {
                    rendered: patch.into_bytes(), token_count: s.token_count + 50,
                    reconstruct: Reconstruct::Delta { base: s.id }, reason: DropReason::Duplicate } }
            }).collect()
        }
    }

    // Replace whose delta does NOT reconstruct the original (violates I4/I5).
    struct LossyPass;
    impl Pass for LossyPass {
        fn name(&self) -> &'static str { "lossy" }
        fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
            ctx.segments.iter().map(|s| PlanEntry { id: s.id, action: SegmentAction::Replace {
                rendered: b"not a valid patch".to_vec(), token_count: 1,
                reconstruct: Reconstruct::Delta { base: SegmentId(999) }, reason: DropReason::Duplicate } }).collect()
        }
    }

    struct DropFrozenPass;
    impl Pass for DropFrozenPass {
        fn name(&self) -> &'static str { "drop-frozen" }
        fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
            ctx.segments.iter().map(|s| PlanEntry { id: s.id, action: SegmentAction::Drop(DropReason::Evicted) }).collect()
        }
    }

    #[test]
    fn i1_reverts_token_increasing_replace_to_keep() {
        let segs = vec![seg(0, MutationClass::Fast)];
        let plan = Planner::new(vec![Box::new(BloatPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(crate::plan::net_tokens(&plan, &segs), 10);
    }

    #[test]
    fn lossy_replace_is_reverted_to_keep() {
        let segs = vec![seg(0, MutationClass::Fast)];
        let plan = Planner::new(vec![Box::new(LossyPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn valid_delta_replace_is_kept() {
        // seg 0 is the base/original (token_count 10); a valid self-delta that reduces tokens survives.
        let mut s = seg(0, MutationClass::Fast);
        s.bytes = b"alpha\nbeta\ngamma\n".to_vec();
        let patch = diffy::create_patch("alpha\nbeta\ngamma\n", "alpha\nB\ngamma\n").to_string();
        struct ValidDelta { patch: String }
        impl Pass for ValidDelta {
            fn name(&self) -> &'static str { "valid-delta" }
            fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
                let s = &ctx.segments[0];
                vec![PlanEntry { id: s.id, action: SegmentAction::Replace {
                    rendered: self.patch.clone().into_bytes(), token_count: 1,
                    reconstruct: crate::plan::Reconstruct::Delta { base: s.id }, reason: DropReason::Duplicate } }]
            }
        }
        let plan = Planner::new(vec![Box::new(ValidDelta { patch })]).plan(&[s.clone()], &SessionState::default());
        assert!(matches!(plan.entries[0].action, SegmentAction::Replace { .. }), "valid lossless reducing delta is kept");
    }

    #[test]
    fn i3_reverts_drop_of_frozen_segment_to_keep() {
        let segs = vec![seg(0, MutationClass::Frozen), seg(1, MutationClass::Fast)];
        let plan = Planner::new(vec![Box::new(DropFrozenPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(plan.entries[1].action, SegmentAction::Drop(DropReason::Evicted));
    }
```
(Remove the old `StripProtectedPass`, `i4_reverts_replace_that_strips_protected_token_to_keep`, and the old `BloatPass`/`i1` versions and any `use crate::protected::...` no longer needed.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cull-core planner::`
Expected: FAIL (`enforce_invariants` still uses the removed `replace_preserves_protected`; `valid_delta`/`lossy` behavior not yet enforced).

- [ ] **Step 3: Update `enforce_invariants`**

Replace `enforce_invariants` in `planner.rs` with:
```rust
/// Enforce plan invariants (spec §9): I3 frozen=Keep; I4/I5 a Replace must losslessly reconstruct
/// the original; I1 a Replace must strictly reduce tokens. Any violation reverts that entry to Keep.
fn enforce_invariants(actions: &mut [SegmentAction], segments: &[Segment]) {
    let by_id: std::collections::HashMap<SegmentId, &Segment> =
        segments.iter().map(|s| (s.id, s)).collect();
    for (a, s) in actions.iter_mut().zip(segments.iter()) {
        if s.mutation_class == MutationClass::Frozen && *a != SegmentAction::Keep {
            *a = SegmentAction::Keep;
            continue;
        }
        if let SegmentAction::Replace { token_count, .. } = a {
            let reduces = *token_count < s.token_count;
            let lossless = crate::plan::replace_is_lossless(s, a, &by_id);
            if !reduces || !lossless { *a = SegmentAction::Keep; }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cull-core planner::`
Expected: PASS (all planner tests: empty/proposals/ordering + i1/lossy/valid-delta/i3).

- [ ] **Step 5: Commit**

```bash
git add crates/cull-core
git commit -m "feat(core): planner enforces lossless Replace (apply-diff verification) + I1/I3"
```

---

### Task 3: Emitter renders `rendered` + round-trip property (`emit.rs`)

**Files:** Modify `crates/cull-core/src/emit.rs`; update its test module.

- [ ] **Step 1: Update the test**

In `emit.rs` test module, update the Replace construction in `applies_keep_drop_replace_and_orders_by_stability` to the new shape and add a round-trip test:
```rust
    // in applies_keep_drop_replace_and_orders_by_stability, the seg(2) entry becomes:
    PlanEntry { id: SegmentId(2), action: SegmentAction::Replace {
        rendered: b"cc".to_vec(), token_count: 3,
        reconstruct: crate::plan::Reconstruct::Delta { base: SegmentId(1) }, reason: DropReason::Duplicate } },
```
(The emitter does not verify reconstruct — it only renders `rendered` and counts `token_count` — so any `reconstruct` value is fine here; the assertions on `emitted[1].bytes == b"cc"` and `token_count == 3` are unchanged.)

Add a new test:
```rust
    #[test]
    fn emits_delta_rendered_and_it_round_trips_to_original() {
        use crate::plan::{apply_unified_diff, Reconstruct};
        let base_text = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let new_text  = "fn a() {}\nfn b2() {}\nfn c() {}\n";
        let base = seg(0, 0, MutationClass::Fast, 20, base_text);
        let target = seg(1, 1, MutationClass::Fast, 20, new_text);
        let patch = diffy::create_patch(base_text, new_text).to_string();

        let plan = CompressionPlan { entries: vec![
            PlanEntry { id: SegmentId(0), action: SegmentAction::Keep },
            PlanEntry { id: SegmentId(1), action: SegmentAction::Replace {
                rendered: patch.clone().into_bytes(), token_count: 5,
                reconstruct: Reconstruct::Delta { base: SegmentId(0) }, reason: DropReason::Duplicate } },
        ]};
        let (emitted, _report) = emit(&[base.clone(), target.clone()], &plan);
        // the emitted Replace bytes are the diff; applying it to base recovers the exact original
        let emitted_delta = &emitted.iter().find(|e| e.id == SegmentId(1)).unwrap().bytes;
        let recovered = apply_unified_diff(base.bytes.as_slice(), emitted_delta).unwrap();
        assert_eq!(recovered, target.bytes);
    }
```
Add `diffy` is already a dep of cull-core (Task 1). Ensure the emit test imports compile (`use crate::plan::...` as needed).

- [ ] **Step 2: Run tests to verify they fail/compile-error**

Run: `cargo test -p cull-core emit::`
Expected: FAIL to compile (old `Replace { bytes, .. }` shape) until updated.

- [ ] **Step 3: Update the emitter match arm**

In `emit.rs`, the `SegmentAction::Replace` arm changes `bytes` → `rendered`:
```rust
            Some(SegmentAction::Replace { rendered, token_count, .. }) => {
                emitted.push(EmittedSegment { id, bytes: rendered.clone(), token_count: *token_count });
                replaced += 1;
                net += *token_count;
            }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cull-core emit::`
Expected: PASS (ordering test + round-trip test).

- [ ] **Step 5: Full workspace test + commit**

Run: `cargo test --workspace`
Expected: PASS (everything, including the CLI integration tests that exercise the engine).

```bash
git add crates/cull-core
git commit -m "feat(core): emitter renders Replace.rendered; delta round-trip property test"
```

---

## Self-Review

**1. Spec coverage:**
- §9 I4/I5 now *verified* (apply-diff reconstruction) instead of the old too-strict byte-subslice → Tasks 1–2. ✓
- §9 I1 (Replace reduces) and I3 (frozen) preserved → Task 2. ✓
- Emitter renders the new model → Task 3. ✓
- §7 A2 IVM/delta pass that produces these Replaces → Plan 8 (consumes this model). Not in scope.

**2. Placeholder scan:** No vague steps. Old `replace_preserves_protected`/`contains_subslice`/`StripProtectedPass` are explicitly removed; every existing Replace construction in tests is updated to the new shape. ✓

**3. Type consistency:** `Reconstruct`/`apply_unified_diff`/`replace_is_lossless` (Task 1) used by `enforce_invariants` (Task 2) and the emit round-trip test (Task 3). `SegmentAction::Replace { rendered, token_count, reconstruct, reason }` is consistent across `plan.rs`, `planner.rs`, `emit.rs`, and all updated tests. `diffy::{create_patch, Patch::from_str, apply}` used in plan.rs (impl) and tests (create_patch). The CLI's `run_compress` does not construct Replace directly (it only runs Drop-based passes), so it is unaffected. ✓

**4. Ambiguity check:** `apply_unified_diff` returns None on non-UTF8 or non-applying patches → `replace_is_lossless` false → revert to Keep (safe). Missing base id → false → revert. The emitter never verifies losslessness (single responsibility); the planner is the sole enforcer. ✓

**Outcome:** Replace is now a proven-lossless operation. Plan 8 implements the file-read IVM/delta pass that emits `Reconstruct::Delta` Replaces — and the planner will automatically verify each one reconstructs its base exactly, or revert it.
