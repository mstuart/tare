use crate::protected::detect_protected_spans;
use crate::segment::*;
use cull_tokenize::TokenCounter;

/// Raw input unit (a provider message/content block, pre-classified). The proxy
/// produces these from Anthropic/OpenAI requests in a later plan.
#[derive(Debug, Clone)]
pub struct RawBlock {
    pub role: Role,
    pub kind: SegmentKind,
    pub text: String,
}

/// Turn raw blocks into fully-populated segments: sequential ids/positions,
/// token counts, mutation classes, and protected-span annotations.
pub fn segment(blocks: &[RawBlock], counter: &dyn TokenCounter) -> Vec<Segment> {
    blocks
        .iter()
        .enumerate()
        .map(|(i, b)| Segment {
            id: SegmentId(i as u64),
            kind: b.kind.clone(),
            role: b.role,
            token_count: counter.count(&b.text) as u32,
            position: i,
            mutation_class: MutationClass::for_kind(&b.kind),
            protected_spans: detect_protected_spans(&b.text),
            origin: Origin { turn: i, ..Origin::default() },
            bytes: b.text.clone().into_bytes(),
            refs: RefLedger::default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::*;
    use cull_tokenize::ApproxCounter;

    fn raw(role: Role, kind: SegmentKind, text: &str) -> RawBlock {
        RawBlock { role, kind, text: text.to_string() }
    }

    #[test]
    fn assigns_ids_positions_counts_and_classes() {
        let counter = ApproxCounter::o200k();
        let blocks = vec![
            raw(Role::System, SegmentKind::SystemPrompt, "You are an agent."),
            raw(Role::Tool, SegmentKind::FileRead, "fn main() {} // src/main.rs:1"),
        ];
        let segs = segment(&blocks, &counter);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].id, SegmentId(0));
        assert_eq!(segs[1].id, SegmentId(1));
        assert_eq!(segs[0].position, 0);
        assert_eq!(segs[1].position, 1);
        assert_eq!(segs[0].mutation_class, MutationClass::Frozen);
        assert_eq!(segs[1].mutation_class, MutationClass::Fast);
        assert!(segs[0].token_count > 0);
        assert!(!segs[1].protected_spans.is_empty());
        assert_eq!(segs[1].bytes, b"fn main() {} // src/main.rs:1");
    }

    #[test]
    fn empty_input_yields_no_segments() {
        let counter = ApproxCounter::o200k();
        assert!(segment(&[], &counter).is_empty());
    }
}
