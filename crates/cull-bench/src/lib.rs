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
/// followed by many irrelevant blocks — enough to overflow the budget and push the needle
/// outside the relevance pass's recency window. Blind truncation (keep most-recent) drops
/// the needle; Cull's query-relevance pass promotes it above the noise.
///
/// Design invariant: ≥ 10 blocks per item so that the needle (position 0) falls outside
/// the RelevancePass recency_keep=6 window. The pass keeps the needle (task-symbol overlap)
/// and drops the irrelevant old blocks; recency guards keep the most-recent noise. Budget
/// eviction then trims both Cull and NaiveTruncation to ≈ budget, making their ratios close.
pub fn corpus() -> Vec<BenchItem> {
    vec![
        BenchItem {
            name: "auth-bug",
            task: "fix the authentication jwt token expiry bug",
            needle: "TokenExpiredError auth/jwt.rs expiry",
            // pos 0: needle (~8 tok), pos 1-3: old irrelevant noise (~20 tok each, dropped by relevance),
            // pos 4-9: recent noise (~20 tok each, kept by recency guard). Total ~130 tok; budget 60.
            blocks: vec![
                file("auth/jwt.rs",
                    "// TokenExpiredError auth/jwt.rs expiry check missing in verify()"),
                tool("grep", "kubernetes helm chart registry ingress deployment replicaset service-a"),
                tool("ls",   "dist build cache .nyc_output coverage __snapshots__ vendor logs-a"),
                tool("grep", "terraform module provider resource variable output tfstate remote-a"),
                tool("grep", "frontend webpack babel postcss tailwind eslint prettier config-b"),
                tool("ls",   "node_modules packages workspaces lerna nx turborepo monorepo-b"),
                tool("grep", "docker compose network volume mount bind tmpfs overlay layer-c"),
                tool("ls",   "migrations seeds fixtures rollback schema baseline history-c"),
                tool("grep", "prometheus alertmanager grafana loki tempo jaeger tracing-d"),
                tool("ls",   "public assets images fonts icons svg sprite manifest sitemap-d"),
            ],
        },
        BenchItem {
            name: "db-pool",
            task: "investigate postgres connection pool exhaustion",
            needle: "connection pool exhausted max_connections=20 db/pool.rs",
            blocks: vec![
                file("db/pool.rs",
                    "// connection pool exhausted max_connections=20 db/pool.rs under load"),
                tool("grep", "frontend react redux saga thunk middleware selector store-a"),
                tool("ls",   "storybook chromatic percy snapshot visual regression tests-a"),
                tool("grep", "ansible playbook inventory role task handler template vault-b"),
                tool("grep", "graphql schema resolver mutation subscription directive-b"),
                tool("ls",   "swagger openapi redoc rapidoc scalar spectral lint spec-c"),
                tool("grep", "rabbitmq kafka pubsub nats jetstream consumer producer queue-c"),
                tool("ls",   "lambda function handler trigger event source mapping arn-d"),
                tool("grep", "redis sentinel cluster shard replica failover eviction-d"),
                tool("ls",   "nginx haproxy traefik envoy caddy proxy upstream backend-e"),
            ],
        },
        BenchItem {
            name: "race-condition",
            task: "fix data race cache writer concurrent lock",
            needle: "data race cache/writer.rs concurrent write without lock",
            blocks: vec![
                file("cache/writer.rs",
                    "// data race cache/writer.rs concurrent write without lock detected"),
                tool("grep", "typescript eslint prettier tsconfig paths alias baseUrl-a"),
                tool("ls",   "vitest jest mocha chai sinon nock supertest playwright-a"),
                tool("grep", "sentry datadog newrelic appdynamics dynatrace apm tracer-b"),
                tool("grep", "stripe paypal braintree mollie adyen payment webhook-b"),
                tool("ls",   "github gitlab bitbucket actions workflow pipeline trigger-c"),
                tool("grep", "terraform cloudformation cdk pulumi bicep arm template-c"),
                tool("ls",   "ecr gcr dockerhub registry pull push tag digest layer-d"),
                tool("grep", "sonarqube snyk dependabot renovate trivy semgrep scan-d"),
                tool("ls",   "helm chart release values override secrets configmap rbac-e"),
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

#[derive(Debug, Clone)]
pub struct BoardRow {
    pub name: &'static str,
    pub mean_ratio: f64,   // mean(net/input); lower = more compressed
    pub fidelity_rate: f64, // fraction of items whose needle survived
}

/// Run every compressor over every corpus item at a fixed budget; aggregate ratio + fidelity.
pub fn run_benchmark(corpus: &[BenchItem], budget: u32) -> Vec<BoardRow> {
    let counter = ApproxCounter::o200k();
    let compressors: Vec<Box<dyn Compressor>> =
        vec![Box::new(NoCompression), Box::new(NaiveTruncation), Box::new(Cull)];

    compressors.iter().map(|c| {
        let mut ratios = Vec::new();
        let mut kept_needle = 0usize;
        for item in corpus {
            let input: u32 = item.blocks.iter().map(|b| counter.count(&b.text) as u32).sum();
            let r = c.compress(&item.blocks, item.task, budget);
            ratios.push(if input == 0 { 1.0 } else { r.net_tokens as f64 / input as f64 });
            if r.text.contains(item.needle) { kept_needle += 1; }
        }
        BoardRow {
            name: c.name(),
            mean_ratio: ratios.iter().sum::<f64>() / ratios.len().max(1) as f64,
            fidelity_rate: kept_needle as f64 / corpus.len().max(1) as f64,
        }
    }).collect()
}

/// Render the leaderboard as a text table.
pub fn render_board(board: &[BoardRow]) -> String {
    let mut s = String::from("compressor        mean_ratio   fidelity\n");
    s.push_str("------------------------------------------------\n");
    for r in board {
        s.push_str(&format!("{:<16}  {:>9.3}   {:>7.0}%\n", r.name, r.mean_ratio, r.fidelity_rate * 100.0));
    }
    s
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
    fn cull_dominates_truncation_better_fidelity_at_no_worse_ratio() {
        let budget = 60;
        let board = run_benchmark(&corpus(), budget);
        let cull = board.iter().find(|r| r.name == "cull").unwrap();
        let trunc = board.iter().find(|r| r.name == "naive-truncation").unwrap();
        // Cull keeps the task-relevant needle far more often than blind truncation
        assert!(cull.fidelity_rate > trunc.fidelity_rate,
            "cull fidelity {} should beat truncation {}", cull.fidelity_rate, trunc.fidelity_rate);
        // ...while compressing at least as well (ratio = net/input, lower is more compressed)
        assert!(cull.mean_ratio <= trunc.mean_ratio + 0.05,
            "cull ratio {} not materially worse than truncation {}", cull.mean_ratio, trunc.mean_ratio);
        // and Cull's fidelity is high in absolute terms
        assert!(cull.fidelity_rate >= 0.99, "cull keeps the needle: {}", cull.fidelity_rate);
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
