use crate::plan::{CompressionPlan, DropReason, PlanEntry, SegmentAction};
use crate::segment::{MutationClass, Segment, SegmentId};
use crate::session::SessionState;

/// Read-only context a pass sees. Later plans add the cache model and budget.
pub struct PlanCtx<'a> {
    pub segments: &'a [Segment],
    pub session: &'a SessionState,
    pub task: &'a crate::task::TaskSignal,
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

    /// Plan with no task signal (query-conditioned passes are inert).
    pub fn plan(&self, segments: &[Segment], session: &SessionState) -> CompressionPlan {
        self.plan_with_task(segments, session, &crate::task::TaskSignal::empty())
    }

    /// Plan conditioned on the current task (no eviction).
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

/// Order ids by ascending mutation frequency (Frozen, Slow, Fast), preserving position within a class.
pub fn stability_order(segments: &[Segment]) -> Vec<SegmentId> {
    fn rank(c: MutationClass) -> u8 { match c { MutationClass::Frozen => 0, MutationClass::Slow => 1, MutationClass::Fast => 2 } }
    let mut idx: Vec<usize> = (0..segments.len()).collect();
    idx.sort_by_key(|&i| (rank(segments[i].mutation_class), segments[i].position));
    idx.into_iter().map(|i| segments[i].id).collect()
}

/// Enforce plan invariants (spec §9): I3 frozen=Keep; I4/I5 a Replace must losslessly reconstruct
/// the original; I1 a Replace must strictly reduce tokens. Any violation reverts that entry to Keep.
fn enforce_invariants(actions: &mut [SegmentAction], segments: &[Segment]) {
    let by_id: std::collections::HashMap<SegmentId, &Segment> =
        segments.iter().map(|s| (s.id, s)).collect();
    for (a, s) in actions.iter_mut().zip(segments.iter()) {
        if s.mutation_class == MutationClass::Frozen && *a != SegmentAction::Keep {
            *a = SegmentAction::Keep;
            continue;
        }
        if let SegmentAction::Replace { token_count, .. } = a {
            let reduces = *token_count < s.token_count;
            let lossless = crate::plan::replace_is_lossless(s, a, &by_id);
            if !reduces || !lossless { *a = SegmentAction::Keep; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use crate::plan::{SegmentAction, DropReason};
    use crate::session::SessionState;
    use crate::plan::Reconstruct;
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

    // Replace that increases tokens (violates I1) — uses a real valid delta so only I1 fails.
    struct BloatPass;
    impl Pass for BloatPass {
        fn name(&self) -> &'static str { "bloat" }
        fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
            ctx.segments.iter().map(|s| {
                let patch = diffy::create_patch(
                    std::str::from_utf8(&s.bytes).unwrap_or(""),
                    "anything",
                ).to_string();
                PlanEntry { id: s.id, action: SegmentAction::Replace {
                    rendered: patch.into_bytes(), token_count: s.token_count + 50,
                    reconstruct: Reconstruct::Delta { base: s.id }, reason: DropReason::Duplicate } }
            }).collect()
        }
    }

    // Replace whose delta does NOT reconstruct the original (violates I4/I5).
    struct LossyPass;
    impl Pass for LossyPass {
        fn name(&self) -> &'static str { "lossy" }
        fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
            ctx.segments.iter().map(|s| PlanEntry { id: s.id, action: SegmentAction::Replace {
                rendered: b"not a valid patch".to_vec(), token_count: 1,
                reconstruct: Reconstruct::Delta { base: SegmentId(999) }, reason: DropReason::Duplicate } }).collect()
        }
    }

    struct DropFrozenPass;
    impl Pass for DropFrozenPass {
        fn name(&self) -> &'static str { "drop-frozen" }
        fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
            ctx.segments.iter().map(|s| PlanEntry { id: s.id, action: SegmentAction::Drop(DropReason::Evicted) }).collect()
        }
    }

    #[test]
    fn i1_reverts_token_increasing_replace_to_keep() {
        let segs = vec![seg(0, MutationClass::Fast)];
        let plan = Planner::new(vec![Box::new(BloatPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(crate::plan::net_tokens(&plan, &segs), 10);
    }

    #[test]
    fn lossy_replace_is_reverted_to_keep() {
        let segs = vec![seg(0, MutationClass::Fast)];
        let plan = Planner::new(vec![Box::new(LossyPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn valid_delta_replace_is_kept() {
        // Two segments: base (id 0) and target (id 1). The Replace on target uses a diff that,
        // when applied to base.bytes, reconstructs target.bytes exactly. Tokens reduce. Survives.
        let mut base = seg(0, MutationClass::Fast);
        base.bytes = b"alpha\nbeta\ngamma\n".to_vec();
        let mut target = seg(1, MutationClass::Fast);
        target.bytes = b"alpha\nBETA\ngamma\n".to_vec();
        let patch = diffy::create_patch("alpha\nbeta\ngamma\n", "alpha\nBETA\ngamma\n").to_string();
        struct ValidDelta { patch: String }
        impl Pass for ValidDelta {
            fn name(&self) -> &'static str { "valid-delta" }
            fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
                let target = &ctx.segments[1];
                let base = &ctx.segments[0];
                vec![PlanEntry { id: target.id, action: SegmentAction::Replace {
                    rendered: self.patch.clone().into_bytes(), token_count: 1,
                    reconstruct: crate::plan::Reconstruct::Delta { base: base.id }, reason: DropReason::Duplicate } }]
            }
        }
        let plan = Planner::new(vec![Box::new(ValidDelta { patch })]).plan(&[base.clone(), target.clone()], &SessionState::default());
        assert!(matches!(plan.entries[1].action, SegmentAction::Replace { .. }), "valid lossless reducing delta is kept");
    }

    #[test]
    fn i3_reverts_drop_of_frozen_segment_to_keep() {
        let segs = vec![seg(0, MutationClass::Frozen), seg(1, MutationClass::Fast)];
        let plan = Planner::new(vec![Box::new(DropFrozenPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(plan.entries[1].action, SegmentAction::Drop(DropReason::Evicted));
    }
}
