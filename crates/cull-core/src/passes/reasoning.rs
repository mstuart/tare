use crate::plan::{DropReason, PlanEntry, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::SegmentKind;

const CONCLUSION_MARKERS: &[&str] = &[
    "therefore",
    "the fix",
    "root cause",
    "conclusion",
    "decided",
    "so i",
    "in summary",
    "the bug is",
    "the issue is",
    "confirmed",
];

/// Prune reasoning/scratch-work (spec §7 B4): drop ReasoningTrace blocks older than `recency_keep`
/// that contain no conclusion marker. Recent reasoning and conclusion-bearing blocks are kept.
pub struct ReasoningTracePass {
    pub recency_keep: usize,
}

impl Default for ReasoningTracePass {
    fn default() -> Self {
        Self { recency_keep: 3 }
    }
}

impl Pass for ReasoningTracePass {
    fn name(&self) -> &'static str {
        "reasoning-trace-prune"
    }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        let max_pos = ctx.segments.iter().map(|s| s.position).max().unwrap_or(0);
        ctx.segments
            .iter()
            .filter_map(|s| {
                if !matches!(s.kind, SegmentKind::ReasoningTrace) {
                    return None;
                }
                if max_pos.saturating_sub(s.position) < self.recency_keep {
                    return None;
                }
                let text = String::from_utf8_lossy(&s.bytes).to_ascii_lowercase();
                if CONCLUSION_MARKERS.iter().any(|m| text.contains(m)) {
                    return None;
                }
                Some(PlanEntry {
                    id: s.id,
                    action: SegmentAction::Drop(DropReason::Evicted),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{DropReason, SegmentAction};
    use crate::planner::Planner;
    use crate::segment::*;
    use crate::session::SessionState;

    fn rt(id: u64, pos: usize, text: &str) -> Segment {
        Segment {
            id: SegmentId(id),
            kind: SegmentKind::ReasoningTrace,
            role: Role::Assistant,
            bytes: text.as_bytes().to_vec(),
            token_count: 30,
            position: pos,
            mutation_class: MutationClass::Fast,
            origin: Origin::default(),
            protected_spans: vec![],
            refs: RefLedger::default(),
        }
    }

    #[test]
    fn drops_old_inconclusive_reasoning_keeps_conclusions_and_recent() {
        let segs = vec![
            rt(
                0,
                0,
                "maybe it's the cache? let me check, not sure, could be anything",
            ), // old scratch -> drop
            rt(1, 1, "therefore the fix is to reset the token on expiry"), // old but conclusion -> keep
            rt(2, 9, "let me try the next thing now"),                     // recent -> keep
        ];
        let plan = Planner::new(vec![Box::new(ReasoningTracePass { recency_keep: 3 })])
            .plan(&segs, &SessionState::default());
        assert_eq!(
            plan.entries[0].action,
            SegmentAction::Drop(DropReason::Evicted)
        );
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);
        assert_eq!(plan.entries[2].action, SegmentAction::Keep);
    }

    #[test]
    fn non_reasoning_segments_untouched() {
        let mut s = rt(0, 0, "anything");
        s.kind = SegmentKind::FileRead;
        let plan = Planner::new(vec![Box::new(ReasoningTracePass { recency_keep: 0 })])
            .plan(&[s], &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }
}
