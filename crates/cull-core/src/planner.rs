use crate::plan::{CompressionPlan, PlanEntry, SegmentAction};
use crate::segment::{MutationClass, Segment, SegmentId};
use crate::session::SessionState;

/// Read-only context a pass sees. Later plans add the task signal, cache model, and budget.
pub struct PlanCtx<'a> {
    pub segments: &'a [Segment],
    pub session: &'a SessionState,
}

/// A pass proposes actions for some segments. Unmentioned segments stay Keep. Passes run in
/// registration order; a later pass's proposal for a segment overrides an earlier one.
pub trait Pass {
    fn name(&self) -> &'static str;
    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry>;
}

pub struct Planner { passes: Vec<Box<dyn Pass>> }

impl Planner {
    pub fn new(passes: Vec<Box<dyn Pass>>) -> Self { Self { passes } }

    pub fn plan(&self, segments: &[Segment], session: &SessionState) -> CompressionPlan {
        let mut actions: Vec<SegmentAction> = vec![SegmentAction::Keep; segments.len()];
        let index: std::collections::HashMap<SegmentId, usize> =
            segments.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
        let ctx = PlanCtx { segments, session };
        for pass in &self.passes {
            for entry in pass.propose(&ctx) {
                if let Some(&i) = index.get(&entry.id) { actions[i] = entry.action; }
            }
        }
        enforce_invariants(&mut actions, segments);
        CompressionPlan {
            entries: segments.iter().zip(actions).map(|(s, a)| PlanEntry { id: s.id, action: a }).collect(),
        }
    }
}

/// Order ids by ascending mutation frequency (Frozen, Slow, Fast), preserving position within a class.
pub fn stability_order(segments: &[Segment]) -> Vec<SegmentId> {
    fn rank(c: MutationClass) -> u8 { match c { MutationClass::Frozen => 0, MutationClass::Slow => 1, MutationClass::Fast => 2 } }
    let mut idx: Vec<usize> = (0..segments.len()).collect();
    idx.sort_by_key(|&i| (rank(segments[i].mutation_class), segments[i].position));
    idx.into_iter().map(|i| segments[i].id).collect()
}

/// Placeholder — TASK 4 replaces this with real I1/I3/I4 enforcement.
fn enforce_invariants(_actions: &mut [SegmentAction], _segments: &[Segment]) {}

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
        let plan = Planner::new(vec![]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries.len(), 2);
        assert!(plan.entries.iter().all(|e| e.action == SegmentAction::Keep));
    }

    #[test]
    fn pass_proposals_are_applied() {
        let segs = vec![seg(0, MutationClass::Fast), seg(1, MutationClass::Fast)];
        let plan = Planner::new(vec![Box::new(DropPass { ids: vec![1] })]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(plan.entries[1].action, SegmentAction::Drop(DropReason::Duplicate));
    }

    #[test]
    fn stability_order_sorts_frozen_before_fast_then_by_position() {
        let segs = vec![
            seg(0, MutationClass::Fast), seg(1, MutationClass::Frozen),
            seg(2, MutationClass::Slow), seg(3, MutationClass::Frozen),
        ];
        let order = stability_order(&segs);
        assert_eq!(order, vec![SegmentId(1), SegmentId(3), SegmentId(2), SegmentId(0)]);
    }
}
