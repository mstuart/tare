pub mod supersession;
pub mod dedup;
pub mod relevance;

pub use supersession::SupersessionPass;
pub use dedup::ExactDedupPass;
pub use relevance::RelevancePass;

use crate::planner::Pass;

/// The default Drop-based structural pass pipeline, in run order. Supersession first (drops
/// obsolete tool outputs), then exact dedup (drops identical leftovers). Replace-based passes
/// (delta/IVM, RePair) are added in a later plan alongside the emitter.
pub fn structural_passes() -> Vec<Box<dyn Pass>> {
    vec![Box::new(SupersessionPass), Box::new(ExactDedupPass)]
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
        assert_eq!(passes.len(), 2);
    }
}
