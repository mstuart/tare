use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SegmentId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role { System, User, Assistant, Tool }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentKind {
    SystemPrompt,
    ToolSchema,
    FileRead,
    DirListing,
    Diff,
    ToolOutput { class: String },
    StackTrace,
    TestOutput,
    ReasoningTrace,
    ConversationTurn,
    CompactSummary,
}

/// Cache-stability class. Drives stability-ordered segmentation (spec section 8 Rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MutationClass { Frozen, Slow, Fast }

impl MutationClass {
    pub fn for_kind(kind: &SegmentKind) -> MutationClass {
        match kind {
            SegmentKind::SystemPrompt => MutationClass::Frozen,
            SegmentKind::ToolSchema => MutationClass::Slow,
            SegmentKind::CompactSummary => MutationClass::Slow,
            _ => MutationClass::Fast,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskPhase { Discovery, Planning, Execution, Verification }

impl Default for TaskPhase { fn default() -> Self { TaskPhase::Discovery } }

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefLedger {
    pub recency: usize,
    pub frequency: u32,
    pub phase: TaskPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span { pub start: usize, pub end: usize }

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub turn: usize,
    pub tool: Option<String>,
    pub path: Option<String>,
    pub byte_range: Option<Span>,
    pub mtime: Option<u64>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub id: SegmentId,
    pub kind: SegmentKind,
    pub role: Role,
    pub bytes: Vec<u8>,
    pub token_count: u32,
    pub position: usize,
    pub mutation_class: MutationClass,
    pub origin: Origin,
    pub protected_spans: Vec<crate::protected::ProtectedSpan>,
    pub refs: RefLedger,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_class_follows_kind() {
        assert_eq!(MutationClass::for_kind(&SegmentKind::SystemPrompt), MutationClass::Frozen);
        assert_eq!(MutationClass::for_kind(&SegmentKind::ToolSchema), MutationClass::Slow);
        assert_eq!(
            MutationClass::for_kind(&SegmentKind::ToolOutput { class: "cargo-test".into() }),
            MutationClass::Fast
        );
        assert_eq!(MutationClass::for_kind(&SegmentKind::ConversationTurn), MutationClass::Fast);
    }

    #[test]
    fn segment_round_trips_via_serde() {
        let s = Segment {
            id: SegmentId(1),
            kind: SegmentKind::FileRead,
            role: Role::Tool,
            bytes: b"hello".to_vec(),
            token_count: 1,
            position: 0,
            mutation_class: MutationClass::Fast,
            origin: Origin::default(),
            protected_spans: vec![],
            refs: RefLedger::default(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Segment = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, SegmentId(1));
        assert_eq!(back.bytes, b"hello");
    }
}
