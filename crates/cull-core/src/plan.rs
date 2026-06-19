use crate::segment::{Segment, SegmentId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropReason { Superseded, IrrelevantBySlice, Duplicate, Evicted, StaleOutput }

/// Drop removes a WHOLE unit (allowed). Replace substitutes a LOSSLESS smaller representation
/// that must preserve every protected token of the original (spec I2/I4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentAction {
    Keep,
    Drop(DropReason),
    Replace { bytes: Vec<u8>, token_count: u32, reason: DropReason },
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

/// I4/I2: every protected token of the original must appear byte-exact in a Replace's bytes.
/// Drop is exempt (whole unit removed).
pub fn replace_preserves_protected(original: &Segment, action: &SegmentAction) -> bool {
    let SegmentAction::Replace { bytes, .. } = action else { return true; };
    original.protected_spans.iter().all(|p| {
        let needle = &original.bytes[p.span.start..p.span.end];
        contains_subslice(bytes, needle)
    })
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() { return true; }
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use crate::protected::{ProtectedKind, ProtectedSpan};

    fn seg(id: u64, text: &str, protected: Vec<ProtectedSpan>) -> Segment {
        Segment {
            id: SegmentId(id), kind: SegmentKind::ToolOutput { class: "grep".into() },
            role: Role::Tool, bytes: text.as_bytes().to_vec(), token_count: 10, position: id as usize,
            mutation_class: MutationClass::Fast, origin: Origin::default(),
            protected_spans: protected, refs: RefLedger::default(),
        }
    }

    #[test]
    fn net_tokens_accounts_keep_drop_replace() {
        let segs = vec![seg(0, "aaaa", vec![]), seg(1, "bbbb", vec![])];
        let plan = CompressionPlan { entries: vec![
            PlanEntry { id: SegmentId(0), action: SegmentAction::Keep },
            PlanEntry { id: SegmentId(1), action: SegmentAction::Drop(DropReason::Duplicate) },
        ]};
        assert_eq!(net_tokens(&plan, &segs), 10);
        assert_eq!(input_tokens(&segs), 20);
    }

    #[test]
    fn i4_validator_rejects_replace_that_drops_a_protected_token() {
        let text = "see src/auth.rs:42 for details";
        let protected = vec![ProtectedSpan { span: Span { start: 4, end: 15 }, kind: ProtectedKind::Path }];
        let s = seg(0, text, protected);
        let keeps_path = SegmentAction::Replace { bytes: b"src/auth.rs changed".to_vec(), token_count: 3, reason: DropReason::Duplicate };
        assert!(replace_preserves_protected(&s, &keeps_path));
        let drops_path = SegmentAction::Replace { bytes: b"a file changed".to_vec(), token_count: 3, reason: DropReason::Duplicate };
        assert!(!replace_preserves_protected(&s, &drops_path));
    }
}
