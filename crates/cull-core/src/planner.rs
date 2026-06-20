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

pub struct Planner { passes: Vec<Box<dyn Pass>>, min_keep_ratio: Option<f64> }

impl Planner {
    pub fn new(passes: Vec<Box<dyn Pass>>) -> Self { Self { passes, min_keep_ratio: None } }

    /// Quality floor (spec I6): eviction will not reduce net below `ratio * input` tokens.
    pub fn with_floor(mut self, ratio: f64) -> Self { self.min_keep_ratio = Some(ratio); self }

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
        if let Some(b) = budget {
            let input: u32 = segments.iter().map(|s| s.token_count).sum();
            let floor_tokens = self.min_keep_ratio
                .map(|r| (r * input as f64).ceil() as u32)
                .unwrap_or(0);
            let effective = b.max(floor_tokens);
            evict_to_budget(&mut actions, segments, task, effective);
        }
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

/// Drop lowest-priority non-frozen survivors until net <= budget (spec §7 C1/C2, §8 Rule 4).
///
/// Priority = future-need (dominant) × frequency × recency.
///   C1 future-need: segment intersects task symbols ∪ symbols of any CompactSummary (running plan/state).
///   C2 frequency: how many other segments co-reference its symbols (frequently-referenced = important).
///   Phase/recency: position — earlier = discovery phase, decays/evicts first.
/// Lowest priority evicted first; frozen never evicted.
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
        if matches!(s.kind, crate::segment::SegmentKind::CompactSummary) {
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

/// Order ids by ascending mutation frequency (Frozen, Slow, Fast), preserving position within a class.
pub fn stability_order(segments: &[Segment]) -> Vec<SegmentId> {
    fn rank(c: MutationClass) -> u8 { match c { MutationClass::Frozen => 0, MutationClass::Slow => 1, MutationClass::Fast => 2 } }
    let mut idx: Vec<usize> = (0..segments.len()).collect();
    idx.sort_by_key(|&i| (rank(segments[i].mutation_class), segments[i].position));
    idx.into_iter().map(|i| segments[i].id).collect()
}

/// Enforce plan invariants (spec §9): I3 frozen=Keep; I4/I5 a Replace must losslessly reconstruct
/// the original; I1 a Replace must strictly reduce tokens. Any violation reverts that entry to Keep.
/// After the per-entry loop, a fixpoint pass enforces base-protection: a Delta whose base is not
/// Keep in the final plan is unreconstructable and reverts to Keep.
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
    // Base protection: a Delta's base must be Keep in the final plan, else it's unreconstructable.
    loop {
        let kept: std::collections::HashSet<SegmentId> = segments.iter().zip(actions.iter())
            .filter(|(_, a)| matches!(a, SegmentAction::Keep))
            .map(|(s, _)| s.id).collect();
        let mut changed = false;
        for (a, _s) in actions.iter_mut().zip(segments.iter()) {
            if let SegmentAction::Replace { reconstruct: crate::plan::Reconstruct::Delta { base }, .. } = a {
                if !kept.contains(base) { *a = SegmentAction::Keep; changed = true; }
            }
        }
        if !changed { break; }
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

    #[test]
    fn quality_floor_prevents_over_compression() {
        let task = TaskSignal::from_text("nothing relevant here");
        // 4 fast segments, 100 tokens each = 400; tiny budget would evict almost all,
        // but a 0.5 floor keeps >= 200 tokens.
        let segs = vec![
            kb(0, 0, MutationClass::Fast, 100, "aaa"), kb(1, 1, MutationClass::Fast, 100, "bbb"),
            kb(2, 2, MutationClass::Fast, 100, "ccc"), kb(3, 3, MutationClass::Fast, 100, "ddd"),
        ];
        let plan = Planner::new(vec![]).with_floor(0.5)
            .plan_with_budget(&segs, &SessionState::default(), &task, Some(10));
        let net = crate::plan::net_tokens(&plan, &segs);
        assert!(net >= 200, "quality floor keeps >= 50% of input: net={net}");
    }

    #[test]
    fn no_floor_evicts_to_budget_as_before() {
        let task = TaskSignal::from_text("x");
        let segs = vec![kb(0,0,MutationClass::Fast,100,"a"), kb(1,1,MutationClass::Fast,100,"b")];
        let plan = Planner::new(vec![]).plan_with_budget(&segs, &SessionState::default(), &task, Some(100));
        assert!(crate::plan::net_tokens(&plan, &segs) <= 100);
    }

    // pass: drop seg0 AND delta seg1 against seg0 — seg1's base is gone, so it must revert to Keep.
    struct DropBaseAndDelta;
    impl Pass for DropBaseAndDelta {
        fn name(&self) -> &'static str { "drop-base-and-delta" }
        fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
            let s0 = &ctx.segments[0]; let s1 = &ctx.segments[1];
            let patch = diffy::create_patch(
                std::str::from_utf8(&s0.bytes).unwrap(), std::str::from_utf8(&s1.bytes).unwrap()).to_string();
            vec![
                PlanEntry { id: s0.id, action: SegmentAction::Drop(DropReason::Evicted) },
                PlanEntry { id: s1.id, action: SegmentAction::Replace {
                    rendered: patch.into_bytes(), token_count: 1,
                    reconstruct: Reconstruct::Delta { base: s0.id }, reason: DropReason::Duplicate } },
            ]
        }
    }

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

    #[test]
    fn delta_against_dropped_base_reverts_to_keep() {
        let mut a = seg(0, MutationClass::Fast); a.bytes = b"alpha\nbeta\ngamma\n".to_vec();
        let mut b = seg(1, MutationClass::Fast); b.bytes = b"alpha\nBETA\ngamma\n".to_vec();
        let plan = Planner::new(vec![Box::new(DropBaseAndDelta)]).plan(&[a.clone(), b.clone()], &SessionState::default());
        // base seg0 dropped -> seg1's delta has no base -> reverted to Keep (reconstructable)
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);
    }
}
