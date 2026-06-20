# Cull — Plan 16: Transitive Symbol-Closure Slice (B1, part 1)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Upgrade query-relevance from *direct* symbol overlap to a *transitive* slice: relevance propagates through shared symbols, so a segment is kept if it connects to the task through a chain of symbol dependencies — the spec's §7 B1 "dependency-slice" idea. (Tree-sitter-precise symbol extraction is the next step; this plan does the slice algorithm over the existing extractor.)

**Architecture:** `RelevancePass` becomes a BFS over a symbol-sharing graph. Seed the relevant set from segments whose symbols overlap the task; repeatedly add any segment sharing a symbol with the growing relevant set, accumulating its symbols (fixpoint). Droppable + old segments NOT in the relevant set are dropped. This strictly keeps *more* than direct overlap (it follows dependency chains), so it only reduces false drops. Recency guard and "no task → no drops" are unchanged.

**Tech:** Rust. Builds on Plan 4. Reference: spec §7 B1.

---

### Task 1: Transitive-closure relevance

**Files:** `crates/cull-core/src/passes/relevance.rs`; tests inline.

- [ ] **Step 1 — failing test.** Add to the relevance test module (keep the existing tests; they must still pass):
```rust
    #[test]
    fn relevance_propagates_transitively_through_shared_symbols() {
        // task mentions "auth"; seg A (auth+jwt) is direct; seg B (jwt+middleware) connects via "jwt";
        // seg C (kafka) is unconnected and old -> dropped.
        let task = TaskSignal::from_text("auth subsystem");
        let segs = vec![
            seg(0, 0, SegmentKind::FileRead, "auth login session jwt"),       // direct (auth)
            seg(1, 1, SegmentKind::FileRead, "jwt verify middleware token"),  // transitive via jwt
            seg(2, 2, SegmentKind::FileRead, "kafka broker partitions offset"),// unconnected
        ];
        let plan = Planner::new(vec![Box::new(RelevancePass { recency_keep: 0 })])
            .plan_with_task(&segs, &SessionState::default(), &task);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);                          // direct
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);                          // transitive — kept
        assert_eq!(plan.entries[2].action, SegmentAction::Drop(DropReason::IrrelevantBySlice)); // unconnected
    }
```
(`seg` helper here takes `(id, pos, kind, text)`; if the existing helper differs, adapt the call to match it.)

- [ ] **Step 2 — confirm FAIL** (current direct-overlap drops seg1, which shares no symbol with the *task* directly).

- [ ] **Step 3 — implement.** Replace the body of `RelevancePass::propose` with the transitive slice:
```rust
    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        if ctx.task.is_empty() { return Vec::new(); }
        let max_pos = ctx.segments.iter().map(|s| s.position).max().unwrap_or(0);

        // symbols per segment
        let seg_syms: Vec<std::collections::HashSet<String>> = ctx.segments.iter()
            .map(|s| extract_symbols(&String::from_utf8_lossy(&s.bytes)))
            .collect();

        // BFS: relevance propagates from task-overlapping segments through shared symbols
        let mut relevant = vec![false; ctx.segments.len()];
        let mut active: std::collections::HashSet<String> = ctx.task.symbols.clone();
        loop {
            let mut changed = false;
            for i in 0..ctx.segments.len() {
                if !relevant[i] && !seg_syms[i].is_disjoint(&active) {
                    relevant[i] = true;
                    for s in &seg_syms[i] { active.insert(s.clone()); }
                    changed = true;
                }
            }
            if !changed { break; }
        }

        ctx.segments.iter().enumerate().filter_map(|(i, s)| {
            if !is_droppable_kind(&s.kind) { return None; }
            if max_pos.saturating_sub(s.position) < self.recency_keep { return None; }
            if relevant[i] { return None; }
            Some(PlanEntry { id: s.id, action: SegmentAction::Drop(DropReason::IrrelevantBySlice) })
        }).collect()
    }
```

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-core relevance` + the new test). All prior relevance tests still pass (the unconnected-irrelevant cases stay dropped; nothing that was kept becomes dropped).

- [ ] **Step 5 — workspace + commit.** `cargo test --workspace` green, then `git add crates/cull-core && git commit -m "feat(core): transitive symbol-closure relevance slice (B1 dependency-slice)"`

---

## After this plan — ledger
- ⚠️ B1 — transitive-closure slice ✅ (dependency propagation); tree-sitter-precise symbol extraction still ❌ (next plan). Keep B1 as ⚠️ until tree-sitter lands.

## Self-Review
- Transitive closure only ADDS to the relevant set vs direct overlap → strictly fewer drops → existing "kept" assertions hold; only previously-false-dropped transitive segments are now kept. ✓
- Fixpoint terminates (relevant set grows monotonically, bounded by n). ✓
- Empty task → no drops; recency + droppable-kind guards unchanged. ✓
- O(n² · symbols) worst case over tool segments — fine for realistic counts. ✓
