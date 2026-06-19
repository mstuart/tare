use serde::Deserialize;
use cull_core::segment::{Role, SegmentKind};
use cull_core::segmenter::{segment, RawBlock};
use cull_core::planner::Planner;
use cull_core::passes::{structural_passes, query_passes};
use cull_core::session::SessionState;
use cull_core::task::TaskSignal;
use cull_core::emit::{emit, FidelityReport};
use cull_tokenize::ApproxCounter;

pub struct CompressOutput {
    pub compressed: String,
    pub report: FidelityReport,
}

/// Run the full pipeline on a JSON context + task: segment, plan (structural + query passes),
/// emit, and join the surviving segments into the compressed context string.
pub fn run_compress(blocks_json: &str, task: &str) -> Result<CompressOutput, String> {
    let blocks = parse_blocks(blocks_json)?;
    let counter = ApproxCounter::o200k();
    let segs = segment(&blocks, &counter);

    let mut passes = structural_passes();
    passes.extend(query_passes());
    let task_sig = TaskSignal::from_text(task);

    let plan = Planner::new(passes).plan_with_task(&segs, &SessionState::default(), &task_sig);
    let (emitted, report) = emit(&segs, &plan);

    let compressed = emitted.iter()
        .map(|e| String::from_utf8_lossy(&e.bytes).into_owned())
        .collect::<Vec<_>>()
        .join("\n---\n");

    Ok(CompressOutput { compressed, report })
}

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
    fn run_compress_drops_superseded_and_reports() {
        let json = r#"[
            {"role":"tool","kind":"tool_output","class":"cargo-test","text":"old run failed"},
            {"role":"tool","kind":"tool_output","class":"cargo-test","text":"new run passed all"}
        ]"#;
        let out = run_compress(json, "run the tests").unwrap();
        // old cargo-test superseded => fewer net tokens than input, at least one drop
        assert!(out.report.net_tokens < out.report.input_tokens);
        assert!(out.report.dropped >= 1);
        // the surviving (new) output text is present
        assert!(out.compressed.contains("new run passed all"));
        assert!(!out.compressed.contains("old run failed"));
    }

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
