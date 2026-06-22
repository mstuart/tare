//! Byte-lossless columnar compaction of repetitive plain-text logs.
//!
//! For each segment whose text is a repetitive log (and not JSON — JSON is handled by
//! `JsonCompactionPass`), propose a `Replace` with the log-columnar encoding (`log_crush::crush`)
//! when it is smaller. The planner verifies byte-exact reconstruction (`Reconstruct::LogColumnar`)
//! and the token reduction before accepting it.

use crate::log_crush::crush;
use crate::plan::{DropReason, PlanEntry, Reconstruct, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use tare_tokenize::{ApproxCounter, TokenCounter};

pub struct LogCompactionPass {
    counter: ApproxCounter,
}

impl LogCompactionPass {
    pub fn new() -> Self {
        Self {
            counter: ApproxCounter::o200k(),
        }
    }
}

impl Default for LogCompactionPass {
    fn default() -> Self {
        Self::new()
    }
}

impl Pass for LogCompactionPass {
    fn name(&self) -> &'static str {
        "log-compaction"
    }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        ctx.segments
            .iter()
            .filter_map(|s| {
                // Compaction targets tool OUTPUTS; file reads are code/text (the IVM/delta path).
                // Without this guard, uniform code like `fn a() { ... }` would be log-columnarized.
                if matches!(s.kind, crate::segment::SegmentKind::FileRead) {
                    return None;
                }
                let text = std::str::from_utf8(&s.bytes).ok()?;
                // Skip JSON — that's JsonCompactionPass's job (and avoids double-encoding).
                if serde_json::from_str::<serde_json::Value>(text).is_ok() {
                    return None;
                }
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
                        reconstruct: Reconstruct::LogColumnar,
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

    fn log_seg(id: u64, text: &str) -> Segment {
        Segment {
            id: SegmentId(id),
            kind: SegmentKind::ToolOutput {
                class: "log".into(),
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
    fn compacts_repetitive_log_losslessly() {
        let text = (0..30)
            .map(|i| {
                format!(
                    "2024-06-20T10:00:{:02}Z INFO worker-{} processed batch {} ok",
                    i % 60,
                    i % 8,
                    i
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let seg = log_seg(0, &text);
        let original = seg.token_count;
        let plan = Planner::new(vec![Box::new(LogCompactionPass::new())])
            .plan(&[seg], &SessionState::default());
        match &plan.entries[0].action {
            SegmentAction::Replace { token_count, .. } => assert!(*token_count < original),
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[test]
    fn leaves_json_to_json_pass() {
        let seg = log_seg(0, r#"[{"a":1},{"a":2},{"a":3}]"#);
        let plan = Planner::new(vec![Box::new(LogCompactionPass::new())])
            .plan(&[seg], &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep); // JSON skipped here
    }
}
