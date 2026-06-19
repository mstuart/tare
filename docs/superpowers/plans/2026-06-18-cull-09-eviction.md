# Cull — Plan 9: Budget-Driven Eviction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add budget-driven eviction. When the planned context still exceeds a token budget after the passes run, drop the lowest-priority non-frozen segments — task-irrelevant and old first — until it fits. Frozen (cached-prefix) segments are never evicted.

**Architecture:** Eviction is a **planner stage**, not a pass: it needs the post-composition net and a budget, which individual passes don't have. `Planner::plan_with_budget(segments, session, task, budget: Option<u32>)` becomes the core; `plan`/`plan_with_task` delegate with `budget = None` (no eviction). After the passes compose and invariants are enforced, if a budget is set and net > budget, surviving non-frozen Keep/Replace segments are sorted by ascending priority and dropped until net ≤ budget. Priority is a v1 heuristic = task-relevance (huge weight) then recency — approximating the spec's Belady-oracle (keep what the task/future needs) with ARC/phase-decay as later refinements. The CLI gets a `--budget` flag.

**Tech Stack:** Rust. Builds on Plans 1–8 (`Planner`/`plan_with_task`, `SegmentAction`, `TaskSignal`/`extract_symbols`, `MutationClass`). Reference: spec §7 C1–C3 (Belady-oracle / ARC / tail-only eviction), §8 Rule 4 (tail-only — frozen prefix never evicted).

---

## File Structure

```
crates/cull-core/src/planner.rs   # plan_with_budget + evict_to_budget + eviction_priority
crates/cull-cli/src/lib.rs        # run_compress gains optional budget (delegating overload)
crates/cull-cli/src/main.rs       # --budget flag
```

---

### Task 1: Eviction stage in the planner

**Files:** Modify `crates/cull-core/src/planner.rs`; tests inline.

- [ ] **Step 1: Write the failing test**

Add to the `planner.rs` test module:
```rust
    use crate::task::TaskSignal;

    fn kb(id: u64, pos: usize, class: MutationClass, tok: u32, text: &str) -> Segment {
        Segment {
            id: SegmentId(id), kind: SegmentKind::FileRead, role: Role::Tool,
            bytes: text.as_bytes().to_vec(), token_count: tok, position: pos,
            mutation_class: class, origin: Origin::default(),
            protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn no_budget_means_no_eviction() {
        let segs = vec![kb(0, 0, MutationClass::Fast, 100, "anything")];
        let plan = Planner::new(vec![]).plan_with_budget(&segs, &SessionState::default(), &TaskSignal::empty(), None);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn evicts_lowest_priority_until_under_budget_keeping_frozen_and_relevant() {
        let task = TaskSignal::from_text("authentication jwt");
        let segs = vec![
            kb(0, 0, MutationClass::Frozen, 100, "system prompt"),          // frozen: never evicted
            kb(1, 1, MutationClass::Fast,   100, "jwt authentication code"), // relevant: keep
            kb(2, 2, MutationClass::Fast,   100, "old irrelevant logs aaa"), // irrelevant, oldest fast
            kb(3, 3, MutationClass::Fast,   100, "more irrelevant bbb"),     // irrelevant, newer fast
        ];
        // total 400; budget 250 -> must drop ~150 worth. Evict irrelevant, oldest-first.
        let plan = Planner::new(vec![]).plan_with_budget(&segs, &SessionState::default(), &task, Some(250));
        assert_eq!(plan.entries[0].action, SegmentAction::Keep, "frozen never evicted");
        assert_eq!(plan.entries[1].action, SegmentAction::Keep, "task-relevant kept");
        assert_eq!(plan.entries[2].action, SegmentAction::Drop(DropReason::Evicted), "oldest irrelevant evicted first");
        // net now 300? still > 250 -> next lowest priority (entry 3) also evicted
        assert_eq!(plan.entries[3].action, SegmentAction::Drop(DropReason::Evicted));
        assert!(crate::plan::net_tokens(&plan, &segs) <= 250);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-core planner::`
Expected: FAIL (`plan_with_budget` not defined).

- [ ] **Step 3: Implement the eviction stage**

In `planner.rs`, refactor `Planner` so `plan_with_budget` is the core and the others delegate:
```rust
impl Planner {
    pub fn new(passes: Vec<Box<dyn Pass>>) -> Self { Self { passes } }

    pub fn plan(&self, segments: &[Segment], session: &SessionState) -> CompressionPlan {
        self.plan_with_task(segments, session, &crate::task::TaskSignal::empty())
    }

    pub fn plan_with_task(&self, segments: &[Segment], session: &SessionState, task: &crate::task::TaskSignal) -> CompressionPlan {
        self.plan_with_budget(segments, session, task, None)
    }

    /// Plan, then (if a budget is set) evict lowest-priority non-frozen survivors until net <= budget.
    pub fn plan_with_budget(
        &self,
        segments: &[Segment],
        session: &SessionState,
        task: &crate::task::TaskSignal,
        budget: Option<u32>,
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
        if let Some(b) = budget { evict_to_budget(&mut actions, segments, task, b); }
        CompressionPlan {
            entries: segments.iter().zip(actions).map(|(s, a)| PlanEntry { id: s.id, action: a }).collect(),
        }
    }
}

/// Tokens a single action currently emits.
fn action_tokens(action: &SegmentAction, seg: &Segment) -> u32 {
    match action {
        SegmentAction::Keep => seg.token_count,
        SegmentAction::Drop(_) => 0,
        SegmentAction::Replace { token_count, .. } => *token_count,
    }
}

/// Higher = keep. Task-relevant segments dominate; among equals, more recent (higher position) wins.
fn eviction_priority(seg: &Segment, task: &crate::task::TaskSignal) -> u64 {
    let relevant = if task.is_empty() {
        false
    } else {
        let syms = crate::task::extract_symbols(&String::from_utf8_lossy(&seg.bytes));
        !syms.is_disjoint(&task.symbols)
    };
    (relevant as u64) * 1_000_000_000 + seg.position as u64
}

/// Drop lowest-priority non-frozen survivors until net <= budget (spec §7 C, §8 Rule 4).
fn evict_to_budget(actions: &mut [SegmentAction], segments: &[Segment], task: &crate::task::TaskSignal, budget: u32) {
    let mut net: u32 = actions.iter().zip(segments).map(|(a, s)| action_tokens(a, s)).sum();
    if net <= budget { return; }
    // candidate indices: non-frozen, currently surviving (Keep or Replace)
    let mut cands: Vec<usize> = (0..segments.len())
        .filter(|&i| segments[i].mutation_class != MutationClass::Frozen
            && !matches!(actions[i], SegmentAction::Drop(_)))
        .collect();
    // ascending priority => lowest priority (old + irrelevant) evicted first
    cands.sort_by_key(|&i| eviction_priority(&segments[i], task));
    for i in cands {
        if net <= budget { break; }
        let saved = action_tokens(&actions[i], &segments[i]);
        actions[i] = SegmentAction::Drop(DropReason::Evicted);
        net -= saved;
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-core planner::`
Expected: PASS (the two eviction tests + all prior planner tests, which call `plan`/`plan_with_task` and so pass `budget = None`).

- [ ] **Step 5: Commit**

```bash
git add crates/cull-core
git commit -m "feat(core): budget-driven eviction (lowest-priority non-frozen first, task+recency)"
```

---

### Task 2: CLI `--budget`

**Files:** Modify `crates/cull-cli/src/lib.rs` and `crates/cull-cli/src/main.rs`; test inline.

- [ ] **Step 1: Write the failing test**

Add to `crates/cull-cli/src/lib.rs` test module:
```rust
    #[test]
    fn run_compress_with_budget_evicts_to_fit() {
        // three ~unrelated file reads; a tiny budget forces eviction beyond the structural passes
        let json = r#"[
            {"role":"tool","kind":"file_read","path":"a.rs","text":"alpha alpha alpha alpha alpha"},
            {"role":"tool","kind":"file_read","path":"b.rs","text":"beta beta beta beta beta beta"},
            {"role":"tool","kind":"file_read","path":"c.rs","text":"gamma gamma gamma gamma gamma"}
        ]"#;
        let unbudgeted = run_compress_with_budget(json, "alpha", None).unwrap();
        let budgeted = run_compress_with_budget(json, "alpha", Some(8)).unwrap();
        assert!(budgeted.report.net_tokens <= 8 || budgeted.report.net_tokens < unbudgeted.report.net_tokens,
            "budget forces additional eviction");
        assert!(budgeted.report.dropped >= unbudgeted.report.dropped);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-cli run_compress_with_budget`
Expected: FAIL (`run_compress_with_budget` not defined).

- [ ] **Step 3: Implement the budget overload**

In `crates/cull-cli/src/lib.rs`, make `run_compress` delegate and add the budgeted variant:
```rust
pub fn run_compress(blocks_json: &str, task: &str) -> Result<CompressOutput, String> {
    run_compress_with_budget(blocks_json, task, None)
}

pub fn run_compress_with_budget(blocks_json: &str, task: &str, budget: Option<u32>) -> Result<CompressOutput, String> {
    let blocks = parse_blocks(blocks_json)?;
    let counter = ApproxCounter::o200k();
    let segs = segment(&blocks, &counter);

    let mut passes = structural_passes();
    passes.extend(query_passes());
    let task_sig = TaskSignal::from_text(task);

    let plan = Planner::new(passes).plan_with_budget(&segs, &SessionState::default(), &task_sig, budget);
    let (emitted, report) = emit(&segs, &plan);

    let compressed = emitted.iter()
        .map(|e| String::from_utf8_lossy(&e.bytes).into_owned())
        .collect::<Vec<_>>()
        .join("\n---\n");

    Ok(CompressOutput { compressed, report })
}
```
(Move the body of the old `run_compress` into `run_compress_with_budget`; `run_compress` now just delegates with `None`.)

- [ ] **Step 4: Add the `--budget` flag to main.rs**

In `crates/cull-cli/src/main.rs`, add a `--budget` option to the `Compress` command and call the budgeted variant:
```rust
    Compress {
        #[arg(long, default_value = "")]
        task: String,
        #[arg(long, default_value_t = false)]
        report: bool,
        /// Optional hard token budget; evict lowest-priority context to fit.
        #[arg(long)]
        budget: Option<u32>,
    },
```
and in the match arm, replace `run_compress(&input, &task)` with `cull_cli::run_compress_with_budget(&input, &task, budget)` (destructure `budget` from the command).

- [ ] **Step 5: Run tests + smoke + commit**

Run: `cargo test --workspace` → Expected: PASS.
Smoke (from `/Users/mark/git/cull`):
```bash
echo '[{"role":"tool","kind":"file_read","path":"a.rs","text":"alpha alpha alpha alpha"},{"role":"tool","kind":"file_read","path":"b.rs","text":"beta beta beta beta"}]' | cargo run -q -p cull-cli -- compress --task "alpha" --budget 6
```
Expected: stderr `[cull]` line with `net <= 6`-ish (eviction happened); the relevant `alpha` read survives over `beta`.
```bash
git add crates/cull-cli
git commit -m "feat(cli): --budget flag (budget-driven eviction)"
```

---

## Self-Review

**1. Spec coverage:**
- §7 C (eviction) + §8 Rule 4 (frozen prefix never evicted) → Task 1. ✓
- Task-relevance + recency priority approximates §7 C1 Belady-oracle; ARC/phase-decay are flagged later refinements. ✓
- CLI budget surface → Task 2. ✓

**2. Placeholder scan:** No vague steps. Belady/ARC refinements are explicit design notes, not in-plan gaps. ✓

**3. Type consistency:** `plan_with_budget` (Task 1) called by `run_compress_with_budget` (Task 2) and `main.rs`. `plan`/`plan_with_task` still delegate (Plan 2–8 callers unaffected). `action_tokens`/`eviction_priority`/`evict_to_budget` are private helpers in `planner.rs`. `TaskSignal`/`extract_symbols` from Plan 4; `MutationClass`/`SegmentAction`/`DropReason::Evicted` from Plans 1–2. ✓

**4. Ambiguity check:** No budget → no eviction (Keep). Frozen never a candidate (cached prefix sacred). Already-dropped segments aren't re-evicted. Priority: relevant dominates (1e9 weight) then position; ascending sort evicts old-irrelevant first. Eviction only adds Drops to non-frozen survivors, which can't violate I1/I3/I4 — so it safely runs after `enforce_invariants`. ✓

**Outcome:** The engine can now compress to a hard token budget, keeping what the task needs and the cached prefix intact. Next: the production proxy that wires this whole engine (with a real budget from the model's context limit) into live API traffic, then the benchmark.
