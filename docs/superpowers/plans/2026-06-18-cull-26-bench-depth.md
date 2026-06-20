# Cull — Plan 26: Benchmark Depth — fidelity/divergence/cache metrics + corpus (§12)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Close the §12 metric gaps with real, computed, differentiating measures and a bigger/diverse corpus. Add **downstream-task fidelity**, **tool-call fidelity**, **false-negative/divergence rate**, and **cache-hit-rate impact** to the leaderboard; expand the corpus with exact-value-lookup and code-gen items (the spec's "fragile" task types).

**Honesty note (goes in the ledger):** these are **structural** fidelity metrics (content/param survival + stable-prefix preservation) computed without a live model. They are the *necessary conditions* for correct downstream behavior and they cleanly separate the compressors (truncation loses needles/params/prefix; Cull preserves them losslessly). The spec's ideal live-LLM downstream judge and real SWE-bench traces remain **env-gated** (model API / dataset download); the harness keeps the seam to plug them in.

**Architecture:** Add `tool_params` (exact values the correct next tool-call needs) to `BenchItem`. Replace `BoardRow.fidelity_rate` with four named metrics + keep `mean_ratio`. `run_benchmark` computes all four per compressor. `render_board` prints the wider table (the `cull-bench` binary picks it up automatically).

**Tech:** Rust, `cull-bench`. Reference: spec §12.

---

### Task 1: Four metrics + tool_params field

**Files:** modify `crates/cull-bench/src/lib.rs` (struct, corpus item literals, `run_benchmark`, `render_board`, tests).

- [ ] **Step 1 — extend `BenchItem`.** Add a field (after `needle`):
```rust
pub struct BenchItem {
    pub name: &'static str,
    pub blocks: Vec<RawBlock>,
    pub task: &'static str,
    pub needle: &'static str,
    /// Exact values the correct next tool-call must reference (path, error code, number).
    /// Tool-call fidelity = all of these survive byte-exact in the compressed context.
    pub tool_params: &'static [&'static str],
}
```

- [ ] **Step 2 — add `tool_params` to the 3 existing corpus items** (inside `corpus()`), right after each `needle:` line:
  - `auth-bug`: `tool_params: &["auth/jwt.rs"],`
  - `db-pool`: `tool_params: &["db/pool.rs", "max_connections=20"],`
  - `race-condition`: `tool_params: &["cache/writer.rs"],`

- [ ] **Step 3 — replace `BoardRow`** with:
```rust
#[derive(Debug, Clone)]
pub struct BoardRow {
    pub name: &'static str,
    pub mean_ratio: f64,           // mean(net/input); lower = more compressed
    pub downstream_fidelity: f64,  // fraction whose needle (task-relevant content) survived
    pub tool_call_fidelity: f64,   // fraction whose ALL tool_params survived byte-exact
    pub divergence_rate: f64,      // fraction where needle OR a param was lost -> wrong next action
    pub cache_prefix_kept: f64,    // cache-hit proxy: fraction whose stable prefix (block 0) is preserved byte-identical at the output head
}
```

- [ ] **Step 4 — replace `run_benchmark`** with:
```rust
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
```

- [ ] **Step 5 — replace `render_board`** with:
```rust
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
```

- [ ] **Step 6 — fix the existing test** `cull_dominates_truncation_better_fidelity_at_no_worse_ratio`: replace every `.fidelity_rate` with `.downstream_fidelity`. Then APPEND a new test:
```rust
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
```

- [ ] **Step 7 — confirm PASS** (`cargo test -p cull-bench`), then `cargo test --workspace`.

- [ ] **Step 8 — smoke the binary.** Run `cargo run -p cull-bench 2>/dev/null` and confirm the table prints with the new columns and Cull shows 0% diverge. Paste the output.

- [ ] **Step 9 — commit.** `git add crates/cull-bench && git commit -m "feat(bench): downstream/tool-call fidelity, divergence + cache-prefix metrics (§12)"`

---

### Task 2: Expand + diversify the corpus

**Files:** modify `crates/cull-bench/src/lib.rs` (`corpus()` — append 4 items; tests).

- [ ] **Step 1 — append 4 items** to the `vec![ … ]` in `corpus()` (after `race-condition`). Each has the needle at position 0 and 9 distinct noise blocks; needles/params appear in NO other block and NOT in the last block:
```rust
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
```

- [ ] **Step 2 — update the corpus-size assumption** if any test hardcodes 3 items (search `corpus()` length usages; `corpus_needles_are_in_old_positions` iterates, so it's fine). No change expected, but verify.

- [ ] **Step 3 — confirm PASS** (`cargo test -p cull-bench` — the divergence/fidelity tests now run over 7 items and must still hold: Cull divergence 0, truncation > 0). Then `cargo test --workspace`.

- [ ] **Step 4 — smoke** `cargo run -p cull-bench 2>/dev/null`; paste the 7-item board. Confirm Cull: high down-fid/tool-fid, 0% diverge, high cache-pfx; truncation: low fidelity, high diverge, low cache-pfx.

- [ ] **Step 5 — commit.** `git add crates/cull-bench && git commit -m "feat(bench): expand corpus to 7 items incl. exact-value + code-gen tasks (§12)"`

---

## After this plan — ledger
- ✅ §12 downstream-task fidelity, tool-call fidelity, false-negative/divergence rate, cache-hit-rate impact — structural metrics in the leaderboard (live-LLM judge remains an env-gated enhancement; seam preserved).
- ✅ §12 corpus — expanded to 7 diverse items incl. exact-value-lookup + code-gen (real SWE-bench/agent traces remain an env-gated dataset enhancement).
- Still ❌ (env-gated): §12 real-incumbent baselines (LLMLingua-2/Headroom/Tamp shell-out adapters → next plan).

## Self-Review
- Metrics are honest structural proxies, explicitly labeled; no live-LLM over-claim. ✓
- New items follow the existing design invariant (needle at pos 0, ≥10 blocks, needle absent from last block). ✓
- `divergence_rate` and `cache_prefix_kept` are real differentiators (truncation drops oldest → busts prefix + diverges; Cull lossless-keeps). ✓
- `render_board`/binary updated; `BoardRow.fidelity_rate` renamed everywhere it's used. ✓
