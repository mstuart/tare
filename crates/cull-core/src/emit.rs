use crate::plan::{input_tokens, CompressionPlan, DropReason, SegmentAction};
use crate::planner::stability_order;
use crate::segment::{Segment, SegmentId};
use std::collections::HashMap;

/// One segment in the compressed output, in cache-stable order.
#[derive(Debug, Clone)]
pub struct EmittedSegment {
    pub id: SegmentId,
    pub bytes: Vec<u8>,
    pub token_count: u32,
}

/// Self-reporting fidelity surface (spec §11): what the engine did, on the real workload.
#[derive(Debug, Clone)]
pub struct FidelityReport {
    pub input_tokens: u32,
    pub net_tokens: u32,
    pub kept: usize,
    pub dropped: usize,
    pub replaced: usize,
    pub drops: Vec<(SegmentId, DropReason)>,
}

impl FidelityReport {
    /// net / input (1.0 means no compression; lower is more compressed).
    pub fn ratio(&self) -> f64 {
        if self.input_tokens == 0 {
            1.0
        } else {
            self.net_tokens as f64 / self.input_tokens as f64
        }
    }
}

/// Apply a plan to segments and assemble the compressed output in cache-stable order.
/// Keep → original bytes; Replace → rendered bytes; Drop → omitted. A segment with no plan
/// entry defaults to Keep (safe).
pub fn emit(segments: &[Segment], plan: &CompressionPlan) -> (Vec<EmittedSegment>, FidelityReport) {
    let action_of: HashMap<SegmentId, &SegmentAction> =
        plan.entries.iter().map(|e| (e.id, &e.action)).collect();
    let seg_of: HashMap<SegmentId, &Segment> = segments.iter().map(|s| (s.id, s)).collect();

    let mut emitted = Vec::new();
    let (mut kept, mut dropped, mut replaced, mut net) = (0usize, 0usize, 0usize, 0u32);
    let mut drops = Vec::new();

    for id in stability_order(segments) {
        let s = seg_of[&id];
        match action_of.get(&id) {
            None | Some(SegmentAction::Keep) => {
                emitted.push(EmittedSegment {
                    id,
                    bytes: s.bytes.clone(),
                    token_count: s.token_count,
                });
                kept += 1;
                net += s.token_count;
            }
            Some(SegmentAction::Drop(reason)) => {
                dropped += 1;
                drops.push((id, reason.clone()));
            }
            Some(SegmentAction::Replace {
                rendered,
                token_count,
                ..
            }) => {
                emitted.push(EmittedSegment {
                    id,
                    bytes: rendered.clone(),
                    token_count: *token_count,
                });
                replaced += 1;
                net += *token_count;
            }
        }
    }

    let report = FidelityReport {
        input_tokens: input_tokens(segments),
        net_tokens: net,
        kept,
        dropped,
        replaced,
        drops,
    };
    (emitted, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{CompressionPlan, DropReason, PlanEntry, SegmentAction};
    use crate::segment::*;

    fn seg(id: u64, pos: usize, class: MutationClass, tok: u32, text: &str) -> Segment {
        Segment {
            id: SegmentId(id),
            kind: SegmentKind::FileRead,
            role: Role::Tool,
            bytes: text.as_bytes().to_vec(),
            token_count: tok,
            position: pos,
            mutation_class: class,
            origin: Origin::default(),
            protected_spans: vec![],
            refs: RefLedger::default(),
        }
    }

    #[test]
    fn applies_keep_drop_replace_and_orders_by_stability() {
        // positions: fast(0), frozen(1), fast(2). Stability order => frozen(1), fast(0), fast(2).
        let segs = vec![
            seg(0, 0, MutationClass::Fast, 10, "AAAA"),
            seg(1, 1, MutationClass::Frozen, 5, "SYS"),
            seg(2, 2, MutationClass::Fast, 10, "CCCC"),
        ];
        let plan = CompressionPlan {
            entries: vec![
                PlanEntry {
                    id: SegmentId(0),
                    action: SegmentAction::Drop(DropReason::Superseded),
                },
                PlanEntry {
                    id: SegmentId(1),
                    action: SegmentAction::Keep,
                },
                PlanEntry {
                    id: SegmentId(2),
                    action: SegmentAction::Replace {
                        rendered: b"cc".to_vec(),
                        token_count: 3,
                        reconstruct: crate::plan::Reconstruct::Delta { base: SegmentId(1) },
                        reason: DropReason::Duplicate,
                    },
                },
            ],
        };
        let (emitted, report) = emit(&segs, &plan);

        // order: frozen seg 1 first, then surviving fast seg 2 (seg 0 dropped)
        assert_eq!(emitted.len(), 2);
        assert_eq!(emitted[0].id, SegmentId(1));
        assert_eq!(emitted[0].bytes, b"SYS"); // kept original
        assert_eq!(emitted[1].id, SegmentId(2));
        assert_eq!(emitted[1].bytes, b"cc"); // replaced rendered
        assert_eq!(emitted[1].token_count, 3);

        assert_eq!(report.input_tokens, 25); // 10+5+10
        assert_eq!(report.net_tokens, 8); // kept 5 + replaced 3
        assert_eq!(report.kept, 1);
        assert_eq!(report.dropped, 1);
        assert_eq!(report.replaced, 1);
        assert_eq!(report.drops, vec![(SegmentId(0), DropReason::Superseded)]);
        assert!((report.ratio() - (8.0 / 25.0)).abs() < 1e-9);
    }

    #[test]
    fn missing_entry_defaults_to_keep() {
        let segs = vec![seg(0, 0, MutationClass::Fast, 10, "X")];
        let plan = CompressionPlan { entries: vec![] }; // no decision for seg 0
        let (emitted, report) = emit(&segs, &plan);
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].bytes, b"X");
        assert_eq!(report.net_tokens, 10);
        assert_eq!(report.kept, 1);
    }

    #[test]
    fn emits_delta_rendered_and_it_round_trips_to_original() {
        use crate::plan::{apply_unified_diff, Reconstruct};
        let base_text = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let new_text = "fn a() {}\nfn b2() {}\nfn c() {}\n";
        let base = seg(0, 0, MutationClass::Fast, 20, base_text);
        let target = seg(1, 1, MutationClass::Fast, 20, new_text);
        let patch = diffy::create_patch(base_text, new_text).to_string();

        let plan = CompressionPlan {
            entries: vec![
                PlanEntry {
                    id: SegmentId(0),
                    action: SegmentAction::Keep,
                },
                PlanEntry {
                    id: SegmentId(1),
                    action: SegmentAction::Replace {
                        rendered: patch.clone().into_bytes(),
                        token_count: 5,
                        reconstruct: Reconstruct::Delta { base: SegmentId(0) },
                        reason: DropReason::Duplicate,
                    },
                },
            ],
        };
        let (emitted, _report) = emit(&[base.clone(), target.clone()], &plan);
        // the emitted Replace bytes are the diff; applying it to base recovers the exact original
        let emitted_delta = &emitted.iter().find(|e| e.id == SegmentId(1)).unwrap().bytes;
        let recovered = apply_unified_diff(base.bytes.as_slice(), emitted_delta).unwrap();
        assert_eq!(recovered, target.bytes);
    }
}
