# Cull — Plan 2: Cache Model + Planner + Invariants Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the provider-parameterized cache economic model, the plan/action data types, the `Pass` trait + `Planner` that composes passes, and the three enforced correctness invariants (I1 net-non-negative, I3 prefix immutability, I4 protected-token preservation) — the core interfaces every compression pass in Plans 3–5 will implement and rely on.

**Architecture:** `cull-cache` is a standalone crate holding the cache model and the break-even/amortization formulas from spec §8. `cull-core` gains a `plan` module (the action/plan types + the I4 validator) and a `planner` module (the `Pass` trait, `PlanCtx`, and the `Planner` that runs passes and enforces invariants per-entry). With zero passes registered, the planner is the identity (all-Keep) — which is the correct, safe default. No actual compression passes exist yet; those are Plans 3–5.

**Tech Stack:** Rust. Builds on Plan 1's `cull-core` (`Segment`, `SegmentId`, `MutationClass`, `ProtectedSpan`, `SessionState`). Reference: spec `docs/superpowers/specs/2026-06-18-cull-design.md` §8 (cache model + economic formulas), §9 (invariants I1–I6), §7 (pass taxonomy).

---

## File Structure

```
crates/cull-cache/src/lib.rs       # Provider, CacheModel, economic formulas
crates/cull-core/src/plan.rs       # SegmentAction, DropReason, PlanEntry, CompressionPlan, net_tokens, I4 validator
crates/cull-core/src/planner.rs    # Pass trait, PlanCtx, Planner (compose + enforce I1/I3/I4), stability_order
crates/cull-core/src/lib.rs        # add `pub mod plan; pub mod planner;` + re-exports
```

Split rationale: cache economics is independent of the engine's data model (pure formulas) → its own crate. Plan types and the planner change together with the engine → `cull-core`. The `Pass` trait lives in `planner.rs` because passes are planner inputs.

---

### Task 1: Cache economic model (`cull-cache`)

**Files:** Replace `crates/cull-cache/src/lib.rs`; test inline.

- [ ] **Step 1: Write the failing test**

Put in `crates/cull-cache/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caching_break_even_thresholds() {
        let a5 = CacheModel::for_provider(Provider::Anthropic5m);
        assert!(a5.caching_net_positive(0.22));   // > 0.2174
        assert!(!a5.caching_net_positive(0.21));

        let a1 = CacheModel::for_provider(Provider::Anthropic1h);
        assert!(a1.caching_net_positive(0.53));    // > 0.5263
        assert!(!a1.caching_net_positive(0.52));

        let oa = CacheModel::for_provider(Provider::OpenAi);
        assert!(oa.caching_net_positive(0.0001));  // threshold is 0 (no write premium)
    }

    #[test]
    fn amortization_gate_threshold() {
        let a5 = CacheModel::for_provider(Provider::Anthropic5m);
        // W/((1-c)*R) = 1.25/((0.4)*0.1) = 31.25
        assert!(a5.amortization_gate(0.6, 32));
        assert!(!a5.amortization_gate(0.6, 31));
        // no compression never amortizes
        assert!(!a5.amortization_gate(1.0, 10_000));
    }

    #[test]
    fn cache_bust_threshold() {
        let a5 = CacheModel::for_provider(Provider::Anthropic5m);
        // (t_old - t_new)*R*N > t_new*W ; 100k->20k: (80000*0.1*N) > 20000*1.25 => N > 3.125
        assert!(a5.cache_bust_worth_it(100_000, 20_000, 4));
        assert!(!a5.cache_bust_worth_it(100_000, 20_000, 3));
        // growing (t_new >= t_old) is never worth busting
        assert!(!a5.cache_bust_worth_it(20_000, 100_000, 1000));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-cache`
Expected: FAIL (`CacheModel` / `Provider` not defined).

- [ ] **Step 3: Implement the model**

Above the test module:

```rust
/// Caching provider + TTL regime (spec §8). Determines the write/read multipliers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider { Anthropic5m, Anthropic1h, OpenAi }

/// Provider-parameterized cache economics (spec §8). `write_mult` (W) and `read_mult` (R)
/// are relative to base input price; all thresholds derive from them.
#[derive(Debug, Clone, Copy)]
pub struct CacheModel {
    pub write_mult: f64,        // W
    pub read_mult: f64,         // R
    pub min_prefix_tokens: u32, // below this, no caching occurs
}

impl CacheModel {
    pub fn for_provider(p: Provider) -> CacheModel {
        match p {
            Provider::Anthropic5m => CacheModel { write_mult: 1.25, read_mult: 0.1, min_prefix_tokens: 1024 },
            Provider::Anthropic1h => CacheModel { write_mult: 2.0,  read_mult: 0.1, min_prefix_tokens: 1024 },
            Provider::OpenAi      => CacheModel { write_mult: 1.0,  read_mult: 0.1, min_prefix_tokens: 1024 },
        }
    }

    /// Caching is net-positive when hit_rate h > (W-1)/(W-R).
    pub fn caching_net_positive(&self, hit_rate: f64) -> bool {
        hit_rate > (self.write_mult - 1.0) / (self.write_mult - self.read_mult)
    }

    /// Compress-once-at-boundary pays off when n_future > W / ((1-c) * R),
    /// where c = compressed_tokens / original_tokens (c < 1 means smaller).
    pub fn amortization_gate(&self, compression_ratio: f64, n_future_turns: u32) -> bool {
        if compression_ratio >= 1.0 { return false; }
        (n_future_turns as f64) > self.write_mult / ((1.0 - compression_ratio) * self.read_mult)
    }

    /// Deliberately busting the cache to shrink context pays off when
    /// (t_old - t_new) * R * n_future > t_new * W.
    pub fn cache_bust_worth_it(&self, t_old: u32, t_new: u32, n_future_turns: u32) -> bool {
        if t_new >= t_old { return false; }
        ((t_old - t_new) as f64) * self.read_mult * (n_future_turns as f64)
            > (t_new as f64) * self.write_mult
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-cache`
Expected: PASS (all three).

- [ ] **Step 5: Commit**

```bash
git add crates/cull-cache
git commit -m "feat(cache): provider-parameterized cache economic model"
```

---

### Task 2: Plan & action types + I4 validator (`cull-core::plan`)

**Files:** Create `crates/cull-core/src/plan.rs`; modify `crates/cull-core/src/lib.rs`; test inline.

- [ ] **Step 1: Write the failing test**

Put in `crates/cull-core/src/plan.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use crate::protected::{ProtectedKind, ProtectedSpan};

    fn seg(id: u64, text: &str, protected: Vec<ProtectedSpan>) -> Segment {
        Segment {
            id: SegmentId(id), kind: SegmentKind::ToolOutput { class: "grep".into() },
            role: Role::Tool, bytes: text.as_bytes().to_vec(), token_count: 10, position: id as usize,
            mutation_class: MutationClass::Fast, origin: Origin::default(),
            protected_spans: protected, refs: RefLedger::default(),
        }
    }

    #[test]
    fn net_tokens_accounts_keep_drop_replace() {
        let segs = vec![seg(0, "aaaa", vec![]), seg(1, "bbbb", vec![])]; // 10 + 10 = 20
        let plan = CompressionPlan { entries: vec![
            PlanEntry { id: SegmentId(0), action: SegmentAction::Keep },
            PlanEntry { id: SegmentId(1), action: SegmentAction::Drop(DropReason::Duplicate) },
        ]};
        assert_eq!(net_tokens(&plan, &segs), 10); // kept 10 + dropped 0
        assert_eq!(input_tokens(&segs), 20);
    }

    #[test]
    fn i4_validator_rejects_replace_that_drops_a_protected_token() {
        // segment text contains a path that must survive a Replace
        let text = "see src/auth.rs:42 for details";
        let protected = vec![ProtectedSpan { span: Span { start: 4, end: 15 }, kind: ProtectedKind::Path }]; // "src/auth.rs"
        let s = seg(0, text, protected);

        // a Replace that keeps the path -> valid
        let keeps_path = SegmentAction::Replace { bytes: b"src/auth.rs changed".to_vec(), token_count: 3, reason: DropReason::Duplicate };
        assert!(replace_preserves_protected(&s, &keeps_path));

        // a Replace that drops the path -> I4 violation
        let drops_path = SegmentAction::Replace { bytes: b"a file changed".to_vec(), token_count: 3, reason: DropReason::Duplicate };
        assert!(!replace_preserves_protected(&s, &drops_path));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core plan::`
Expected: FAIL (types/functions not defined).

- [ ] **Step 3: Implement the plan types + helpers**

Above the test module:

```rust
use crate::segment::{Segment, SegmentId};

/// Why a segment was dropped or replaced (audit + fidelity report).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropReason { Superseded, IrrelevantBySlice, Duplicate, Evicted, StaleOutput }

/// What the planner decided for one segment. Drop removes a WHOLE unit (allowed —
/// the design drops whole irrelevant/superseded/duplicate units). Replace substitutes a
/// LOSSLESS smaller representation (delta/dedup/stub); it must preserve every protected
/// token of the original (enforced by `replace_preserves_protected`, spec I2/I4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentAction {
    Keep,
    Drop(DropReason),
    Replace { bytes: Vec<u8>, token_count: u32, reason: DropReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry { pub id: SegmentId, pub action: SegmentAction }

/// One decision per input segment, in input order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompressionPlan { pub entries: Vec<PlanEntry> }

/// Total tokens of the original context.
pub fn input_tokens(segments: &[Segment]) -> u32 {
    segments.iter().map(|s| s.token_count).sum()
}

/// Total tokens the plan would emit (Keep = original, Drop = 0, Replace = replacement).
/// Assumes `plan.entries[i]` corresponds to `segments[i]` (planner guarantees this).
pub fn net_tokens(plan: &CompressionPlan, segments: &[Segment]) -> u32 {
    plan.entries.iter().zip(segments.iter()).map(|(e, s)| match &e.action {
        SegmentAction::Keep => s.token_count,
        SegmentAction::Drop(_) => 0,
        SegmentAction::Replace { token_count, .. } => *token_count,
    }).sum()
}

/// I4/I2 check for a Replace: every protected token of the original must appear,
/// byte-exact, in the replacement bytes. Drop is exempt (the whole unit is removed).
pub fn replace_preserves_protected(original: &Segment, action: &SegmentAction) -> bool {
    let SegmentAction::Replace { bytes, .. } = action else { return true; };
    original.protected_spans.iter().all(|p| {
        let needle = &original.bytes[p.span.start..p.span.end];
        contains_subslice(bytes, needle)
    })
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() { return true; }
    haystack.windows(needle.len()).any(|w| w == needle)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core plan::`
Expected: PASS (both tests).

- [ ] **Step 5: Wire module + commit**

Add to `crates/cull-core/src/lib.rs` (after the existing `pub mod` lines):
```rust
pub mod plan;
```
and to the re-export block:
```rust
pub use plan::{CompressionPlan, DropReason, PlanEntry, SegmentAction};
```

Run `cargo build -p cull-core` → PASS. Then:
```bash
git add crates/cull-core
git commit -m "feat(core): compression plan/action types + I4 protected-preservation validator"
```

---

### Task 3: Pass trait + Planner (`cull-core::planner`)

**Files:** Create `crates/cull-core/src/planner.rs`; modify `crates/cull-core/src/lib.rs`; test inline.

- [ ] **Step 1: Write the failing test**

Put in `crates/cull-core/src/planner.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use crate::plan::{SegmentAction, DropReason};
    use crate::session::SessionState;

    fn seg(id: u64, class: MutationClass) -> Segment {
        Segment {
            id: SegmentId(id), kind: SegmentKind::FileRead, role: Role::Tool,
            bytes: b"x".to_vec(), token_count: 10, position: id as usize,
            mutation_class: class, origin: Origin::default(),
            protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    // a test pass that drops every segment whose id is in `ids`
    struct DropPass { ids: Vec<u64> }
    impl Pass for DropPass {
        fn name(&self) -> &'static str { "drop-test" }
        fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
            ctx.segments.iter().filter(|s| self.ids.contains(&s.id.0))
               .map(|s| PlanEntry { id: s.id, action: SegmentAction::Drop(DropReason::Duplicate) })
               .collect()
        }
    }

    #[test]
    fn empty_planner_is_identity_keep_all() {
        let segs = vec![seg(0, MutationClass::Fast), seg(1, MutationClass::Fast)];
        let session = SessionState::default();
        let plan = Planner::new(vec![]).plan(&segs, &session);
        assert_eq!(plan.entries.len(), 2);
        assert!(plan.entries.iter().all(|e| e.action == SegmentAction::Keep));
    }

    #[test]
    fn pass_proposals_are_applied() {
        let segs = vec![seg(0, MutationClass::Fast), seg(1, MutationClass::Fast)];
        let session = SessionState::default();
        let plan = Planner::new(vec![Box::new(DropPass { ids: vec![1] })]).plan(&segs, &session);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(plan.entries[1].action, SegmentAction::Drop(DropReason::Duplicate));
    }

    #[test]
    fn stability_order_sorts_frozen_before_fast_then_by_position() {
        let segs = vec![
            seg(0, MutationClass::Fast),
            seg(1, MutationClass::Frozen),
            seg(2, MutationClass::Slow),
            seg(3, MutationClass::Frozen),
        ];
        let order = stability_order(&segs);
        // Frozen (1,3) then Slow (2) then Fast (0), each group by original position
        assert_eq!(order, vec![SegmentId(1), SegmentId(3), SegmentId(2), SegmentId(0)]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core planner::`
Expected: FAIL (`Pass` / `Planner` / `PlanCtx` / `stability_order` not defined).

- [ ] **Step 3: Implement the planner**

Above the test module:

```rust
use crate::plan::{CompressionPlan, PlanEntry, SegmentAction};
use crate::segment::{MutationClass, Segment, SegmentId};
use crate::session::SessionState;

/// Read-only context a pass sees. Later plans add the task signal, cache model, and budget.
pub struct PlanCtx<'a> {
    pub segments: &'a [Segment],
    pub session: &'a SessionState,
}

/// A compression pass proposes actions for some segments. Segments it does not mention
/// keep their current action (default Keep). Passes run in registration order; a later
/// pass's proposal for a segment overrides an earlier one.
pub trait Pass {
    fn name(&self) -> &'static str;
    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry>;
}

pub struct Planner { passes: Vec<Box<dyn Pass>> }

impl Planner {
    pub fn new(passes: Vec<Box<dyn Pass>>) -> Self { Self { passes } }

    /// Produce a plan: start all-Keep, apply each pass's proposals in order, then enforce
    /// invariants (I3 prefix immutability, I4 protected preservation, I1 net-non-negative)
    /// per-entry. Entry order matches `segments` order.
    pub fn plan(&self, segments: &[Segment], session: &SessionState) -> CompressionPlan {
        let mut actions: Vec<SegmentAction> = vec![SegmentAction::Keep; segments.len()];
        let index: std::collections::HashMap<SegmentId, usize> =
            segments.iter().enumerate().map(|(i, s)| (s.id, i)).collect();

        let ctx = PlanCtx { segments, session };
        for pass in &self.passes {
            for entry in pass.propose(&ctx) {
                if let Some(&i) = index.get(&entry.id) {
                    actions[i] = entry.action;
                }
            }
        }

        // Invariant enforcement (Task 4 fills these in; for now they are identity).
        enforce_invariants(&mut actions, segments);

        CompressionPlan {
            entries: segments.iter().zip(actions).map(|(s, a)| PlanEntry { id: s.id, action: a }).collect(),
        }
    }
}

/// Order segment ids by ascending mutation frequency (Frozen, Slow, Fast), preserving
/// original position within each class. Used by the emitter for cache-stable assembly (spec §8 Rule 3).
pub fn stability_order(segments: &[Segment]) -> Vec<SegmentId> {
    fn rank(c: MutationClass) -> u8 { match c { MutationClass::Frozen => 0, MutationClass::Slow => 1, MutationClass::Fast => 2 } }
    let mut idx: Vec<usize> = (0..segments.len()).collect();
    idx.sort_by_key(|&i| (rank(segments[i].mutation_class), segments[i].position));
    idx.into_iter().map(|i| segments[i].id).collect()
}

/// Placeholder — Task 4 implements I1/I3/I4 enforcement here.
fn enforce_invariants(_actions: &mut [SegmentAction], _segments: &[Segment]) {}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core planner::`
Expected: PASS (all three).

- [ ] **Step 5: Wire module + commit**

Add to `crates/cull-core/src/lib.rs`:
```rust
pub mod planner;
```
and to re-exports:
```rust
pub use planner::{Pass, PlanCtx, Planner, stability_order};
```

Run `cargo build -p cull-core` → PASS. Then:
```bash
git add crates/cull-core
git commit -m "feat(core): Pass trait + Planner composition + stability ordering"
```

---

### Task 4: Invariant enforcement (I1, I3, I4)

**Files:** Modify `crates/cull-core/src/planner.rs` (replace `enforce_invariants`); test inline (extend the planner test module).

- [ ] **Step 1: Write the failing tests**

Add these tests to the existing `mod tests` in `planner.rs`:

```rust
    use crate::protected::{ProtectedKind, ProtectedSpan};

    // pass that REPLACES a segment with bigger content (violates I1)
    struct BloatPass;
    impl Pass for BloatPass {
        fn name(&self) -> &'static str { "bloat" }
        fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
            ctx.segments.iter().map(|s| PlanEntry { id: s.id,
                action: SegmentAction::Replace { bytes: vec![b'x'; 1000], token_count: s.token_count + 50, reason: DropReason::Duplicate } }).collect()
        }
    }

    // pass that tries to DROP a frozen segment (violates I3)
    struct DropFrozenPass;
    impl Pass for DropFrozenPass {
        fn name(&self) -> &'static str { "drop-frozen" }
        fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
            ctx.segments.iter().map(|s| PlanEntry { id: s.id, action: SegmentAction::Drop(DropReason::Evicted) }).collect()
        }
    }

    // pass that REPLACES a segment, dropping its protected path (violates I4)
    struct StripProtectedPass;
    impl Pass for StripProtectedPass {
        fn name(&self) -> &'static str { "strip-protected" }
        fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
            ctx.segments.iter().map(|s| PlanEntry { id: s.id,
                action: SegmentAction::Replace { bytes: b"redacted".to_vec(), token_count: 1, reason: DropReason::Duplicate } }).collect()
        }
    }

    #[test]
    fn i1_reverts_token_increasing_replace_to_keep() {
        let segs = vec![seg(0, MutationClass::Fast)]; // token_count 10
        let plan = Planner::new(vec![Box::new(BloatPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep); // bloat reverted
        assert_eq!(crate::plan::net_tokens(&plan, &segs), 10);   // never exceeds input
    }

    #[test]
    fn i3_reverts_drop_of_frozen_segment_to_keep() {
        let segs = vec![seg(0, MutationClass::Frozen), seg(1, MutationClass::Fast)];
        let plan = Planner::new(vec![Box::new(DropFrozenPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep); // frozen protected
        assert_eq!(plan.entries[1].action, SegmentAction::Drop(DropReason::Evicted)); // fast may drop
    }

    #[test]
    fn i4_reverts_replace_that_strips_protected_token_to_keep() {
        let mut s = seg(0, MutationClass::Fast);
        s.bytes = b"path src/a.rs here".to_vec();
        s.protected_spans = vec![ProtectedSpan { span: Span { start: 5, end: 12 }, kind: ProtectedKind::Path }]; // "src/a.rs"
        let plan = Planner::new(vec![Box::new(StripProtectedPass)]).plan(&[s.clone()], &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep); // strip reverted, exact token preserved
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cull-core planner::`
Expected: FAIL — `enforce_invariants` is a no-op, so violations are not reverted (the three new tests fail; the earlier ones still pass).

- [ ] **Step 3: Implement enforcement**

Replace the placeholder `enforce_invariants` in `planner.rs` with:

```rust
/// Enforce the three plan-level invariants per entry (spec §9):
/// I3 — a Frozen segment must be Keep (the cached prefix is immutable).
/// I4 — a Replace must preserve every protected token of the original, else revert to Keep.
/// I1 — a Replace must strictly reduce tokens, else revert to Keep (plan can never increase net).
fn enforce_invariants(actions: &mut [SegmentAction], segments: &[Segment]) {
    for (a, s) in actions.iter_mut().zip(segments.iter()) {
        // I3: frozen is immutable
        if s.mutation_class == MutationClass::Frozen && *a != SegmentAction::Keep {
            *a = SegmentAction::Keep;
            continue;
        }
        if let SegmentAction::Replace { token_count, .. } = a {
            // I1: replace must reduce
            let reduces = *token_count < s.token_count;
            // I4: replace must preserve protected tokens
            let preserves = crate::plan::replace_preserves_protected(s, a);
            if !reduces || !preserves {
                *a = SegmentAction::Keep;
            }
        }
    }
}
```

Note: `replace_preserves_protected` is `pub` in `plan.rs` (Task 2). If it is not yet `pub`, make it `pub`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cull-core planner::`
Expected: PASS (all six planner tests — the three originals plus the three invariant tests).

- [ ] **Step 5: Full workspace test + commit**

Run: `cargo test --workspace`
Expected: PASS (cull-core + cull-tokenize + cull-cache).

```bash
git add crates/cull-core
git commit -m "feat(core): enforce plan invariants I1 (net-non-negative), I3 (prefix immutable), I4 (protected-preserving)"
```

---

## Self-Review

**1. Spec coverage:**
- §8 cache model + break-even (`caching_net_positive`), amortization gate, cache-bust → Task 1. ✓
- §8 Rule 3 stability ordering → Task 3 `stability_order`. ✓
- §9 I1 net-non-negative → Task 4 (Replace must reduce). ✓
- §9 I3 prefix immutability → Task 4 (frozen forced Keep). ✓
- §9 I4/I2 protected/byte-exact preservation → Task 2 validator + Task 4 enforcement. ✓
- §7 `Pass` trait (the interface every pass implements) → Task 3. ✓
- I5 (lossless round-trip) and I6 (quality floor) are deferred: I5 is exercised once Replace-producing passes + the emitter exist (Plans 3, 6); I6 is the proxy's compress-to-floor loop (Plan 7). Not in this plan.

**2. Placeholder scan:** `enforce_invariants` is intentionally a no-op in Task 3 and replaced with the real implementation in Task 4 (TDD: the Task 4 tests fail against the no-op, pass against the real one). No "TBD"/vague steps. ✓

**3. Type consistency:** `SegmentAction`, `DropReason`, `PlanEntry`, `CompressionPlan` defined in Task 2 (`plan.rs`), used in Tasks 3–4 (`planner.rs`). `replace_preserves_protected` defined `pub` in Task 2, called in Task 4. `Pass`/`PlanCtx`/`Planner` defined Task 3, used Task 4. `net_tokens`/`input_tokens` defined Task 2, used in Task 4 test. `MutationClass`, `Segment`, `SegmentId`, `ProtectedSpan`, `SessionState` from Plan 1. ✓

**4. Ambiguity check:** Pass composition is explicit (later pass overrides earlier for the same id; unmentioned segments stay Keep). Invariant enforcement is per-entry and order-independent (I3 first via `continue`, then I1+I4 for Replace). `net_tokens` assumes entry[i]↔segment[i]; the planner constructs entries in segment order, so this holds. ✓

**Outcome:** Plan 2 delivers the cache economics, the pass/plan interfaces, and the enforced safety net. Plans 3–5 implement concrete `Pass`es (supersession, IVM/delta, slice, dedup, eviction) that plug into `Planner` and are automatically held to I1/I3/I4.
