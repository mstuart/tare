pub mod supersession;
pub mod dedup;
pub mod relevance;
pub mod ivm;

pub use supersession::SupersessionPass;
pub use dedup::ExactDedupPass;
pub use relevance::RelevancePass;
pub use ivm::IvmDeltaPass;

use crate::planner::Pass;

/// The default structural pass pipeline, in run order: supersession (drops obsolete tool outputs),
/// IVM/delta (changed re-reads become unified diffs), then exact dedup (drops identical leftovers).
pub fn structural_passes() -> Vec<Box<dyn Pass>> {
    vec![Box::new(SupersessionPass), Box::new(IvmDeltaPass::new()), Box::new(ExactDedupPass)]
}

/// The default query-conditioned pass pipeline. Currently the deterministic RelevancePass;
/// PRF and embedding-salience passes are added here in a later plan.
pub fn query_passes() -> Vec<Box<dyn Pass>> {
    vec![Box::new(RelevancePass::default())]
}

#[cfg(test)]
mod tests {
    use crate::segment::*;
    use crate::plan::{SegmentAction, net_tokens, input_tokens};
    use crate::planner::Planner;
    use crate::session::SessionState;

    fn seg(id: u64, kind: SegmentKind, class_or_text: &str) -> Segment {
        Segment {
            id: SegmentId(id), kind, role: Role::Tool, bytes: class_or_text.as_bytes().to_vec(),
            token_count: 10, position: id as usize, mutation_class: MutationClass::Fast,
            origin: Origin::default(), protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn structural_pipeline_compresses_and_never_increases() {
        // two superseded builds + one exact-duplicate file read
        let segs = vec![
            seg(0, SegmentKind::ToolOutput { class: "cargo-test".into() }, "build-old"),
            seg(1, SegmentKind::FileRead, "FILEDATA"),
            seg(2, SegmentKind::ToolOutput { class: "cargo-test".into() }, "build-new"),
            seg(3, SegmentKind::FileRead, "FILEDATA"), // exact dup of id 1
        ];
        let before = input_tokens(&segs); // 40
        let plan = Planner::new(super::structural_passes()).plan(&segs, &SessionState::default());
        let after = net_tokens(&plan, &segs);
        assert!(after < before, "pipeline must reduce tokens: {after} < {before}");
        // id 0 superseded, id 3 deduped; ids 1 and 2 kept => 20 tokens
        assert_eq!(after, 20);
        assert_eq!(plan.entries[0].action, SegmentAction::Drop(crate::plan::DropReason::Superseded));
        assert_eq!(plan.entries[3].action, SegmentAction::Drop(crate::plan::DropReason::Duplicate));
    }

    #[test]
    fn structural_passes_returns_both_passes() {
        let passes = super::structural_passes();
        assert_eq!(passes.len(), 3);
    }

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
        assert!(net_tokens(&plan, &[base.clone(), reread.clone()]) < input_tokens(&[base, reread]));
    }

    #[test]
    fn structural_passes_has_three_passes() {
        assert_eq!(super::structural_passes().len(), 3);
    }
}

#[cfg(test)]
mod query_tests {
    use crate::segment::*;
    use crate::plan::{SegmentAction, net_tokens, input_tokens};
    use crate::planner::Planner;
    use crate::session::SessionState;
    use crate::task::TaskSignal;

    fn seg(id: u64, pos: usize, text: &str) -> Segment {
        Segment {
            id: SegmentId(id), kind: SegmentKind::FileRead, role: Role::Tool,
            bytes: text.as_bytes().to_vec(), token_count: 10, position: pos,
            mutation_class: MutationClass::Fast, origin: Origin::default(),
            protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn query_pipeline_drops_irrelevant_and_never_increases() {
        let task = TaskSignal::from_text("authentication jwt");
        let segs = vec![
            seg(0, 0, "jwt authentication handler"),  // relevant, old — kept (overlap)
            seg(1, 1, "kubernetes deployment yaml"),  // irrelevant, old — dropped
            seg(2, 20, "grafana dashboard metrics"),  // irrelevant, recent — kept (recency)
        ];
        let before = input_tokens(&segs);
        let plan = Planner::new(super::query_passes()).plan_with_task(&segs, &SessionState::default(), &task);
        let after = net_tokens(&plan, &segs);
        assert!(after < before);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn query_passes_returns_relevance_pass() {
        assert_eq!(super::query_passes().len(), 1);
    }
}
