# Cull — Plan 25: Session-State Threading + Internal-Mode Engine (§6)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Close the §6 ⚠️ "session-state types exist but are never threaded." Make `SessionState` genuinely **consumed** by a pass, and add the **internal-mode** `SessionEngine` that accumulates state across turns and plans against it.

**Background (verified):** No pass reads `ctx.session` today — every pass operates on `ctx.segments`/`ctx.task`, achieving cross-turn behavior from the in-request full history (correct for the stateless hosted-model proxy). The persisted `SessionState` (`tools` registry, `files` store, `prefix` commitment) is for **internal mode** — an embedded driver that retains state between turns without resending history. This plan makes that mode real.

**Architecture:** (1) `SupersessionPass` additionally consults `ctx.session.tools`: a `ToolOutput` is superseded if the registry recorded a **newer turn** for its class — so an old output is dropped even when the newer run isn't in the visible slice. Default/empty state ⇒ unchanged behavior (the proxy is unaffected). (2) A new `SessionEngine` (cull-core) accumulates `tools`/`files`/`prefix` across turns and exposes `plan(...)`. The IVM delta still bases off in-request segments (a cross-store delta would need a new stored-base `Reconstruct` variant — explicitly out of scope here, noted in the ledger).

**Tech:** Rust. Builds on `session.rs`, `planner.rs`, `passes/supersession.rs`. Reference: spec §6, §7 A1, §10 ("internal mode").

---

### Task 1: SupersessionPass consults the persisted registry

**Files:** modify `crates/cull-core/src/passes/supersession.rs` (impl + tests inline).

- [ ] **Step 1 — failing tests.** Add to the `tests` module:
```rust
    #[test]
    fn registry_supersedes_old_output_not_in_slice() {
        let mut session = SessionState::default();
        session.tools.record("cargo-test", 9, Some(0)); // a newer run recorded out-of-band (turn 9)
        let mut s = tool_seg(0, "cargo-test");
        s.origin.turn = 2;                               // this output predates the recorded run
        let plan = Planner::new(vec![Box::new(SupersessionPass)]).plan(&[s], &session);
        assert_eq!(plan.entries[0].action, SegmentAction::Drop(DropReason::Superseded));
    }

    #[test]
    fn registry_keeps_output_newer_than_recorded_run() {
        let mut session = SessionState::default();
        session.tools.record("cargo-test", 1, Some(0)); // recorded run is OLDER than the output
        let mut s = tool_seg(0, "cargo-test");
        s.origin.turn = 5;
        let plan = Planner::new(vec![Box::new(SupersessionPass)]).plan(&[s], &session);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }
```
(Keep all existing tests — with a `default()` session the registry is empty, so they behave exactly as before.)

- [ ] **Step 2 — confirm FAIL** (`cargo test -p cull-core supersession` — the registry path doesn't exist yet, old output is kept).

- [ ] **Step 3 — implement.** Replace the `propose` body's final `filter_map` so it also checks the registry:
```rust
    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        // latest position per class WITHIN this request
        let mut latest: HashMap<&str, usize> = HashMap::new();
        for s in ctx.segments {
            if let SegmentKind::ToolOutput { class } = &s.kind {
                let e = latest.entry(class.as_str()).or_insert(s.position);
                if s.position > *e { *e = s.position; }
            }
        }
        // drop earlier same-class outputs (in-request OR a newer run in the persisted registry)
        ctx.segments.iter().filter_map(|s| {
            if let SegmentKind::ToolOutput { class } = &s.kind {
                let class = class.as_str();
                let in_request_superseded = s.position < latest[class];
                let registry_superseded = ctx.session.tools.latest_run(class)
                    .map(|run| run.turn > s.origin.turn)
                    .unwrap_or(false);
                if in_request_superseded || registry_superseded {
                    return Some(PlanEntry { id: s.id, action: SegmentAction::Drop(DropReason::Superseded) });
                }
            }
            None
        }).collect()
    }
```

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-core supersession` — new + existing).

- [ ] **Step 5 — commit.** `cargo test --workspace` green; `git add crates/cull-core && git commit -m "feat(core): supersession consults persisted ToolClassRegistry (session threading)"`

---

### Task 2: SessionEngine (internal-mode driver)

**Files:** create `crates/cull-core/src/engine.rs`; modify `crates/cull-core/src/lib.rs` (`pub mod engine;`).

- [ ] **Step 1 — failing test.** Create `crates/cull-core/src/engine.rs` with the test module:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::passes::SupersessionPass;
    use crate::segment::*;
    use crate::plan::{SegmentAction, DropReason};
    use crate::task::TaskSignal;

    fn tool_seg(id: u64, class: &str, turn: usize) -> Segment {
        Segment {
            id: SegmentId(id), kind: SegmentKind::ToolOutput { class: class.into() },
            role: Role::Tool, bytes: format!("output {id}").into_bytes(), token_count: 10,
            position: id as usize, mutation_class: MutationClass::Fast,
            origin: Origin { turn, ..Origin::default() },
            protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn accumulated_registry_supersedes_across_turns() {
        let mut eng = SessionEngine::new();
        eng.begin_turn();                                  // turn 1
        eng.record_tool_run("cargo-test", Some(0));        // a fresh cargo-test ran this turn
        // now plan over an OLD cargo-test output (turn 0), the only output in the slice
        let old = tool_seg(0, "cargo-test", 0);
        let plan = eng.plan(vec![Box::new(SupersessionPass)], &[old], &TaskSignal::empty(), None);
        assert_eq!(plan.entries[0].action, SegmentAction::Drop(DropReason::Superseded));
    }

    #[test]
    fn fresh_engine_keeps_lone_output() {
        let eng = SessionEngine::new();                    // empty state
        let s = tool_seg(0, "cargo-test", 0);
        let plan = eng.plan(vec![Box::new(SupersessionPass)], &[s], &TaskSignal::empty(), None);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn accumulates_file_snapshots() {
        let mut eng = SessionEngine::new();
        eng.record_file_read("src/a.rs", b"v0".to_vec(), 1);
        eng.record_file_read("src/a.rs", b"v1".to_vec(), 1);
        assert_eq!(eng.state().files.get("src/a.rs").unwrap().version, 1);
    }
}
```

- [ ] **Step 2 — confirm FAIL** (`cargo test -p cull-core engine` — `SessionEngine` missing).

- [ ] **Step 3 — implement** at the top of `engine.rs`:
```rust
use crate::session::SessionState;
use crate::segment::Segment;
use crate::planner::{Planner, Pass};
use crate::plan::CompressionPlan;
use crate::task::TaskSignal;

/// Internal/stateful-mode driver (spec §6 + §10 "internal mode"). Accumulates `SessionState`
/// across turns and plans against it. The hosted-model proxy is stateless (full history is resent
/// each turn, so passes reconstruct state from the request); this driver is for embedded use where
/// the engine retains state between turns and can supersede/delta against content no longer in the
/// visible slice.
pub struct SessionEngine {
    state: SessionState,
    turn: usize,
}

impl SessionEngine {
    pub fn new() -> Self { Self { state: SessionState::default(), turn: 0 } }

    /// Advance the monotonic turn clock the registries key on. Returns the new turn.
    pub fn begin_turn(&mut self) -> usize { self.turn += 1; self.turn }

    /// Record a tool run for supersession (A1) at the current turn.
    pub fn record_tool_run(&mut self, class: &str, exit_code: Option<i32>) {
        self.state.tools.record(class, self.turn, exit_code);
    }

    /// Record a canonical file snapshot for IVM (A2).
    pub fn record_file_read(&mut self, path: &str, bytes: Vec<u8>, token_count: u32) {
        self.state.files.put(path, bytes, token_count);
    }

    /// Commit the frozen cache prefix (Rule 1/7).
    pub fn commit_prefix(&mut self, frozen_bytes: &[u8], frozen_len_tokens: usize) {
        self.state.prefix.commit(frozen_bytes, frozen_len_tokens);
    }

    pub fn state(&self) -> &SessionState { &self.state }

    /// Plan compression for this turn's segments against the accumulated state.
    pub fn plan(&self, passes: Vec<Box<dyn Pass>>, segments: &[Segment], task: &TaskSignal,
                budget: Option<u32>) -> CompressionPlan {
        Planner::new(passes).plan_with_budget(segments, &self.state, task, budget)
    }
}

impl Default for SessionEngine { fn default() -> Self { Self::new() } }
```

- [ ] **Step 4 — wire the module.** Add `pub mod engine;` to `crates/cull-core/src/lib.rs` (near the other `pub mod`s).

- [ ] **Step 5 — confirm PASS** (`cargo test -p cull-core engine` then `cargo test --workspace`).

- [ ] **Step 6 — commit.** `git add crates/cull-core && git commit -m "feat(core): SessionEngine internal-mode driver — accumulates SessionState across turns"`

---

## After this plan — ledger
- ✅ §6 session-state threading — `SupersessionPass` consumes the persisted `ToolClassRegistry`; `SessionEngine` accumulates `tools`/`files`/`prefix` across turns and plans against them. Note: IVM delta still bases off in-request segments; cross-store deltas would need a stored-base `Reconstruct` variant (bounded follow-up, not claimed here). The hosted-model proxy stays stateless by design.

## Self-Review
- Default/empty `SessionState` ⇒ identical behavior ⇒ proxy + all existing tests unaffected. ✓
- Registry supersession uses `origin.turn` vs recorded `run.turn` — a Drop, so no reconstruction/base concerns. ✓
- `SessionEngine` makes the inert types genuinely load-bearing (a pass reads them; the driver populates them). ✓
- No over-claim: files/prefix are populated + exposed; only `tools` drives a pass decision today — stated honestly in the ledger. ✓
