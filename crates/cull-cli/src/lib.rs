use serde::Deserialize;
use cull_core::segment::{Role, SegmentKind};
use cull_core::segmenter::RawBlock;

#[derive(Debug, Deserialize)]
pub struct InputBlock {
    pub role: String,
    pub kind: String,
    #[serde(default)]
    pub class: Option<String>, // for tool_output
    pub text: String,
}

fn parse_role(s: &str) -> Result<Role, String> {
    match s {
        "system" => Ok(Role::System),
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        "tool" => Ok(Role::Tool),
        other => Err(format!("unknown role: {other}")),
    }
}

fn parse_kind(kind: &str, class: &Option<String>) -> Result<SegmentKind, String> {
    Ok(match kind {
        "system_prompt" => SegmentKind::SystemPrompt,
        "tool_schema" => SegmentKind::ToolSchema,
        "file_read" => SegmentKind::FileRead,
        "dir_listing" => SegmentKind::DirListing,
        "diff" => SegmentKind::Diff,
        "tool_output" => SegmentKind::ToolOutput { class: class.clone().unwrap_or_else(|| "unknown".into()) },
        "stack_trace" => SegmentKind::StackTrace,
        "test_output" => SegmentKind::TestOutput,
        "reasoning_trace" => SegmentKind::ReasoningTrace,
        "conversation_turn" => SegmentKind::ConversationTurn,
        "compact_summary" => SegmentKind::CompactSummary,
        other => return Err(format!("unknown kind: {other}")),
    })
}

/// Parse a JSON array of input blocks into engine `RawBlock`s.
pub fn parse_blocks(json: &str) -> Result<Vec<RawBlock>, String> {
    let input: Vec<InputBlock> = serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
    input.into_iter().map(|b| {
        Ok(RawBlock {
            role: parse_role(&b.role)?,
            kind: parse_kind(&b.kind, &b.class)?,
            text: b.text,
        })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cull_core::segment::{Role, SegmentKind};

    #[test]
    fn parses_roles_and_kinds_including_tool_output_class() {
        let json = r#"[
            {"role":"system","kind":"system_prompt","text":"hi"},
            {"role":"tool","kind":"tool_output","class":"cargo-test","text":"ok"},
            {"role":"tool","kind":"file_read","text":"fn main(){}"}
        ]"#;
        let blocks = parse_blocks(json).unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].role, Role::System);
        assert!(matches!(blocks[0].kind, SegmentKind::SystemPrompt));
        assert!(matches!(&blocks[1].kind, SegmentKind::ToolOutput { class } if class == "cargo-test"));
        assert!(matches!(blocks[2].kind, SegmentKind::FileRead));
    }

    #[test]
    fn unknown_kind_is_an_error() {
        let json = r#"[{"role":"tool","kind":"nonsense","text":"x"}]"#;
        assert!(parse_blocks(json).is_err());
    }
}
