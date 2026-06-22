use crate::embed::{cosine, Embedder};
use crate::plan::{DropReason, PlanEntry, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::SegmentKind;

fn is_droppable_kind(kind: &SegmentKind) -> bool {
    matches!(
        kind,
        SegmentKind::FileRead
            | SegmentKind::DirListing
            | SegmentKind::ToolOutput { .. }
            | SegmentKind::StackTrace
            | SegmentKind::TestOutput
            | SegmentKind::Diff
    )
}

/// Embedding-salience pruning (spec §7 B3). Complements symbol-based relevance (B1): scores each
/// droppable, non-recent segment by cosine similarity between its embedding and the task embedding
/// (joined task symbols), dropping those below `min_similarity`. The default embedder is the
/// dependency-free `HashEmbedder`; a neural embedder plugs in behind the `Embedder` trait. Opt-in:
/// not part of the default pipeline (the symbol-based relevance pass stays the conservative
/// default). No task signal ⇒ no drops.
pub struct EmbeddingSaliencePass<E: Embedder> {
    pub embedder: E,
    pub min_similarity: f32,
    pub recency_keep: usize,
}

impl<E: Embedder> EmbeddingSaliencePass<E> {
    pub fn new(embedder: E, min_similarity: f32, recency_keep: usize) -> Self {
        Self {
            embedder,
            min_similarity,
            recency_keep,
        }
    }
}

impl<E: Embedder> Pass for EmbeddingSaliencePass<E> {
    fn name(&self) -> &'static str {
        "embedding-salience"
    }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        if ctx.task.is_empty() {
            return Vec::new();
        }
        let mut query: Vec<&str> = ctx.task.symbols.iter().map(|s| s.as_str()).collect();
        query.sort_unstable(); // deterministic query string
        let qvec = self.embedder.embed(&query.join(" "));
        let max_pos = ctx.segments.iter().map(|s| s.position).max().unwrap_or(0);

        ctx.segments
            .iter()
            .filter_map(|s| {
                if !is_droppable_kind(&s.kind) {
                    return None;
                }
                if max_pos.saturating_sub(s.position) < self.recency_keep {
                    return None;
                }
                let svec = self.embedder.embed(&String::from_utf8_lossy(&s.bytes));
                if cosine(&qvec, &svec) < self.min_similarity {
                    Some(PlanEntry {
                        id: s.id,
                        action: SegmentAction::Drop(DropReason::IrrelevantBySlice),
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::HashEmbedder;
    use crate::plan::{DropReason, SegmentAction};
    use crate::planner::Planner;
    use crate::segment::*;
    use crate::session::SessionState;
    use crate::task::TaskSignal;

    fn seg(id: u64, pos: usize, kind: SegmentKind, text: &str) -> Segment {
        Segment {
            id: SegmentId(id),
            kind,
            role: Role::Tool,
            bytes: text.as_bytes().to_vec(),
            token_count: 10,
            position: pos,
            mutation_class: MutationClass::Fast,
            origin: Origin::default(),
            protected_spans: vec![],
            refs: RefLedger::default(),
        }
    }

    #[test]
    fn drops_low_salience_old_keeps_salient_and_recent() {
        let task = TaskSignal::from_text("jwt authentication token");
        let segs = vec![
            seg(
                0,
                0,
                SegmentKind::FileRead,
                "jwt token verify authentication middleware",
            ), // salient, old -> keep
            seg(
                1,
                1,
                SegmentKind::ToolOutput {
                    class: "grep".into(),
                },
                "kafka broker partition offset consumer",
            ), // low, old -> drop
            seg(
                2,
                50,
                SegmentKind::ToolOutput {
                    class: "grep".into(),
                },
                "redis sentinel cluster failover",
            ), // low BUT recent -> keep
        ];
        let plan = Planner::new(vec![Box::new(EmbeddingSaliencePass::new(
            HashEmbedder::default(),
            0.1,
            6,
        ))])
        .plan_with_task(&segs, &SessionState::default(), &task);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(
            plan.entries[1].action,
            SegmentAction::Drop(DropReason::IrrelevantBySlice)
        );
        assert_eq!(plan.entries[2].action, SegmentAction::Keep);
    }

    #[test]
    fn no_task_signal_drops_nothing() {
        let segs = vec![seg(0, 0, SegmentKind::FileRead, "anything at all here")];
        let plan = Planner::new(vec![Box::new(EmbeddingSaliencePass::new(
            HashEmbedder::default(),
            0.1,
            0,
        ))])
        .plan(&segs, &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn non_droppable_kinds_never_dropped() {
        let task = TaskSignal::from_text("jwt authentication");
        let segs = vec![seg(
            0,
            0,
            SegmentKind::ConversationTurn,
            "totally unrelated kafka chatter",
        )];
        let plan = Planner::new(vec![Box::new(EmbeddingSaliencePass::new(
            HashEmbedder::default(),
            0.5,
            0,
        ))])
        .plan_with_task(&segs, &SessionState::default(), &task);
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }
}
