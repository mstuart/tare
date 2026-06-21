//! Cull's context-compression engine: segmentation, the pass pipeline and planner, lossless
//! transforms (columnar JSON/log, exact/envelope dedup, IVM delta, supersession), opt-in lossy
//! compaction, and AST code skeletonization.

pub mod protected;
pub mod segment;
pub mod segmenter;
pub mod session;

pub use protected::{detect_protected_spans, ProtectedKind, ProtectedSpan};
pub use segment::*;
pub use segmenter::{segment, RawBlock};
pub use session::SessionState;
pub mod plan;
pub use plan::{
    apply_unified_diff, replace_is_lossless, CompressionPlan, DropReason, PlanEntry, Reconstruct,
    SegmentAction,
};
pub mod planner;
pub use planner::{stability_order, Pass, PlanCtx, Planner};
pub mod task;
pub use task::TaskSignal;
pub mod passes;
pub use passes::SupersessionPass;
pub mod engine;
pub use passes::EnvelopeDedupPass;
pub use passes::ExactDedupPass;
pub use passes::IvmDeltaPass;
pub use passes::ReasoningTracePass;
pub use passes::RelevancePass;
pub mod embed;
#[cfg(feature = "neural-embed")]
pub mod embed_neural;
pub mod json_crush;
pub mod log_crush;
pub mod lossy_compact;
pub mod predicate;
pub mod schema_slim;
pub mod telegraphic;
pub use predicate::narrow_tool_call;
pub mod code;
pub mod code_skeleton;
pub mod emit;
pub use emit::{emit, EmittedSegment, FidelityReport};
