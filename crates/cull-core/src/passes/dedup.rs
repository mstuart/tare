use crate::plan::{DropReason, PlanEntry, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::SegmentKind;
use std::collections::HashSet;

/// Drop later byte-identical copies of data segments (re-read unchanged file, repeated grep,
/// duplicate directory listing). The first occurrence is kept; later exact duplicates are
/// dropped (the content is still present once). Scoped to "data" kinds — never conversation,
/// reasoning, system prompt, or schemas. Whole-unit Drop (I4-exempt); I3 still protects frozen.
pub struct ExactDedupPass;

fn is_dedup_eligible(kind: &SegmentKind) -> bool {
    matches!(
        kind,
        SegmentKind::FileRead
            | SegmentKind::DirListing
            | SegmentKind::ToolOutput { .. }
            | SegmentKind::StackTrace
            | SegmentKind::TestOutput
    )
}

impl Pass for ExactDedupPass {
    fn name(&self) -> &'static str {
        "exact-dedup"
    }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        let mut seen: HashSet<&[u8]> = HashSet::new();
        let mut out = Vec::new();
        for s in ctx.segments {
            if !is_dedup_eligible(&s.kind) {
                continue;
            }
            if seen.contains(s.bytes.as_slice()) {
                out.push(PlanEntry {
                    id: s.id,
                    action: SegmentAction::Drop(DropReason::Duplicate),
                });
            } else {
                seen.insert(s.bytes.as_slice());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{DropReason, SegmentAction};
    use crate::planner::Planner;
    use crate::segment::*;
    use crate::session::SessionState;

    fn data_seg(id: u64, kind: SegmentKind, text: &str) -> Segment {
        Segment {
            id: SegmentId(id),
            kind,
            role: Role::Tool,
            bytes: text.as_bytes().to_vec(),
            token_count: 10,
            position: id as usize,
            mutation_class: MutationClass::Fast,
            origin: Origin::default(),
            protected_spans: vec![],
            refs: RefLedger::default(),
        }
    }

    #[test]
    fn drops_later_byte_identical_data_segment() {
        let segs = vec![
            data_seg(0, SegmentKind::FileRead, "same contents"),
            data_seg(1, SegmentKind::FileRead, "different"),
            data_seg(2, SegmentKind::FileRead, "same contents"), // exact dup of id 0
        ];
        let plan =
            Planner::new(vec![Box::new(ExactDedupPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep); // first occurrence kept
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);
        assert_eq!(
            plan.entries[2].action,
            SegmentAction::Drop(DropReason::Duplicate)
        ); // later dup dropped
    }

    #[test]
    fn does_not_dedup_non_data_kinds() {
        // two identical conversation turns are NOT dedup-eligible
        let segs = vec![
            data_seg(0, SegmentKind::ConversationTurn, "hello"),
            data_seg(1, SegmentKind::ConversationTurn, "hello"),
        ];
        let plan =
            Planner::new(vec![Box::new(ExactDedupPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);
    }
}
