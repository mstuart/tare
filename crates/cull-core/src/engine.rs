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
