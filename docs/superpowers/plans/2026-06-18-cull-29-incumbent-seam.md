# Cull — Plan 29: Real-Incumbent Shell-Out Seam + LLMLingua-2 Adapter (§12)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` checkboxes.

**Goal:** Close the last §12 ❌ — real-incumbent baselines. The spec is explicit: "invoked uniformly via a CLI seam (the harness shells out to the Python/Node baselines — we do **not** reimplement them)." So the deliverable is the **seam**: a `ShellCompressor` that runs any external compressor as a subprocess behind the existing `Compressor` trait, plus a grounded LLMLingua-2 adapter.

**Architecture:** `ShellCompressor { name, program, args }` pipes the joined context to a subprocess on **stdin**, passes task/budget via **env vars** (`CULL_TASK`/`CULL_BUDGET`) so stdin stays clean, and reads the compressed context from **stdout**. Any spawn/non-zero-exit ⇒ passthrough (the harness never crashes); `probe()` runs a trivial input to decide inclusion. `run_benchmark_with(corpus, budget, extra)` runs the built-ins plus any provided incumbents. The `cull-bench` binary probes the LLMLingua-2 adapter and includes it when available, else prints an honest "unavailable: <reason>". Tests use coreutils (`tr`, `cat`) — no real incumbent needed.

**Honesty note (ledger):** LLMLingua-2 has a grounded adapter (`pip install llmlingua`; documented `PromptCompressor.compress_prompt` API). Headroom/Tamp package identities/APIs are unconfirmed, so they get the same seam + a documented template, not a fabricated adapter.

**Tech:** Rust `std::process`, `cull-bench`. Reference: spec §12 (CLI seam).

---

### Task 1: `ShellCompressor` seam

**Files:** modify `crates/cull-bench/src/lib.rs` (add the struct + impl + tests).

- [ ] **Step 1 — failing tests.** Add to the `tests` module in `lib.rs`:
```rust
    #[test]
    fn shell_compressor_transforms_via_stdin_stdout() {
        // `tr a-z A-Z` uppercases stdin -> proves the pipe works; env task/budget are ignored by tr
        let c = ShellCompressor::new("upper", "tr", vec!["a-z".into(), "A-Z".into()]);
        let blocks = vec![tool("grep", "hello world")];
        let r = c.compress(&blocks, "task", 60);
        assert!(r.text.contains("HELLO WORLD"), "stdin->stdout transform: {}", r.text);
        assert!(r.net_tokens > 0);
    }

    #[test]
    fn shell_compressor_missing_program_passes_through() {
        let c = ShellCompressor::new("nope", "cull-no-such-binary-xyz", vec![]);
        let blocks = vec![tool("grep", "alpha beta gamma")];
        let r = c.compress(&blocks, "task", 60);
        assert!(r.text.contains("alpha beta gamma"), "missing program -> passthrough");
        assert!(c.probe().is_err(), "probe reports the missing program");
    }

    #[test]
    fn shell_compressor_probe_ok_for_cat() {
        let c = ShellCompressor::new("cat-pass", "cat", vec![]);
        assert!(c.probe().is_ok(), "cat is available and echoes input");
    }
```

- [ ] **Step 2 — confirm FAIL** (`cargo test -p cull-bench shell_compressor` — type missing).

- [ ] **Step 3 — implement.** Add to `lib.rs` (after the `Cull` compressor, before `BoardRow`):
```rust
use std::io::Write;
use std::process::{Command, Stdio};

/// Shell-out compressor seam (spec §12): runs an external compressor (LLMLingua-2, Tamp, …) as a
/// subprocess behind the `Compressor` trait. The joined context goes to stdin; the task and budget
/// go via env (`CULL_TASK`/`CULL_BUDGET`) so stdin stays clean; the compressed context is read from
/// stdout. Any spawn error or non-zero exit ⇒ passthrough (the harness never fails because a
/// baseline is missing). We do NOT reimplement the baselines — adapters are thin scripts.
pub struct ShellCompressor {
    name: &'static str,
    program: String,
    args: Vec<String>,
}

impl ShellCompressor {
    pub fn new(name: &'static str, program: impl Into<String>, args: Vec<String>) -> Self {
        Self { name, program: program.into(), args }
    }

    fn run(&self, task: &str, budget: u32, input: &str) -> Result<String, String> {
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .env("CULL_TASK", task)
            .env("CULL_BUDGET", budget.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", self.program))?;
        {
            let mut si = child.stdin.take().ok_or("no stdin handle")?;
            si.write_all(input.as_bytes()).map_err(|e| e.to_string())?;
        } // stdin dropped here -> EOF
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("{} exit {:?}: {}", self.program, out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Is this baseline runnable here? Runs a trivial input; Ok iff exit 0 with non-empty output.
    pub fn probe(&self) -> Result<(), String> {
        match self.run("probe", 1, "probe input text\n") {
            Ok(s) if !s.trim().is_empty() => Ok(()),
            Ok(_) => Err("produced empty output".to_string()),
            Err(e) => Err(e),
        }
    }
}

impl Compressor for ShellCompressor {
    fn name(&self) -> &'static str { self.name }
    fn compress(&self, blocks: &[RawBlock], task: &str, budget: u32) -> CompressResult {
        let input = blocks.iter().map(|b| b.text.clone()).collect::<Vec<_>>().join("\n");
        let text = self.run(task, budget, &input).unwrap_or(input); // failure -> passthrough
        let net = ApproxCounter::o200k().count(&text) as u32;
        CompressResult { text, net_tokens: net }
    }
}
```

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-bench shell_compressor`), then `cargo test --workspace`.

- [ ] **Step 5 — commit.** `git add crates/cull-bench && git commit -m "feat(bench): ShellCompressor seam — run external baselines as subprocesses (§12)"`

---

### Task 2: `run_benchmark_with` + LLMLingua-2 adapter + binary wiring

**Files:** modify `crates/cull-bench/src/lib.rs` (refactor `run_benchmark`); create `crates/cull-bench/adapters/llmlingua2.py`; modify `crates/cull-bench/src/main.rs`.

- [ ] **Step 1 — failing test.** Add to the `tests` module:
```rust
    #[test]
    fn run_benchmark_with_includes_extra_compressors() {
        let extra: Vec<Box<dyn Compressor>> = vec![Box::new(ShellCompressor::new("cat-pass", "cat", vec![]))];
        let board = run_benchmark_with(&corpus(), 60, extra);
        assert_eq!(board.len(), 4); // 3 built-ins + cat-pass
        let cat = board.iter().find(|r| r.name == "cat-pass").expect("cat-pass row present");
        // cat is a passthrough -> every needle survives -> downstream fidelity 1.0
        assert_eq!(cat.downstream_fidelity, 1.0);
    }
```

- [ ] **Step 2 — confirm FAIL** (`run_benchmark_with` missing).

- [ ] **Step 3 — refactor.** Replace the `run_benchmark` fn so the built-in version delegates to a new `run_benchmark_with` that appends `extra` compressors:
```rust
/// Run the three built-in compressors over the corpus at a fixed budget.
pub fn run_benchmark(corpus: &[BenchItem], budget: u32) -> Vec<BoardRow> {
    run_benchmark_with(corpus, budget, Vec::new())
}

/// Like `run_benchmark`, plus any external baselines (shell-out incumbents) appended to the board.
pub fn run_benchmark_with(corpus: &[BenchItem], budget: u32, extra: Vec<Box<dyn Compressor>>) -> Vec<BoardRow> {
    let counter = ApproxCounter::o200k();
    let mut compressors: Vec<Box<dyn Compressor>> =
        vec![Box::new(NoCompression), Box::new(NaiveTruncation), Box::new(Cull)];
    compressors.extend(extra);

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
(This is the existing `run_benchmark` body, moved into `run_benchmark_with` with the `extra` extension. Do NOT change the metric logic.)

- [ ] **Step 4 — confirm PASS** (`cargo test -p cull-bench`), then `cargo test --workspace`.

- [ ] **Step 5 — create the adapter** `crates/cull-bench/adapters/llmlingua2.py`:
```python
#!/usr/bin/env python3
# LLMLingua-2 shell-out adapter for cull-bench (spec §12).
# Reads the context on stdin, reads CULL_TASK / CULL_BUDGET from the env, writes the compressed
# context to stdout. Requires `pip install llmlingua`. If the package is unavailable it exits
# non-zero so the harness's probe() excludes it (rather than recording a misleading passthrough).
import sys
def main():
    context = sys.stdin.read()
    try:
        from llmlingua import PromptCompressor
    except Exception as e:
        sys.stderr.write(f"llmlingua not installed: {e}\n")
        sys.exit(3)
    compressor = PromptCompressor(
        model_name="microsoft/llmlingua-2-xlm-roberta-large-meetingbank",
        use_llmlingua2=True,
    )
    result = compressor.compress_prompt(context, rate=0.5, force_tokens=["\n", ".", ",", "?", "!"])
    sys.stdout.write(result.get("compressed_prompt", context))
if __name__ == "__main__":
    main()
```

- [ ] **Step 6 — wire the binary.** Replace `crates/cull-bench/src/main.rs` with:
```rust
use cull_bench::{corpus, render_board, run_benchmark_with, Compressor, ShellCompressor};

fn main() {
    let budget: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(60);

    // Real-incumbent shell-out seam (spec §12): probe each adapter; include the ones available here.
    let mut extra: Vec<Box<dyn Compressor>> = Vec::new();
    let llmlingua = ShellCompressor::new(
        "llmlingua-2", "python3",
        vec![concat!(env!("CARGO_MANIFEST_DIR"), "/adapters/llmlingua2.py").to_string()],
    );
    match llmlingua.probe() {
        Ok(()) => { eprintln!("[bench] llmlingua-2: available — included"); extra.push(Box::new(llmlingua)); }
        Err(e) => eprintln!("[bench] llmlingua-2: unavailable — skipped ({e})"),
    }

    let board = run_benchmark_with(&corpus(), budget, extra);
    println!("Cull benchmark — budget {budget} tokens, {} items\n", corpus().len());
    print!("{}", render_board(&board));
    println!("\nLower ratio = more compressed; higher fidelity = task-relevant content preserved.");
    println!("(Incumbents run via the ShellCompressor seam — adapters in crates/cull-bench/adapters/;");
    println!(" install one, e.g. `pip install llmlingua`, and it appears in the board automatically.)");
}
```

- [ ] **Step 7 — confirm.** `cargo test --workspace` green. Run `cargo run -p cull-bench 2>&1` and confirm: it prints `[bench] llmlingua-2: unavailable — skipped (...)` (llmlingua isn't installed) AND still prints the 3-row board. Paste the output.

- [ ] **Step 8 — commit.** `git add crates/cull-bench && git commit -m "feat(bench): run_benchmark_with + LLMLingua-2 adapter, auto-include available incumbents (§12)"`

---

## After this plan — ledger
- ✅ §12 real-incumbent baselines — `ShellCompressor` seam (spec's "uniform CLI seam") + LLMLingua-2 adapter; the binary auto-includes any installed incumbent, else reports it unavailable. **Runtime:** the orchestrator will attempt `pip install llmlingua` and record whether the live run executes here (Python 3.14 + torch wheels are the likely blocker). Headroom/Tamp: same seam, adapters pending verified packages/APIs.

## Self-Review
- The seam is the spec deliverable; it's built + tested with coreutils (no incumbent needed for green tests). ✓
- Failure-tolerant: missing/erroring incumbent ⇒ passthrough or probe-exclusion, never a harness crash. ✓
- Adapter exits non-zero when llmlingua is absent ⇒ probe excludes it (no misleading passthrough row). ✓
- No fabricated Headroom/Tamp adapters — honest seam + template only. ✓
