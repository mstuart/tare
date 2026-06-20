use std::collections::HashMap;
use crate::segment::{Segment, SegmentId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropReason { Superseded, IrrelevantBySlice, Duplicate, Evicted, StaleOutput }

/// How a Replace's `rendered` bytes recover the original. `Delta` is a unified diff against a base
/// segment; `JsonColumnar` is a self-contained columnar encoding of a repetitive JSON array
/// (value-lossless, reversible by `json_crush::expand`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconstruct { Delta { base: SegmentId }, JsonColumnar, LogColumnar }

/// Drop removes a WHOLE unit (allowed). Replace substitutes a LOSSLESS smaller representation:
/// `rendered` (a unified diff) is what gets sent; `reconstruct` says how to recover the exact
/// original. Losslessness is verified at plan time (spec I2/I4/I5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentAction {
    Keep,
    Drop(DropReason),
    Replace { rendered: Vec<u8>, token_count: u32, reconstruct: Reconstruct, reason: DropReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry { pub id: SegmentId, pub action: SegmentAction }

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompressionPlan { pub entries: Vec<PlanEntry> }

pub fn input_tokens(segments: &[Segment]) -> u32 {
    segments.iter().map(|s| s.token_count).sum()
}

/// Tokens the plan emits (Keep=original, Drop=0, Replace=replacement). Assumes entries[i] <-> segments[i].
pub fn net_tokens(plan: &CompressionPlan, segments: &[Segment]) -> u32 {
    plan.entries.iter().zip(segments.iter()).map(|(e, s)| match &e.action {
        SegmentAction::Keep => s.token_count,
        SegmentAction::Drop(_) => 0,
        SegmentAction::Replace { token_count, .. } => *token_count,
    }).sum()
}

/// Apply a unified diff (`patch`) to `base`, returning the reconstructed bytes, or None if the
/// inputs are not valid UTF-8 or the patch does not apply.
pub fn apply_unified_diff(base: &[u8], patch: &[u8]) -> Option<Vec<u8>> {
    let base = std::str::from_utf8(base).ok()?;
    let patch = std::str::from_utf8(patch).ok()?;
    let p = diffy::Patch::from_str(patch).ok()?;
    diffy::apply(base, &p).ok().map(String::into_bytes)
}

/// Verify a Replace losslessly reconstructs the original (spec I2/I4/I5). Drop/Keep are exempt.
pub fn replace_is_lossless(
    original: &Segment,
    action: &SegmentAction,
    by_id: &HashMap<SegmentId, &Segment>,
) -> bool {
    match action {
        SegmentAction::Replace { rendered, reconstruct, .. } => match reconstruct {
            Reconstruct::Delta { base } => match by_id.get(base) {
                Some(base_seg) => apply_unified_diff(&base_seg.bytes, rendered)
                    .map_or(false, |r| r == original.bytes),
                None => false,
            },
            Reconstruct::JsonColumnar => match (
                std::str::from_utf8(&original.bytes),
                std::str::from_utf8(rendered),
            ) {
                (Ok(orig), Ok(rend)) => crate::json_crush::round_trips(orig, rend),
                _ => false,
            },
            Reconstruct::LogColumnar => match (
                std::str::from_utf8(&original.bytes),
                std::str::from_utf8(rendered),
            ) {
                (Ok(orig), Ok(rend)) => crate::log_crush::round_trips(orig, rend),
                _ => false,
            },
        },
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;

    fn mk_seg(id: u64, text: &str) -> Segment {
        use crate::segment::*;
        Segment {
            id: SegmentId(id), kind: SegmentKind::FileRead, role: Role::Tool,
            bytes: text.as_bytes().to_vec(), token_count: 10, position: id as usize,
            mutation_class: MutationClass::Fast, origin: Origin::default(),
            protected_spans: vec![], refs: RefLedger::default(),
        }
    }

    fn seg(id: u64, text: &str) -> Segment {
        mk_seg(id, text)
    }

    #[test]
    fn net_tokens_accounts_keep_drop_replace() {
        let segs = vec![seg(0, "aaaa"), seg(1, "bbbb")];
        let plan = CompressionPlan { entries: vec![
            PlanEntry { id: SegmentId(0), action: SegmentAction::Keep },
            PlanEntry { id: SegmentId(1), action: SegmentAction::Drop(DropReason::Duplicate) },
        ]};
        assert_eq!(net_tokens(&plan, &segs), 10);
        assert_eq!(input_tokens(&segs), 20);
    }

    #[test]
    fn apply_unified_diff_round_trips() {
        let base = b"line one\nline two\nline three\n";
        let modified = b"line one\nline TWO changed\nline three\n";
        let patch = diffy::create_patch(
            std::str::from_utf8(base).unwrap(),
            std::str::from_utf8(modified).unwrap(),
        ).to_string();
        let recovered = apply_unified_diff(base, patch.as_bytes()).unwrap();
        assert_eq!(recovered, modified);
    }

    #[test]
    fn replace_is_lossless_accepts_valid_delta_rejects_bad_one() {
        use std::collections::HashMap;
        let base = mk_seg(0, "alpha\nbeta\ngamma\n");
        let target = mk_seg(1, "alpha\nBETA-X\ngamma\n");
        let by_id: HashMap<SegmentId, &Segment> = [(base.id, &base), (target.id, &target)].into_iter().collect();

        let good_patch = diffy::create_patch("alpha\nbeta\ngamma\n", "alpha\nBETA-X\ngamma\n").to_string();
        let good = SegmentAction::Replace {
            rendered: good_patch.into_bytes(), token_count: 2,
            reconstruct: Reconstruct::Delta { base: SegmentId(0) }, reason: DropReason::Duplicate,
        };
        assert!(replace_is_lossless(&target, &good, &by_id));

        // wrong diff (against base but produces different text) -> not lossless
        let bad_patch = diffy::create_patch("alpha\nbeta\ngamma\n", "totally different\n").to_string();
        let bad = SegmentAction::Replace {
            rendered: bad_patch.into_bytes(), token_count: 1,
            reconstruct: Reconstruct::Delta { base: SegmentId(0) }, reason: DropReason::Duplicate,
        };
        assert!(!replace_is_lossless(&target, &bad, &by_id));

        // missing base -> not lossless
        let orphan = SegmentAction::Replace {
            rendered: b"@@".to_vec(), token_count: 1,
            reconstruct: Reconstruct::Delta { base: SegmentId(99) }, reason: DropReason::Duplicate,
        };
        assert!(!replace_is_lossless(&target, &orphan, &by_id));
    }
}
