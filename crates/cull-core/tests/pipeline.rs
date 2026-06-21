// End-to-end: segment a context, plan with the full pass set, emit, check the report.
use cull_core::emit::emit;
use cull_core::passes::{query_passes, structural_passes};
use cull_core::planner::Planner;
use cull_core::segment::{Role, SegmentKind};
use cull_core::segmenter::{segment, RawBlock};
use cull_core::session::SessionState;
use cull_core::task::TaskSignal;
use cull_tokenize::ApproxCounter;

fn raw(role: Role, kind: SegmentKind, text: &str) -> RawBlock {
    RawBlock {
        role,
        kind,
        text: text.to_string(),
        path: None,
    }
}

#[test]
fn full_pipeline_compresses_superseded_and_irrelevant_content() {
    let counter = ApproxCounter::o200k();
    let blocks = vec![
        raw(
            Role::System,
            SegmentKind::SystemPrompt,
            "You are a coding agent working on authentication.",
        ),
        raw(
            Role::Tool,
            SegmentKind::ToolOutput {
                class: "cargo-test".into(),
            },
            "old test run: 3 failed",
        ),
        raw(
            Role::Tool,
            SegmentKind::FileRead,
            "jwt authentication middleware verify token",
        ),
        raw(
            Role::Tool,
            SegmentKind::ToolOutput {
                class: "grep".into(),
            },
            "kubernetes helm chart values yaml registry",
        ),
        raw(
            Role::Tool,
            SegmentKind::ToolOutput {
                class: "cargo-test".into(),
            },
            "new test run: all passed",
        ),
    ];
    let segs = segment(&blocks, &counter);

    // structural (supersession+dedup) + query (relevance) passes
    let mut passes = structural_passes();
    passes.extend(query_passes());
    let task = TaskSignal::from_text("authentication jwt middleware");

    let plan = Planner::new(passes).plan_with_task(&segs, &SessionState::default(), &task);
    let (emitted, report) = emit(&segs, &plan);

    // The old cargo-test output is superseded (dropped); the irrelevant kubernetes grep is dropped
    // if it falls outside the recency window. The system prompt and the relevant jwt read survive.
    assert!(
        report.net_tokens < report.input_tokens,
        "must compress: {} < {}",
        report.net_tokens,
        report.input_tokens
    );
    assert!(
        report.dropped >= 1,
        "at least the superseded test output is dropped"
    );

    // The relevant jwt read must survive (its bytes appear in the emitted output).
    let survived: Vec<String> = emitted
        .iter()
        .map(|e| String::from_utf8_lossy(&e.bytes).into_owned())
        .collect();
    assert!(
        survived
            .iter()
            .any(|t| t.contains("jwt authentication middleware")),
        "relevant read must survive"
    );
    // The system prompt (frozen) must always survive and be first (stability order).
    assert!(
        survived[0].contains("You are a coding agent"),
        "frozen system prompt first"
    );
}
