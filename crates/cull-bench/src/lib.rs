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
    /// Exact values the correct next tool-call must reference (path, error code, number).
    /// Tool-call fidelity = all of these survive byte-exact in the compressed context.
    pub tool_params: &'static [&'static str],
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
            tool_params: &["auth/jwt.rs"],
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
            tool_params: &["db/pool.rs", "max_connections=20"],
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
            tool_params: &["cache/writer.rs"],
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
        BenchItem {
            name: "null-deref",
            task: "fix the null pointer dereference in getProfile",
            needle: "NullPointerException handlers/user.go getProfile nil receiver",
            tool_params: &["handlers/user.go"],
            blocks: vec![
                file("handlers/user.go",
                    "// NullPointerException handlers/user.go getProfile nil receiver on session"),
                tool("grep", "vue nuxt pinia vuex composition options sfc template-a"),
                tool("ls",   "cypress selenium webdriver puppeteer headless e2e suite-a"),
                tool("grep", "celery rq sidekiq resque bull agenda cron scheduler-b"),
                tool("grep", "elasticsearch opensearch solr lucene index shard analyzer-b"),
                tool("ls",   "packer vagrant virtualbox qemu kvm libvirt image box-c"),
                tool("grep", "consul etcd zookeeper vault nomad service-mesh discovery-c"),
                tool("ls",   "maven gradle ant ivy sbt bazel buck pants build-d"),
                tool("grep", "spark hadoop flink beam airflow dbt warehouse etl-d"),
                tool("ls",   "cocoapods carthage spm gradle xcconfig provisioning-e"),
            ],
        },
        BenchItem {
            name: "exact-timeout",
            task: "report the server read timeout configuration value",
            needle: "timeout=30000 config/server.toml read timeout milliseconds",
            tool_params: &["config/server.toml", "timeout=30000"],
            blocks: vec![
                file("config/server.toml",
                    "# timeout=30000 config/server.toml read timeout milliseconds for upstream"),
                tool("grep", "tailwind bootstrap bulma foundation materialize semantic-a"),
                tool("ls",   "rollup vite parcel esbuild swc turbopack bundler dist-a"),
                tool("grep", "keycloak auth0 okta cognito firebase oidc saml sso-b"),
                tool("grep", "minio s3 gcs azure-blob ceph swift object storage-b"),
                tool("ls",   "grafana kibana datadog splunk sumologic dashboard panel-c"),
                tool("grep", "istio linkerd consul-connect cilium sidecar mtls mesh-c"),
                tool("ls",   "argocd flux spinnaker tekton jenkins-x gitops deploy-d"),
                tool("grep", "cassandra scylla dynamo bigtable hbase wide-column-d"),
                tool("ls",   "protobuf thrift avro flatbuffers capnp msgpack codec-e"),
            ],
        },
        BenchItem {
            name: "error-code",
            task: "handle the ERR_CONN_REFUSED connection retry path",
            needle: "ERR_CONN_REFUSED net/dialer.rs retry backoff exhausted",
            tool_params: &["net/dialer.rs", "ERR_CONN_REFUSED"],
            blocks: vec![
                file("net/dialer.rs",
                    "// ERR_CONN_REFUSED net/dialer.rs retry backoff exhausted after 5 attempts"),
                tool("grep", "next remix gatsby astro svelte-kit qwik solid-start-a"),
                tool("ls",   "yarn pnpm npm bun lockfile workspace hoist node-modules-a"),
                tool("grep", "opentelemetry zipkin honeycomb lightstep span context-b"),
                tool("grep", "wireguard openvpn ipsec tailscale zerotier tunnel vpn-b"),
                tool("ls",   "alembic flyway liquibase goose migrate schema version-c"),
                tool("grep", "pgbouncer pgpool patroni citus timescale replication-c"),
                tool("ls",   "fastlane gym match sigh pilot deliver screenshots-d"),
                tool("grep", "numpy pandas polars dask ray modin dataframe vectorize-d"),
                tool("ls",   "webpack-bundle-analyzer source-map-explorer treemap stats-e"),
            ],
        },
        BenchItem {
            name: "codegen-sig",
            task: "implement the parse function with the required signature",
            needle: "fn parse(input: &str) -> Result<Ast, ParseError> parser/mod.rs",
            tool_params: &["parser/mod.rs"],
            blocks: vec![
                file("parser/mod.rs",
                    "// fn parse(input: &str) -> Result<Ast, ParseError> parser/mod.rs entrypoint"),
                tool("grep", "hibernate jpa mybatis jooq diesel sea-orm prisma typeorm-a"),
                tool("ls",   "checkstyle spotbugs pmd errorprone ktlint detekt lint-a"),
                tool("grep", "rabbitmq-streams pulsar redpanda warpstream event-log-b"),
                tool("grep", "terratest kitchen inspec serverspec goss molecule test-b"),
                tool("ls",   "buildkite circleci drone concourse woodpecker pipeline-c"),
                tool("grep", "envoy-gateway contour ambassador kong apisix gateway-c"),
                tool("ls",   "renovate dependabot snyk greenkeeper bump upgrade dep-d"),
                tool("grep", "duckdb clickhouse pinot druid materialize olap query-d"),
                tool("ls",   "storybook ladle histoire styleguidist component docs-e"),
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
    pub mean_ratio: f64,           // mean(net/input); lower = more compressed
    pub downstream_fidelity: f64,  // fraction whose needle (task-relevant content) survived
    pub tool_call_fidelity: f64,   // fraction whose ALL tool_params survived byte-exact
    pub divergence_rate: f64,      // fraction where needle OR a param was lost -> wrong next action
    pub cache_prefix_kept: f64,    // cache-hit proxy: fraction whose stable prefix (block 0) is preserved byte-identical at the output head
}

/// Run every compressor over every corpus item at a fixed budget; aggregate ratio + four fidelity
/// metrics. Metrics are structural (content/param survival + stable-prefix preservation) — the
/// necessary conditions for correct downstream behavior, computed without a live model.
pub fn run_benchmark(corpus: &[BenchItem], budget: u32) -> Vec<BoardRow> {
    let counter = ApproxCounter::o200k();
    let compressors: Vec<Box<dyn Compressor>> =
        vec![Box::new(NoCompression), Box::new(NaiveTruncation), Box::new(Cull)];

    compressors.iter().map(|c| {
        let mut ratios = Vec::new();
        let (mut needle_ok, mut toolcall_ok, mut diverged, mut prefix_ok) = (0usize, 0usize, 0usize, 0usize);
        for item in corpus {
            let input: u32 = item.blocks.iter().map(|b| counter.count(&b.text) as u32).sum();
            let r = c.compress(&item.blocks, item.task, budget);
            ratios.push(if input == 0 { 1.0 } else { r.net_tokens as f64 / input as f64 });
            let needle_kept = r.text.contains(item.needle);
            let params_kept = item.tool_params.iter().all(|p| r.text.contains(p));
            if needle_kept { needle_ok += 1; }
            if params_kept { toolcall_ok += 1; }
            if !(needle_kept && params_kept) { diverged += 1; }
            if let Some(first) = item.blocks.first() {
                if r.text.starts_with(&first.text) { prefix_ok += 1; }
            }
        }
        let n = corpus.len().max(1) as f64;
        BoardRow {
            name: c.name(),
            mean_ratio: ratios.iter().sum::<f64>() / n,
            downstream_fidelity: needle_ok as f64 / n,
            tool_call_fidelity: toolcall_ok as f64 / n,
            divergence_rate: diverged as f64 / n,
            cache_prefix_kept: prefix_ok as f64 / n,
        }
    }).collect()
}

/// Render the leaderboard as a text table.
pub fn render_board(board: &[BoardRow]) -> String {
    let mut s = String::from("compressor        ratio  down-fid  tool-fid  diverge  cache-pfx\n");
    s.push_str("---------------------------------------------------------------------\n");
    for r in board {
        s.push_str(&format!("{:<16} {:>6.3}  {:>7.0}%  {:>7.0}%  {:>6.0}%  {:>8.0}%\n",
            r.name, r.mean_ratio, r.downstream_fidelity * 100.0, r.tool_call_fidelity * 100.0,
            r.divergence_rate * 100.0, r.cache_prefix_kept * 100.0));
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
        assert!(cull.downstream_fidelity > trunc.downstream_fidelity,
            "cull fidelity {} should beat truncation {}", cull.downstream_fidelity, trunc.downstream_fidelity);
        // ...while compressing at least as well (ratio = net/input, lower is more compressed)
        assert!(cull.mean_ratio <= trunc.mean_ratio + 0.05,
            "cull ratio {} not materially worse than truncation {}", cull.mean_ratio, trunc.mean_ratio);
        // and Cull's fidelity is high in absolute terms
        assert!(cull.downstream_fidelity >= 0.99, "cull keeps the needle: {}", cull.downstream_fidelity);
    }

    #[test]
    fn cull_wins_on_divergence_and_cache_prefix() {
        let board = run_benchmark(&corpus(), 60);
        let cull = board.iter().find(|r| r.name == "cull").unwrap();
        let trunc = board.iter().find(|r| r.name == "naive-truncation").unwrap();
        // Cull never diverges (lossless keep of needle + exact params); truncation does
        assert_eq!(cull.divergence_rate, 0.0, "cull divergence {}", cull.divergence_rate);
        assert!(trunc.divergence_rate > 0.0, "truncation should diverge: {}", trunc.divergence_rate);
        // Cull keeps the cacheable stable prefix; blind truncation drops the oldest block -> busts cache
        assert!(cull.cache_prefix_kept > trunc.cache_prefix_kept,
            "cull cache-prefix {} should beat truncation {}", cull.cache_prefix_kept, trunc.cache_prefix_kept);
        // Cull reconstructs the exact tool-call params far more often
        assert!(cull.tool_call_fidelity > trunc.tool_call_fidelity,
            "cull tool-fid {} vs truncation {}", cull.tool_call_fidelity, trunc.tool_call_fidelity);
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
