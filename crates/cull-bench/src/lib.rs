use cull_core::segment::{Role, SegmentKind};
use cull_core::segmenter::{segment, RawBlock};
use cull_core::planner::Planner;
use cull_core::passes::{structural_passes, query_passes};
use cull_core::session::SessionState;
use cull_core::task::TaskSignal;
use cull_core::emit::emit;
use cull_tokenize::{ApproxCounter, TokenCounter};

pub struct BenchItem {
    pub name: &'static str,
    pub blocks: Vec<RawBlock>,
    pub task: &'static str,
    pub needle: &'static str,
}

fn tool(class: &str, text: &str) -> RawBlock {
    RawBlock { role: Role::Tool, kind: SegmentKind::ToolOutput { class: class.into() }, text: text.into(), path: None }
}
fn file(path: &str, text: &str) -> RawBlock {
    RawBlock { role: Role::Tool, kind: SegmentKind::FileRead, text: text.into(), path: Some(path.into()) }
}

/// Built-in corpus. In every item the needle (task-relevant content) sits in an OLD block,
/// followed by several irrelevant recent blocks — the case where blind truncation fails.
pub fn corpus() -> Vec<BenchItem> {
    vec![
        BenchItem {
            name: "auth-bug",
            task: "fix the authentication jwt token expiry bug",
            needle: "TokenExpiredError in auth/jwt.rs verify",
            blocks: vec![
                file("auth/jwt.rs", "fn verify() { /* TokenExpiredError in auth/jwt.rs verify path */ }"),
                tool("grep", "kubernetes helm chart values registry ingress unrelated noise one"),
                tool("grep", "grafana dashboard prometheus metrics scrape config unrelated two"),
                tool("ls", "node_modules dist build cache target coverage unrelated three"),
                tool("git-status", "modified: README.md docs/CHANGELOG unrelated four"),
            ],
        },
        BenchItem {
            name: "db-pool",
            task: "investigate the postgres connection pool exhaustion",
            needle: "connection pool exhausted max_connections=20 in db/pool.rs",
            blocks: vec![
                file("db/pool.rs", "// connection pool exhausted max_connections=20 in db/pool.rs under load"),
                tool("grep", "frontend react component css tailwind unrelated alpha"),
                tool("test", "passed 40 tests in ui module unrelated beta"),
                tool("ls", "assets images fonts public unrelated gamma"),
                tool("cargo-build", "compiling crate features serde tokio unrelated delta"),
            ],
        },
        BenchItem {
            name: "race-condition",
            task: "fix the data race in the cache writer",
            needle: "data race: cache/writer.rs concurrent write without lock",
            blocks: vec![
                file("cache/writer.rs", "// data race: cache/writer.rs concurrent write without lock detected"),
                tool("grep", "documentation markdown sphinx readthedocs unrelated x"),
                tool("npm", "audit found 0 vulnerabilities in 1200 packages unrelated y"),
                tool("ls", "examples samples templates unrelated z"),
            ],
        },
    ]
}

pub struct CompressResult { pub text: String, pub net_tokens: u32 }

pub trait Compressor {
    fn name(&self) -> &'static str;
    fn compress(&self, blocks: &[RawBlock], task: &str, budget: u32) -> CompressResult;
}

pub struct NoCompression;
impl Compressor for NoCompression {
    fn name(&self) -> &'static str { "no-compression" }
    fn compress(&self, blocks: &[RawBlock], _task: &str, _budget: u32) -> CompressResult {
        let counter = ApproxCounter::o200k();
        let text = blocks.iter().map(|b| b.text.clone()).collect::<Vec<_>>().join("\n");
        let net = blocks.iter().map(|b| counter.count(&b.text) as u32).sum();
        CompressResult { text, net_tokens: net }
    }
}

/// Blind: keep the most-recent blocks until the budget is reached. Drops oldest first.
pub struct NaiveTruncation;
impl Compressor for NaiveTruncation {
    fn name(&self) -> &'static str { "naive-truncation" }
    fn compress(&self, blocks: &[RawBlock], _task: &str, budget: u32) -> CompressResult {
        let counter = ApproxCounter::o200k();
        let mut kept: Vec<&RawBlock> = Vec::new();
        let mut total = 0u32;
        for b in blocks.iter().rev() {
            let t = counter.count(&b.text) as u32;
            if !kept.is_empty() && total + t > budget { break; }
            kept.push(b);
            total += t;
        }
        kept.reverse();
        let text = kept.iter().map(|b| b.text.clone()).collect::<Vec<_>>().join("\n");
        CompressResult { text, net_tokens: total }
    }
}

/// Cull: the full engine — structural + query passes + budget eviction.
pub struct Cull;
impl Compressor for Cull {
    fn name(&self) -> &'static str { "cull" }
    fn compress(&self, blocks: &[RawBlock], task: &str, budget: u32) -> CompressResult {
        let counter = ApproxCounter::o200k();
        let segs = segment(blocks, &counter);
        let mut passes = structural_passes();
        passes.extend(query_passes());
        let plan = Planner::new(passes).plan_with_budget(
            &segs, &SessionState::default(), &TaskSignal::from_text(task), Some(budget));
        let (emitted, report) = emit(&segs, &plan);
        let text = emitted.iter().map(|e| String::from_utf8_lossy(&e.bytes).into_owned())
            .collect::<Vec<_>>().join("\n");
        CompressResult { text, net_tokens: report.net_tokens }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_needles_are_in_old_positions() {
        // each item's needle is present in the context but NOT in the last block,
        // so blind truncation is at risk of dropping it.
        for item in corpus() {
            assert!(item.blocks.iter().any(|b| b.text.contains(item.needle)),
                "{}: needle present", item.name);
            let last = item.blocks.last().unwrap();
            assert!(!last.text.contains(item.needle), "{}: needle not in last block", item.name);
        }
    }

    #[test]
    fn cull_and_truncation_both_compress_to_budget_ballpark() {
        let item = &corpus()[0];
        let budget = 60;
        let c = Cull.compress(&item.blocks, item.task, budget);
        let t = NaiveTruncation.compress(&item.blocks, item.task, budget);
        // both reduce vs no-compression
        let full = NoCompression.compress(&item.blocks, item.task, budget);
        assert!(c.net_tokens < full.net_tokens);
        assert!(t.net_tokens <= full.net_tokens);
    }
}
