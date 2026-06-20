# Cull — Plan 19: Belady-Oracle + ARC Phase-Decay Eviction (C1, C2)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Upgrade the eviction priority to close §7 C1 (Belady-oracle lookahead) and C2 (ARC frequency × recency, phase-decay). Currently eviction ranks by task-relevance + position; this makes it richer with a forward signal and a frequency signal.

**Architecture:** Replace the standalone `eviction_priority` with an in-`evict_to_budget` priority that uses three signals: **C1 future-need** — a segment is "needed soon" if its symbols intersect the *future set* = the task symbols ∪ the symbols of any `CompactSummary` segment (the running plan/state = the agent's stated near-future); **C2 frequency** — how many other segments co-reference its symbols (frequently-referenced = important); **phase/recency** — `position` (earlier = discovery phase, decays/evicts first). Priority = future-need (dominant) × frequency × recency. Lowest priority evicted first; frozen never evicted.

**Tech:** Rust; uses `crate::code::extract_symbols_for` (Plan 17). Builds on Plan 9. Reference: spec §7 C1/C2, §8 Rule 4.

---

### Task 1: Forward + frequency + phase eviction priority

**Files:** `crates/cull-core/src/planner.rs`; tests inline.

- [ ] **Step 1 — failing test.** Add to the planner test module:
```rust
    #[test]
    fn eviction_prefers_future_needed_and_frequently_referenced() {
        let task = TaskSignal::from_text("auth");
        // budget forces dropping 1 of the 3 fast segments.
        // - seg1 shares "auth" with the task (future-needed) -> keep
        // - seg2 shares "session" with a CompactSummary plan/state (future-needed) -> keep
        // - seg3 is unrelated + old -> evicted
        let mut s1 = kb(1, 1, MutationClass::Fast, 100, "auth login flow");
        let mut s2 = kb(2, 2, MutationClass::Fast, 100, "session token rotation");
        let mut plan_seg = kb(3, 3, MutationClass::Fast, 1, "next: refactor session handling");
        plan_seg.kind = SegmentKind::CompactSummary;
        let s3 = kb(0, 0, MutationClass::Fast, 100, "kafka broker partitions offset");
        let segs = vec![s3, s1, s2, plan_seg];
        let plan = Planner::new(vec![]).plan_with_budget(&segs, &SessionState::default(), &task, Some(201));
        // total ~301; budget 201 -> drop ~100. The unrelated old seg (entry 0) goes first.
        assert_eq!(plan.entries[0].action, SegmentAction::Drop(DropReason::Evicted));
        assert_eq!(plan.entries[1].action, SegmentAction::Keep); // auth (task)
        assert_eq!(plan.entries[2].action, SegmentAction::Keep); // session (plan/state)
    }
```
(Keep the existing eviction tests; they must still pass — relevance still dominates so the prior assertions hold.)

- [ ] **Step 2 — confirm FAIL/already-pass.** If it already passes via the old priority, that's fine — Step 3 still strengthens the priority; ensure the existing tests stay green.

- [ ] **Step 3 — implement.** Remove the standalone `eviction_priority` function and replace `evict_to_budget` with:
```rust
fn evict_to_budget(actions: &mut [SegmentAction], segments: &[Segment], task: &crate::task::TaskSignal, budget: u32) {
    let mut net: u32 = actions.iter().zip(segments).map(|(a, s)| action_tokens(a, s)).sum();
    if net <= budget { return; }

    // symbol sets (path-aware, tree-sitter for code)
    let syms: Vec<std::collections::HashSet<String>> = segments.iter()
        .map(|s| crate::code::extract_symbols_for(&String::from_utf8_lossy(&s.bytes), s.origin.path.as_deref()))
        .collect();

    // C1 future-need signal: task symbols ∪ symbols of any CompactSummary (running plan/state)
    let mut future = task.symbols.clone();
    for (i, s) in segments.iter().enumerate() {
        if matches!(s.kind, SegmentKind::CompactSummary) {
            for x in &syms[i] { future.insert(x.clone()); }
        }
    }
    // C2 co-reference frequency
    let freq: Vec<u32> = (0..segments.len()).map(|i| {
        (0..segments.len()).filter(|&j| j != i && !syms[i].is_disjoint(&syms[j])).count() as u32
    }).collect();

    // priority: future-need dominates, then frequency, then recency (position = phase: early decays first)
    let priority = |i: usize| -> u64 {
        let future_need = if !future.is_empty() && !syms[i].is_disjoint(&future) { 1u64 } else { 0 };
        future_need * 1_000_000_000 + (freq[i] as u64) * 100_000 + segments[i].position as u64
    };

    let mut cands: Vec<usize> = (0..segments.len())
        .filter(|&i| segments[i].mutation_class != MutationClass::Frozen
            && !matches!(actions[i], SegmentAction::Drop(_)))
        .collect();
    cands.sort_by_key(|&i| priority(i)); // ascending: lowest priority evicted first
    for i in cands {
        if net <= budget { break; }
        let saved = action_tokens(&actions[i], &segments[i]);
        actions[i] = SegmentAction::Drop(DropReason::Evicted);
        net -= saved;
    }
}
```
Ensure `SegmentKind` is in scope in `planner.rs` (it likely is via `crate::segment::*`).

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-core planner::` — new + all existing eviction/quality-floor tests).

- [ ] **Step 5 — workspace + commit.** `cargo test --workspace` green; `git add crates/cull-core && git commit -m "feat(core): Belady-oracle + ARC eviction priority (future-need x frequency x phase)"`

---

## After this plan — ledger
- ✅ C1 Belady-oracle eviction — future set = task ∪ running `CompactSummary` plan/state.
- ✅ C2 ARC freq×recency + phase-decay — co-reference frequency + position phase.

## Self-Review
- Frozen never a candidate; only adds Drops to non-frozen survivors → invariants hold. ✓
- Future-need term dominates (1e9) so task-relevant content is still preserved (existing tests hold). ✓
- O(n²) symbol comparisons for frequency — fine for realistic segment counts. ✓
