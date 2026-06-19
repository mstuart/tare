use std::collections::HashMap;
use crate::plan::{DropReason, PlanEntry, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::SegmentKind;

/// Drop superseded tool outputs (spec §7 A1): for each ToolOutput class, every occurrence
/// except the latest (highest `position`) is dropped — an old build/test/lint/status run is
/// obsoleted by a newer run of the same class. Whole-unit Drop (exempt from I4); the planner's
/// I3 still protects any frozen segment.
pub struct SupersessionPass;

impl Pass for SupersessionPass {
    fn name(&self) -> &'static str { "supersession-decay" }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        // latest position per class
        let mut latest: HashMap<&str, usize> = HashMap::new();
        for s in ctx.segments {
            if let SegmentKind::ToolOutput { class } = &s.kind {
                let e = latest.entry(class.as_str()).or_insert(s.position);
                if s.position > *e { *e = s.position; }
            }
        }
        // drop earlier same-class outputs
        ctx.segments.iter().filter_map(|s| {
            if let SegmentKind::ToolOutput { class } = &s.kind {
                if s.position < latest[class.as_str()] {
                    return Some(PlanEntry { id: s.id, action: SegmentAction::Drop(DropReason::Superseded) });
                }
            }
            None
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use crate::plan::{SegmentAction, DropReason};
    use crate::planner::Planner;
    use crate::session::SessionState;

    fn tool_seg(id: u64, class: &str) -> Segment {
        Segment {
            id: SegmentId(id), kind: SegmentKind::ToolOutput { class: class.into() },
            role: Role::Tool, bytes: format!("output {id}").into_bytes(), token_count: 10,
            position: id as usize, mutation_class: MutationClass::Fast, origin: Origin::default(),
            protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    #[test]
    fn drops_all_but_latest_per_class() {
        // cargo-test at 0,2,4 ; git-status at 1 ; only the latest cargo-test (4) survives
        let segs = vec![
            tool_seg(0, "cargo-test"),
            tool_seg(1, "git-status"),
            tool_seg(2, "cargo-test"),
            tool_seg(4, "cargo-test"),
        ];
        let plan = Planner::new(vec![Box::new(SupersessionPass)]).plan(&segs, &SessionState::default());
        // entries are in segment order: ids 0,1,2,4
        assert_eq!(plan.entries[0].action, SegmentAction::Drop(DropReason::Superseded)); // cargo-test 0
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);                          // git-status (only one)
        assert_eq!(plan.entries[2].action, SegmentAction::Drop(DropReason::Superseded)); // cargo-test 2
        assert_eq!(plan.entries[3].action, SegmentAction::Keep);                          // cargo-test 4 (latest)
    }

    #[test]
    fn single_output_is_kept() {
        let segs = vec![tool_seg(0, "cargo-test")];
        let plan = Planner::new(vec![Box::new(SupersessionPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn non_tooloutput_segments_are_ignored() {
        let mut s = tool_seg(0, "x");
        s.kind = SegmentKind::ConversationTurn;
        let segs = vec![s, tool_seg(1, "x")]; // different kinds; the ConversationTurn is not a ToolOutput
        let plan = Planner::new(vec![Box::new(SupersessionPass)]).plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep); // conversation turn untouched
        assert_eq!(plan.entries[1].action, SegmentAction::Keep); // only one ToolOutput of class "x"
    }
}
