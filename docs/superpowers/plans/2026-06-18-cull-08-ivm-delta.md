# Cull — Plan 8: File-Read IVM/Delta Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the file-read IVM/delta pass. When the same file is read more than once, the first read is the canonical base (kept verbatim) and later reads become a **unified diff against that base** (a lossless `Reconstruct::Delta` Replace). The planner (Plan 7) automatically verifies each diff reconstructs the original exactly, or reverts it.

**Architecture:** Files gain identity via a new `path: Option<String>` on `RawBlock`, carried into `Origin::path` by the segmenter. `IvmDeltaPass` matches `FileRead` segments by `origin.path`: the lowest-position read per path is the base; later *changed* reads become `Replace { rendered = unified diff, reconstruct = Delta { base }, token_count }`. Exact-duplicate re-reads are left for the dedup pass (the IVM pass skips them). The pass owns its own `ApproxCounter` to size the diff (no Planner/PlanCtx changes). Run order becomes supersession → IVM → dedup, so dedup's whole-unit Drop wins for any exact duplicate.

**Tech Stack:** Rust, `diffy` (already a cull-core dep). Builds on Plans 1–7. Reference: spec §7 A2 (IVM/delta), §9 (the planner verifies losslessness).

---

## File Structure

```
crates/cull-core/src/segment.rs       # RawBlock gains `path` (NOTE: RawBlock lives in segmenter.rs)
crates/cull-core/src/segmenter.rs     # RawBlock.path + segment() sets origin.path; update test `raw` helper
crates/cull-core/src/passes/ivm.rs    # IvmDeltaPass
crates/cull-core/src/passes/mod.rs    # add ivm; structural_passes order = supersession, ivm, dedup
crates/cull-core/tests/pipeline.rs    # update `raw` helper for the new field
crates/cull-cli/src/lib.rs            # InputBlock.path -> RawBlock.path
```

---

### Task 1: File identity — `RawBlock.path` + `Origin.path`

**Files:** Modify `crates/cull-core/src/segmenter.rs`, `crates/cull-core/tests/pipeline.rs`, `crates/cull-cli/src/lib.rs`. (`Origin` already has a `path: Option<String>` field from Plan 1.)

- [ ] **Step 1: Write the failing test**

In `crates/cull-core/src/segmenter.rs` test module, add:
```rust
    #[test]
    fn segment_carries_path_into_origin() {
        let counter = ApproxCounter::o200k();
        let blocks = vec![RawBlock {
            role: Role::Tool, kind: SegmentKind::FileRead,
            text: "fn main(){}".into(), path: Some("src/main.rs".into()),
        }];
        let segs = segment(&blocks, &counter);
        assert_eq!(segs[0].origin.path.as_deref(), Some("src/main.rs"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core segmenter::`
Expected: FAIL (compile error — `RawBlock` has no `path` field).

- [ ] **Step 3: Add the field and thread it**

In `crates/cull-core/src/segmenter.rs`:

(a) Add `path` to `RawBlock`:
```rust
#[derive(Debug, Clone)]
pub struct RawBlock {
    pub role: Role,
    pub kind: SegmentKind,
    pub text: String,
    pub path: Option<String>, // file path for FileRead/Diff, used by the IVM pass
}
```

(b) In `segment()`, set `origin.path`:
```rust
            origin: Origin { turn: i, path: b.path.clone(), ..Origin::default() },
```

(c) Update the existing `raw` test helper in `segmenter.rs` to include the field:
```rust
    fn raw(role: Role, kind: SegmentKind, text: &str) -> RawBlock {
        RawBlock { role, kind, text: text.to_string(), path: None }
    }
```

In `crates/cull-core/tests/pipeline.rs`, update its `raw` helper the same way:
```rust
fn raw(role: Role, kind: SegmentKind, text: &str) -> RawBlock {
    RawBlock { role, kind, text: text.to_string(), path: None }
}
```

In `crates/cull-cli/src/lib.rs`: add `path` to `InputBlock` and map it through:
```rust
#[derive(Debug, Deserialize)]
pub struct InputBlock {
    pub role: String,
    pub kind: String,
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    pub text: String,
}
```
and in `parse_blocks`, the `RawBlock { ... }` construction adds `path: b.path`:
```rust
        Ok(RawBlock {
            role: parse_role(&b.role)?,
            kind: parse_kind(&b.kind, &b.class)?,
            text: b.text,
            path: b.path,
        })
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --workspace`
Expected: PASS (the new segmenter test + all existing tests, after the `raw`/`RawBlock` updates).

- [ ] **Step 5: Commit**

```bash
git add crates/cull-core crates/cull-cli
git commit -m "feat(core): RawBlock.path threaded into Origin.path (file identity for IVM)"
```

---

### Task 2: IvmDeltaPass

**Files:** Create `crates/cull-core/src/passes/ivm.rs`; modify `crates/cull-core/src/passes/mod.rs`; tests inline.

- [ ] **Step 1: Write the failing test**

Put in `crates/cull-core/src/passes/ivm.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use crate::plan::SegmentAction;
    use crate::planner::Planner;
    use crate::session::SessionState;

    fn file_seg(id: u64, pos: usize, path: &str, text: &str) -> Segment {
        Segment {
            id: SegmentId(id), kind: SegmentKind::FileRead, role: Role::Tool,
            bytes: text.as_bytes().to_vec(), token_count: 100, position: pos,
            mutation_class: MutationClass::Fast,
            origin: Origin { turn: pos, path: Some(path.into()), ..Origin::default() },
            protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn reread_with_small_change_becomes_lossless_delta() {
        let base = file_seg(0, 0, "src/a.rs",
            "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\nfn five() {}\n");
        let reread = file_seg(1, 5, "src/a.rs",
            "fn one() {}\nfn two() {}\nfn THREE() {}\nfn four() {}\nfn five() {}\n");
        let plan = Planner::new(vec![Box::new(IvmDeltaPass::new())])
            .plan(&[base.clone(), reread.clone()], &SessionState::default());
        // base kept; reread replaced by a lossless delta (planner already verified it reconstructs)
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert!(matches!(plan.entries[1].action, SegmentAction::Replace { .. }),
            "changed re-read of same path becomes a delta Replace");
    }

    #[test]
    fn different_paths_are_not_deltad() {
        let a = file_seg(0, 0, "src/a.rs", "alpha contents here and there\n");
        let b = file_seg(1, 1, "src/b.rs", "beta contents totally other\n");
        let plan = Planner::new(vec![Box::new(IvmDeltaPass::new())])
            .plan(&[a, b], &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);
    }

    #[test]
    fn exact_reread_is_left_for_dedup() {
        let base = file_seg(0, 0, "src/a.rs", "identical contents\n");
        let same = file_seg(1, 5, "src/a.rs", "identical contents\n");
        let plan = Planner::new(vec![Box::new(IvmDeltaPass::new())])
            .plan(&[base, same], &SessionState::default());
        // IVM skips exact duplicates -> Keep (dedup pass handles these)
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core ivm`
Expected: FAIL (`IvmDeltaPass` not defined).

- [ ] **Step 3: Implement the pass**

Above the test module:
```rust
use std::collections::HashMap;
use crate::plan::{DropReason, PlanEntry, Reconstruct, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::{Segment, SegmentId, SegmentKind};
use cull_tokenize::{ApproxCounter, TokenCounter};

/// File-read IVM/delta (spec §7 A2). For each file path, the lowest-position FileRead is the base
/// (kept). Later *changed* reads of that path become a unified-diff Replace against the base; the
/// planner verifies each diff reconstructs the original exactly (else reverts). Exact-duplicate
/// re-reads are skipped (the dedup pass drops them as whole units).
pub struct IvmDeltaPass { counter: ApproxCounter }

impl IvmDeltaPass {
    pub fn new() -> Self { Self { counter: ApproxCounter::o200k() } }
}

impl Default for IvmDeltaPass { fn default() -> Self { Self::new() } }

impl Pass for IvmDeltaPass {
    fn name(&self) -> &'static str { "file-read-ivm-delta" }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        // base = lowest-position FileRead per path
        let mut base_of: HashMap<&str, &Segment> = HashMap::new();
        for s in ctx.segments {
            if let (SegmentKind::FileRead, Some(path)) = (&s.kind, &s.origin.path) {
                base_of.entry(path.as_str())
                    .and_modify(|b| { if s.position < b.position { *b = s; } })
                    .or_insert(s);
            }
        }

        let mut out = Vec::new();
        for s in ctx.segments {
            let (SegmentKind::FileRead, Some(path)) = (&s.kind, &s.origin.path) else { continue; };
            let Some(base) = base_of.get(path.as_str()) else { continue; };
            if base.id == s.id { continue; }            // this IS the base
            if base.bytes == s.bytes { continue; }      // exact dup -> dedup handles it
            let (Ok(base_str), Ok(this_str)) =
                (std::str::from_utf8(&base.bytes), std::str::from_utf8(&s.bytes)) else { continue; };
            let patch = diffy::create_patch(base_str, this_str).to_string();
            let token_count = self.counter.count(&patch) as u32;
            out.push(PlanEntry { id: s.id, action: SegmentAction::Replace {
                rendered: patch.into_bytes(),
                token_count,
                reconstruct: Reconstruct::Delta { base: base.id },
                reason: DropReason::Duplicate,
            }});
        }
        out
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core ivm`
Expected: PASS (all three). The first test relies on the planner accepting the delta (it is lossless and the diff is smaller than the 100-token original).

- [ ] **Step 5: Wire + commit**

Add to `crates/cull-core/src/passes/mod.rs`:
```rust
pub mod ivm;
pub use ivm::IvmDeltaPass;
```
Add `pub use passes::IvmDeltaPass;` to `crates/cull-core/src/lib.rs` re-exports. Run `cargo build -p cull-core` → PASS.
```bash
git add crates/cull-core
git commit -m "feat(core): file-read IVM/delta pass (lossless diff for changed re-reads)"
```

---

### Task 3: Wire IVM into the structural pipeline + integration test

**Files:** Modify `crates/cull-core/src/passes/mod.rs`; test inline.

- [ ] **Step 1: Update `structural_passes` order + add the test**

Change `structural_passes()` in `passes/mod.rs` to run supersession → IVM → dedup:
```rust
pub fn structural_passes() -> Vec<Box<dyn Pass>> {
    vec![Box::new(SupersessionPass), Box::new(IvmDeltaPass::new()), Box::new(ExactDedupPass)]
}
```

Add to the `passes/mod.rs` test module (the existing `mod tests`):
```rust
    #[test]
    fn structural_pipeline_deltas_changed_reread() {
        use crate::session::SessionState;
        use crate::planner::Planner;
        use crate::plan::{SegmentAction, net_tokens, input_tokens};

        fn fseg(id: u64, pos: usize, path: &str, text: &str) -> Segment {
            Segment {
                id: SegmentId(id), kind: SegmentKind::FileRead, role: Role::Tool,
                bytes: text.as_bytes().to_vec(), token_count: 100, position: pos,
                mutation_class: MutationClass::Fast,
                origin: Origin { turn: pos, path: Some(path.into()), ..Origin::default() },
                protected_spans: vec![], refs: RefLedger::default(),
            }
        }
        let base = fseg(0, 0, "src/x.rs", "line a\nline b\nline c\nline d\nline e\nline f\n");
        let reread = fseg(1, 1, "src/x.rs", "line a\nline b\nline CHANGED\nline d\nline e\nline f\n");
        let plan = Planner::new(super::structural_passes()).plan(&[base.clone(), reread.clone()], &SessionState::default());
        assert!(matches!(plan.entries[1].action, SegmentAction::Replace { .. }));
        assert!(net_tokens(&plan, &[base, reread]) < input_tokens(&[fseg(0,0,"src/x.rs","x"); 2]).max(1) + 200);
    }

    #[test]
    fn structural_passes_has_three_passes() {
        assert_eq!(super::structural_passes().len(), 3);
    }
```
(If the `net_tokens` assertion is awkward, replace its body with a simpler check that `net_tokens(&plan, &segs) < input_tokens(&segs)` using the actual `base`/`reread` segs — the intent is "the delta reduced tokens.")

- [ ] **Step 2: Run test to verify it fails (count) / passes**

Run: `cargo test -p cull-core passes`
Expected: the `has_three_passes` test FAILS first (count was 2), then passes after Step 1's change; the delta test passes given the implemented pass.

- [ ] **Step 3: (no new impl — Step 1 already updated `structural_passes`)**

Confirm the order is supersession → IVM → dedup so dedup's Drop wins for any exact duplicate.

- [ ] **Step 4: Full workspace test**

Run: `cargo test --workspace`
Expected: PASS (everything; the CLI smoke/integration still green since IVM only activates on same-path FileRead segments, which the CLI smoke input does not contain).

- [ ] **Step 5: Commit**

```bash
git add crates/cull-core
git commit -m "feat(core): wire IVM into structural pipeline (supersession -> ivm -> dedup)"
```

---

## Self-Review

**1. Spec coverage:**
- §7 A2 file-read IVM/delta (canonical base + diff for changed re-reads) → Tasks 1–2. ✓
- Pipeline ordering so exact dups Drop and changed re-reads Delta → Task 3. ✓
- Losslessness is enforced by the Plan 7 planner (the pass just proposes; the planner verifies `apply_diff(base, rendered) == original`). ✓
- Cross-session canonical persistence (the `SessionState::CanonicalFileStore`) is a later proxy concern; within one plan call the base is the first read in the batch. Noted, not in scope.

**2. Placeholder scan:** No vague steps. The one soft spot (the `net_tokens` assertion phrasing in Task 3) has an explicit fallback instruction. ✓

**3. Type consistency:** `RawBlock.path`/`Origin.path` (Task 1) read by `IvmDeltaPass` (Task 2). `Reconstruct::Delta`/`SegmentAction::Replace` from Plan 7. `Pass`/`PlanCtx`/`PlanEntry` from Plan 2. `ApproxCounter`/`TokenCounter` from Plan 1. `structural_passes` (Task 3) composes `SupersessionPass`+`IvmDeltaPass`+`ExactDedupPass`. The CLI's `InputBlock.path` (Task 1) flows to `RawBlock.path`. ✓

**4. Ambiguity check:** Base = lowest `position` per path. A segment equal to its base id is skipped (it IS the base). Exact-content re-reads are skipped (dedup handles them). Non-UTF8 file bytes → skipped (Keep). The planner reverts any delta that is not strictly smaller or not perfectly reconstructing — so a pathological "diff bigger than the file" case safely degrades to Keep. ✓

**Outcome:** File re-reads after edits now cost a small diff instead of a full copy, losslessly and verifiably. Next: RePair envelope dedup + budget-driven eviction, then the proxy, then the benchmark.
