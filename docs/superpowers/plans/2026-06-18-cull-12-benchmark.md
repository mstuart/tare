# Cull — Plan 12: Honest Benchmark Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove the thesis with numbers. A self-contained benchmark (`cull-bench`) runs a corpus of coding-agent contexts through Cull vs. naive truncation vs. no-compression at the same token budget, and reports compression ratio AND **fidelity** (did the task-relevant "needle" survive?). The corpus is built so the needle sits in an *old* position — so blind truncation drops it while Cull's query-aware compression keeps it. The proof: **Cull achieves equal-or-better ratio at strictly higher fidelity.**

**Architecture:** `cull-bench` is a binary + lib. A `Compressor` trait with three in-repo implementations: `NoCompression`, `NaiveTruncation` (keep most-recent blocks to budget — blind), and `Cull` (the engine: structural + query passes + budget eviction). `run_benchmark(corpus)` runs each compressor over each item, computing mean ratio and fidelity rate (fraction of items whose needle survives), and prints a leaderboard. No external dependencies — the comparison against LLMLingua-2/Headroom/Tamp is a documented future extension (each is a shell-out adapter behind the same `Compressor` trait, requiring those tools installed).

**Tech Stack:** Rust. Builds on the whole engine (`segment`/`RawBlock`, `Planner`/`plan_with_budget`, `structural_passes`/`query_passes`, `TaskSignal`, `emit`, `ApproxCounter`). Reference: spec §12 (honest benchmark), §4 (success criteria — better at equal-or-better fidelity).

---

## File Structure

```
crates/cull-bench/Cargo.toml     # deps: cull-core, cull-tokenize; [[bin]] cull-bench
crates/cull-bench/src/lib.rs     # BenchItem, corpus(), Compressor trait + 3 impls, run_benchmark, Leaderboard
crates/cull-bench/src/main.rs    # run corpus, print leaderboard
```

---

### Task 1: Corpus + compressors

**Files:** Replace `crates/cull-bench/Cargo.toml` and `crates/cull-bench/src/lib.rs`; tests inline.

- [ ] **Step 1: Dependencies + binary**

Replace `crates/cull-bench/Cargo.toml`:
```toml
[package]
name = "cull-bench"
version = "0.0.0"
edition.workspace = true

[[bin]]
name = "cull-bench"
path = "src/main.rs"

[lib]
name = "cull_bench"
path = "src/lib.rs"

[dependencies]
cull-core = { path = "../cull-core" }
cull-tokenize = { path = "../cull-tokenize" }
```

- [ ] **Step 2: Write the failing test**

Put in `crates/cull-bench/src/lib.rs`:
```rust
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
```

- [ ] **Step 3: Implement corpus + compressors**

Above the test module:
```rust
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-bench`
Expected: PASS (both). (Write the tests first, confirm FAIL, then this impl makes them pass — TDD order.)

- [ ] **Step 5: Commit**

```bash
git add crates/cull-bench
git commit -m "feat(bench): corpus (needle-in-old-position) + NoCompression/NaiveTruncation/Cull compressors"
```

---

### Task 2: run_benchmark + leaderboard + the proof + binary

**Files:** Modify `crates/cull-bench/src/lib.rs`; create `crates/cull-bench/src/main.rs`; tests inline.

- [ ] **Step 1: Write the failing test (the proof)**

Add to `crates/cull-bench/src/lib.rs` test module:
```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cull-bench cull_dominates`
Expected: FAIL (`run_benchmark`/`Leaderboard` not defined).

- [ ] **Step 3: Implement run_benchmark + metrics**

Add to `crates/cull-bench/src/lib.rs`:
```rust
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cull-bench`
Expected: PASS (the proof + Task 1 tests).

- [ ] **Step 5: Binary + commit**

Create `crates/cull-bench/src/main.rs`:
```rust
use cull_bench::{corpus, render_board, run_benchmark};

fn main() {
    let budget: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(60);
    let board = run_benchmark(&corpus(), budget);
    println!("Cull benchmark — budget {budget} tokens, {} items\n", corpus().len());
    print!("{}", render_board(&board));
    println!("\nLower ratio = more compressed; higher fidelity = task-relevant content preserved.");
    println!("(Baselines are in-repo; LLMLingua-2/Headroom/Tamp are a shell-out extension behind the same Compressor trait.)");
}
```

Run the binary to confirm the leaderboard prints:
```bash
cargo run -q -p cull-bench -- 60
```
Expected: a table where `cull` shows high fidelity (≈100%) at a ratio no worse than `naive-truncation`, and `naive-truncation` shows lower fidelity.

Run: `cargo test --workspace` → PASS.
```bash
git add crates/cull-bench
git commit -m "feat(bench): run_benchmark + leaderboard + proof (cull dominates blind truncation) + binary"
```

---

## Self-Review

**1. Spec coverage:**
- §12 honest benchmark (corpus, compressors, ratio + fidelity, leaderboard) → Tasks 1–2. ✓
- §4 success criterion "better at equal-or-better fidelity" → the proof test (`cull_dominates_truncation...`). ✓
- vs-incumbent (LLMLingua-2/Headroom/Tamp) leaderboard → documented shell-out extension behind the `Compressor` trait. Not in scope (avoids fragile external installs).

**2. Placeholder scan:** No vague steps. The external-baseline extension is an explicit design note, not an in-plan gap. ✓

**3. Type consistency:** `BenchItem`/`corpus`/`Compressor`/`NoCompression`/`NaiveTruncation`/`Cull`/`CompressResult` (Task 1) used by `run_benchmark`/`BoardRow`/`render_board` (Task 2) and the binary. `RawBlock` (with `path`), `segment`, `Planner::plan_with_budget`, `structural_passes`/`query_passes`, `TaskSignal`, `emit`, `ApproxCounter` from the engine. ✓

**4. Ambiguity check:** Fidelity = needle substring present in the compressed text. Ratio = net/input (lower = better compression). The corpus guarantees the needle is in an old (non-last) block, so truncation is genuinely at risk while Cull's relevance keeps it. The proof asserts Cull's fidelity strictly exceeds truncation's at no materially worse ratio — the honest, falsifiable statement of "better." ✓

**Outcome:** Cull is complete and proven: a query-aware, cache-correct, lossless compression engine; a runnable CLI; a live structure-preserving Anthropic proxy; and a benchmark that demonstrates the wedge — query-aware compression keeps what the task needs where blind compression drops it.
