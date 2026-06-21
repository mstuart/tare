use crate::plan::{DropReason, PlanEntry, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::SegmentKind;

/// Query-conditioned relevance pruning (spec §7 B1, deterministic v1). Drops droppable
/// tool-result/file-read segments whose symbols are disjoint from the task query symbols,
/// keeping the most recent `recency_keep` positions regardless (guards against false drops).
/// No task signal → drops nothing. PRF expansion (B2) and embedding salience (B3) are future
/// upgrades that would enrich `ctx.task.symbols` / replace the disjoint check.
pub struct RelevancePass {
    pub recency_keep: usize,
}

impl Default for RelevancePass {
    fn default() -> Self {
        Self { recency_keep: 6 }
    }
}

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

impl Pass for RelevancePass {
    fn name(&self) -> &'static str {
        "query-relevance"
    }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        if ctx.task.is_empty() {
            return Vec::new();
        }
        let max_pos = ctx.segments.iter().map(|s| s.position).max().unwrap_or(0);

        // symbols per segment (path-aware: tree-sitter for known code extensions, regex otherwise)
        let seg_syms: Vec<std::collections::HashSet<String>> = ctx
            .segments
            .iter()
            .map(|s| {
                crate::code::extract_symbols_for(
                    &String::from_utf8_lossy(&s.bytes),
                    s.origin.path.as_deref(),
                )
            })
            .collect();

        // BFS: relevance propagates from task-overlapping segments through shared symbols
        let mut relevant = vec![false; ctx.segments.len()];
        let mut active: std::collections::HashSet<String> = ctx.task.symbols.clone();
        loop {
            let mut changed = false;
            for i in 0..ctx.segments.len() {
                if !relevant[i] && !seg_syms[i].is_disjoint(&active) {
                    relevant[i] = true;
                    for s in &seg_syms[i] {
                        active.insert(s.clone());
                    }
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        ctx.segments
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                if !is_droppable_kind(&s.kind) {
                    return None;
                }
                if max_pos.saturating_sub(s.position) < self.recency_keep {
                    return None;
                }
                if relevant[i] {
                    return None;
                }
                Some(PlanEntry {
                    id: s.id,
                    action: SegmentAction::Drop(DropReason::IrrelevantBySlice),
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
    fn drops_irrelevant_old_segment_keeps_relevant_and_recent() {
        let task = TaskSignal::from_text("authentication jwt middleware");
        let segs = vec![
            seg(0, 0, SegmentKind::FileRead, "jwt verify middleware token"), // relevant (jwt, middleware)
            seg(
                1,
                1,
                SegmentKind::ToolOutput {
                    class: "grep".into(),
                },
                "postgres connection pool retries",
            ), // irrelevant, old
            seg(
                2,
                20,
                SegmentKind::ToolOutput {
                    class: "grep".into(),
                },
                "unrelated kafka topic",
            ), // irrelevant BUT recent
        ];
        let plan = Planner::new(vec![Box::new(RelevancePass { recency_keep: 6 })]).plan_with_task(
            &segs,
            &SessionState::default(),
            &task,
        );
        assert_eq!(plan.entries[0].action, SegmentAction::Keep); // relevant
        assert_eq!(
            plan.entries[1].action,
            SegmentAction::Drop(DropReason::IrrelevantBySlice)
        ); // irrelevant + old
        assert_eq!(plan.entries[2].action, SegmentAction::Keep); // recent (within recency_keep of max pos 20)
    }

    #[test]
    fn no_task_signal_drops_nothing() {
        let segs = vec![seg(0, 0, SegmentKind::FileRead, "anything at all")];
        let plan = Planner::new(vec![Box::new(RelevancePass::default())])
            .plan(&segs, &SessionState::default()); // plan() => empty task
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn non_droppable_kinds_are_never_dropped_by_relevance() {
        let task = TaskSignal::from_text("authentication");
        // a conversation turn with no overlap, old position — must still be kept
        let segs = vec![seg(
            0,
            0,
            SegmentKind::ConversationTurn,
            "totally unrelated chatter",
        )];
        let plan = Planner::new(vec![Box::new(RelevancePass { recency_keep: 0 })]).plan_with_task(
            &segs,
            &SessionState::default(),
            &task,
        );
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
    }

    #[test]
    fn relevance_uses_treesitter_symbols_for_code_paths() {
        // task references a function name; a code file defining it (path .rs) must be kept,
        // an unrelated code file dropped — symbols come from the AST, not loose words.
        let task = TaskSignal::from_text("fix validate_session");
        let mut a = seg(
            0,
            0,
            SegmentKind::FileRead,
            "fn validate_session(t: Token) { check(t) }",
        );
        a.origin.path = Some("session.rs".into());
        let mut b = seg(1, 1, SegmentKind::FileRead, "fn render_button() { draw() }");
        b.origin.path = Some("ui.rs".into());
        let plan = Planner::new(vec![Box::new(RelevancePass { recency_keep: 0 })]).plan_with_task(
            &[a, b],
            &SessionState::default(),
            &task,
        );
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(
            plan.entries[1].action,
            SegmentAction::Drop(DropReason::IrrelevantBySlice)
        );
    }

    #[test]
    fn relevance_propagates_transitively_through_shared_symbols() {
        // task mentions "auth"; seg A (auth+jwt) is direct; seg B (jwt+middleware) connects via "jwt";
        // seg C (kafka) is unconnected and old -> dropped.
        let task = TaskSignal::from_text("auth subsystem");
        let segs = vec![
            seg(0, 0, SegmentKind::FileRead, "auth login session jwt"), // direct (auth)
            seg(1, 1, SegmentKind::FileRead, "jwt verify middleware token"), // transitive via jwt
            seg(
                2,
                2,
                SegmentKind::FileRead,
                "kafka broker partitions offset",
            ), // unconnected
        ];
        let plan = Planner::new(vec![Box::new(RelevancePass { recency_keep: 0 })]).plan_with_task(
            &segs,
            &SessionState::default(),
            &task,
        );
        assert_eq!(plan.entries[0].action, SegmentAction::Keep); // direct
        assert_eq!(plan.entries[1].action, SegmentAction::Keep); // transitive — kept
        assert_eq!(
            plan.entries[2].action,
            SegmentAction::Drop(DropReason::IrrelevantBySlice)
        ); // unconnected
    }
}
