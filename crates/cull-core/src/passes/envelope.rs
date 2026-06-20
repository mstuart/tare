use crate::plan::{DropReason, PlanEntry, Reconstruct, SegmentAction};
use crate::planner::{Pass, PlanCtx};
use crate::segment::SegmentKind;
use cull_tokenize::{ApproxCounter, TokenCounter};

/// Lossless dedup of repetitive tool-output envelopes (spec §7 A3). For each ToolOutput, delta
/// against the most-similar EARLIER ToolOutput when that diff is materially smaller than the
/// original. Reuses Reconstruct::Delta (planner verifies losslessness + base protection).
pub struct EnvelopeDedupPass { counter: ApproxCounter, min_reduction: f64 }

impl EnvelopeDedupPass {
    pub fn new() -> Self { Self { counter: ApproxCounter::o200k(), min_reduction: 0.6 } }
}
impl Default for EnvelopeDedupPass { fn default() -> Self { Self::new() } }

impl Pass for EnvelopeDedupPass {
    fn name(&self) -> &'static str { "envelope-dedup" }

    fn propose(&self, ctx: &PlanCtx) -> Vec<PlanEntry> {
        let outs: Vec<_> = ctx.segments.iter()
            .filter(|s| matches!(s.kind, SegmentKind::ToolOutput { .. }))
            .collect();
        let mut proposals = Vec::new();
        for (idx, s) in outs.iter().enumerate() {
            let Ok(s_str) = std::str::from_utf8(&s.bytes) else { continue; };
            let mut best: Option<(crate::segment::SegmentId, String, u32)> = None;
            for base in &outs[..idx] {
                let Ok(b_str) = std::str::from_utf8(&base.bytes) else { continue; };
                let patch = diffy::create_patch(b_str, s_str).to_string();
                let pt = self.counter.count(&patch) as u32;
                if best.as_ref().map_or(true, |(_, _, bp)| pt < *bp) {
                    best = Some((base.id, patch, pt));
                }
            }
            if let Some((base, patch, pt)) = best {
                if (pt as f64) < self.min_reduction * (s.token_count as f64) {
                    proposals.push(PlanEntry { id: s.id, action: SegmentAction::Replace {
                        rendered: patch.into_bytes(), token_count: pt,
                        reconstruct: Reconstruct::Delta { base }, reason: DropReason::Duplicate } });
                }
            }
        }
        proposals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use crate::plan::SegmentAction;
    use crate::planner::Planner;
    use crate::session::SessionState;

    fn out(id: u64, pos: usize, tok: u32, text: &str) -> Segment {
        Segment { id: SegmentId(id), kind: SegmentKind::ToolOutput { class: "grep".into() },
            role: Role::Tool, bytes: text.as_bytes().to_vec(), token_count: tok, position: pos,
            mutation_class: MutationClass::Fast, origin: Origin::default(),
            protected_spans: vec![], refs: RefLedger::default() }
    }

    #[test]
    fn repetitive_envelope_is_deltad_against_earlier_output() {
        // same JSON envelope, one differing field — the second becomes a small delta.
        // token_count is set to 200 so that the patch (~79 tokens) is materially smaller.
        let a = out(0, 0, 200, "{\"tool\":\"grep\",\"version\":1,\"results\":[\"a\",\"b\",\"c\"],\"meta\":{\"ms\":12}}");
        let b = out(1, 1, 200, "{\"tool\":\"grep\",\"version\":1,\"results\":[\"a\",\"b\",\"X\"],\"meta\":{\"ms\":12}}");
        let plan = Planner::new(vec![Box::new(EnvelopeDedupPass::new())]).plan(&[a.clone(), b.clone()], &SessionState::default());
        assert_eq!(plan.entries[0].action, SegmentAction::Keep);
        assert!(matches!(plan.entries[1].action, SegmentAction::Replace { .. }), "near-duplicate envelope deltad");
    }

    #[test]
    fn dissimilar_outputs_are_not_deltad() {
        let a = out(0, 0, 60, "completely different alpha content here and there everywhere");
        let b = out(1, 1, 60, "nothing in common beta gamma delta epsilon zeta eta theta");
        let plan = Planner::new(vec![Box::new(EnvelopeDedupPass::new())]).plan(&[a, b], &SessionState::default());
        assert_eq!(plan.entries[1].action, SegmentAction::Keep, "no shared envelope -> no delta");
    }
}
