use crate::plan::{DropReason, PlanEntry, Reconstruct, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::{Segment, SegmentKind};
use std::collections::HashMap;
use tare_tokenize::{ApproxCounter, TokenCounter};

/// File-read IVM/delta (spec §7 A2). For each file path, the lowest-position FileRead is the base
/// (kept). Later *changed* reads of that path become a unified-diff Replace against the base; the
/// planner verifies each diff reconstructs the original exactly (else reverts). Exact-duplicate
/// re-reads are skipped (the dedup pass drops them as whole units).
pub struct IvmDeltaPass {
    counter: ApproxCounter,
}

impl IvmDeltaPass {
    pub fn new() -> Self {
        Self {
            counter: ApproxCounter::o200k(),
        }
    }
}

impl Default for IvmDeltaPass {
    fn default() -> Self {
        Self::new()
    }
}

impl Pass for IvmDeltaPass {
    fn name(&self) -> &'static str {
        "file-read-ivm-delta"
    }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        // base = lowest-position FileRead per path
        let mut base_of: HashMap<&str, &Segment> = HashMap::new();
        for s in ctx.segments {
            if let (SegmentKind::FileRead, Some(path)) = (&s.kind, &s.origin.path) {
                base_of
                    .entry(path.as_str())
                    .and_modify(|b| {
                        if s.position < b.position {
                            *b = s;
                        }
                    })
                    .or_insert(s);
            }
        }

        let mut out = Vec::new();
        for s in ctx.segments {
            let (SegmentKind::FileRead, Some(path)) = (&s.kind, &s.origin.path) else {
                continue;
            };
            let Some(base) = base_of.get(path.as_str()) else {
                continue;
            };
            if base.id == s.id {
                continue;
            } // this IS the base
            if base.bytes == s.bytes {
                continue;
            } // exact dup -> dedup handles it
            let (Ok(base_str), Ok(this_str)) = (
                std::str::from_utf8(&base.bytes),
                std::str::from_utf8(&s.bytes),
            ) else {
                continue;
            };
            let patch = diffy::create_patch(base_str, this_str).to_string();
            let token_count = self.counter.count(&patch) as u32;
            out.push(PlanEntry {
                id: s.id,
                action: SegmentAction::Replace {
                    rendered: patch.into_bytes(),
                    token_count,
                    reconstruct: Reconstruct::Delta { base: base.id },
                    reason: DropReason::Duplicate,
                },
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::SegmentAction;
    use crate::planner::Planner;
    use crate::segment::*;
    use crate::session::SessionState;

    fn file_seg(id: u64, pos: usize, path: &str, text: &str) -> Segment {
        Segment {
            id: SegmentId(id),
            kind: SegmentKind::FileRead,
            role: Role::Tool,
            bytes: text.as_bytes().to_vec(),
            token_count: 100,
            position: pos,
            mutation_class: MutationClass::Fast,
            origin: Origin {
                turn: pos,
                path: Some(path.into()),
                ..Origin::default()
            },
            protected_spans: vec![],
            refs: RefLedger::default(),
        }
    }

    #[test]
    fn reread_with_small_change_becomes_lossless_delta() {
        let base = file_seg(
            0,
            0,
            "src/a.rs",
            "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\nfn five() {}\n",
        );
        let reread = file_seg(
            1,
            5,
            "src/a.rs",
            "fn one() {}\nfn two() {}\nfn THREE() {}\nfn four() {}\nfn five() {}\n",
        );
        let plan = Planner::new(vec![Box::new(IvmDeltaPass::new())])
            .plan(&[base.clone(), reread.clone()], &SessionState::default());
        // base kept; reread replaced by a lossless delta (planner already verified it reconstructs)
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert!(
            matches!(plan.entries[1].action, SegmentAction::Replace { .. }),
            "changed re-read of same path becomes a delta Replace"
        );
    }

    #[test]
    fn different_paths_are_not_deltad() {
        let a = file_seg(0, 0, "src/a.rs", "alpha contents here and there\n");
        let b = file_seg(1, 1, "src/b.rs", "beta contents totally other\n");
        let plan = Planner::new(vec![Box::new(IvmDeltaPass::new())])
            .plan(&[a, b], &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);
    }

    #[test]
    fn exact_reread_is_left_for_dedup() {
        let base = file_seg(0, 0, "src/a.rs", "identical contents\n");
        let same = file_seg(1, 5, "src/a.rs", "identical contents\n");
        let plan = Planner::new(vec![Box::new(IvmDeltaPass::new())])
            .plan(&[base, same], &SessionState::default());
        // IVM skips exact duplicates -> Keep (dedup pass handles these)
        assert_eq!(plan.entries[1].action, SegmentAction::Keep);
    }
}
