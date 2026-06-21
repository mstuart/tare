//! Lossless columnar compaction of repetitive JSON arrays (spec: structured tool-output case).
//!
//! For each segment whose text is a JSON array of similar objects, propose a `Replace` with the
//! columnar encoding (`json_crush::crush`) when it is smaller. The planner verifies value-lossless
//! reconstruction (`Reconstruct::JsonColumnar`) and the token reduction before accepting it. This is
//! the intra-blob structural compression incumbents (e.g. Headroom's SmartCrusher) specialize in —
//! here done value-losslessly (every field recoverable, not just flagged "needles").

use crate::json_crush::crush;
use crate::plan::{DropReason, PlanEntry, Reconstruct, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use cull_tokenize::{ApproxCounter, TokenCounter};

pub struct JsonCompactionPass {
    counter: ApproxCounter,
}

impl JsonCompactionPass {
    pub fn new() -> Self {
        Self {
            counter: ApproxCounter::o200k(),
        }
    }
}

impl Default for JsonCompactionPass {
    fn default() -> Self {
        Self::new()
    }
}

impl Pass for JsonCompactionPass {
    fn name(&self) -> &'static str {
        "json-compaction"
    }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        ctx.segments
            .iter()
            .filter_map(|s| {
                // Compaction targets tool OUTPUTS (API responses, command results); file reads are
                // code/text handled by the IVM/delta path and kept verbatim.
                if matches!(s.kind, crate::segment::SegmentKind::FileRead) {
                    return None;
                }
                let text = std::str::from_utf8(&s.bytes).ok()?;
                let crushed = crush(text)?;
                let token_count = self.counter.count(&crushed) as u32;
                if token_count >= s.token_count {
                    return None;
                }
                Some(PlanEntry {
                    id: s.id,
                    action: SegmentAction::Replace {
                        rendered: crushed.into_bytes(),
                        token_count,
                        reconstruct: Reconstruct::JsonColumnar,
                        reason: DropReason::Duplicate,
                    },
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::SegmentAction;
    use crate::planner::Planner;
    use crate::segment::*;
    use crate::session::SessionState;

    fn json_seg(id: u64, text: &str) -> Segment {
        Segment {
            id: SegmentId(id),
            kind: SegmentKind::ToolOutput {
                class: "json".into(),
            },
            role: Role::Tool,
            bytes: text.as_bytes().to_vec(),
            token_count: ApproxCounter::o200k().count(text) as u32,
            position: id as usize,
            mutation_class: MutationClass::Fast,
            origin: Origin::default(),
            protected_spans: vec![],
            refs: RefLedger::default(),
        }
    }

    #[test]
    fn compacts_repetitive_json_array_losslessly() {
        let text = serde_json::to_string_pretty(&serde_json::json!([
            {"id":0,"name":"item_0","value":0.0,"status":"active"},
            {"id":1,"name":"item_1","value":1.5,"status":"active"},
            {"id":2,"name":"item_2","value":3.0,"status":"active"},
            {"id":3,"name":"item_3","value":4.5,"status":"active"},
            {"id":4,"name":"item_4","value":6.0,"status":"active"}
        ]))
        .unwrap();
        let seg = json_seg(0, &text);
        let original_tokens = seg.token_count;
        let plan = Planner::new(vec![Box::new(JsonCompactionPass::new())])
            .plan(&[seg], &SessionState::default());
        // accepted as a lossless Replace that reduces tokens
        match &plan.entries[0].action {
            SegmentAction::Replace { token_count, .. } => assert!(*token_count < original_tokens),
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[test]
    fn leaves_non_json_untouched() {
        let seg = json_seg(0, "just some prose, not json at all here");
        let plan = Planner::new(vec![Box::new(JsonCompactionPass::new())])
            .plan(&[seg], &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }
}
