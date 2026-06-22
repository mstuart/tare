<div align="center"><pre>
 ██████╗██╗   ██╗██╗     ██╗
██╔════╝██║   ██║██║     ██║
██║     ██║   ██║██║     ██║
██║     ██║   ██║██║     ██║
╚██████╗╚██████╔╝███████╗███████╗
 ╚═════╝ ╚═════╝ ╚══════╝╚══════╝
     Lossless-by-default context compression for LLM coding agents
</pre></div>

<p align="center"><strong>lossless by default · query-aware · cache-correct · closed-loop · proxy · library · CLI · local</strong></p>

<p align="center">
  <a href="https://github.com/mstuart/cull/actions/workflows/ci.yml"><img src="https://github.com/mstuart/cull/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <img src="https://img.shields.io/badge/rust-1.82%2B-orange.svg" alt="Rust 1.82+">
  <img src="https://img.shields.io/badge/status-pre--1.0-yellow.svg" alt="pre-1.0">
</p>

<p align="center">
  <a href="#get-started-60-seconds">Install</a> ·
  <a href="#how-it-works-30-seconds">How it works</a> ·
  <a href="#proof">Proof</a> ·
  <a href="#compared-to">Compared to</a> ·
  <a href="#when-to-use--when-to-skip">When to use</a> ·
  <a href="#status--limitations">Status</a>
</p>

---

cull sits between an agent and the model API and shrinks the context window. Unlike most compressors
it is **lossless by default**, **cache-correct** (it never rewrites the provider's cached prefix), and
**closed-loop** — it watches the model's *output* and adapts. Opt in and it compresses aggressively:
row-capping, field-truncation, telegraphic NL, and AST code skeletonization.

> **Status: pre-1.0, not yet exercised against a live model API.** The engine is well-tested (154
> tests) and beats incumbents on the included benchmarks, but read [Status & limitations](#status--limitations)
> before deploying.

## What it does

- **Proxy** — `cull-proxy`, point your agent's base URL at it; zero code changes, any language.
- **Library** — call the `cull-core` engine directly from Rust.
- **CLI** — `cull compress | slim-schema | compact-lossy | skeletonize` for one-shot transforms.
- **Lossless by default** — re-encodes tool output, logs, and JSON into a denser *equivalent* form;
  it only drops information when you explicitly opt in.
- **Cache-correct** — detects the provider's cache breakpoint and only compresses the dynamic suffix,
  so your 90%-discount prefix cache keeps hitting.
- **Closed-loop** — watches output verbosity (the *compression paradox*), context fill, and cache
  hit-rate, and dials compression up or down per session.

## Why

- **Lossless wins where it can.** Tool output, logs, and JSON re-encode losslessly into a far denser
  form; only drop information when the caller accepts it.
- **The cache is the economy.** Provider prefix caches discount cached tokens ~10×. Perturb the
  cached prefix and a 90% discount becomes 0%. cull only ever rewrites the dynamic suffix.
- **Compression has a feedback loop.** Over-compressing makes models *compensate with verbose
  output*, so total cost can rise even as input falls. cull is the only proxy that watches output and
  backs off.

## How it works (30 seconds)

```
 Your agent / app  (Claude Code, Cursor, Codex, your own loop…)
      │  prompts · tool outputs · logs · file reads · RAG results
      ▼
  ┌──────────────────────────────────────────────────────────┐
  │  cull   (runs locally — your data and API key stay here)  │
  │  ──────────────────────────────────────────────────────  │
  │  cache-boundary detect → only touch the dynamic suffix    │
  │  lossless passes:  supersession · IVM/delta · dedup ·     │
  │                    columnar JSON/log · schema-slim ·      │
  │                    query-relevance                        │
  │  opt-in lossy:     row-cap · field-truncate · telegraphic │
  │                    · AST code skeletonization             │
  │  closed-loop controller:  cache-hit-rate (halt) ·         │
  │                    output-verbosity (back off) ·          │
  │                    context-fill (compress harder)         │
  └──────────────────────────────────────────────────────────┘
      │  compressed request                ▲  response streamed back unchanged
      ▼                                     │  (output tokens observed for the loop)
 LLM provider  (Anthropic /v1/messages · OpenAI /v1/chat/completions)
```

## Get started (60 seconds)

```bash
# 1 — build
git clone https://github.com/mstuart/cull && cd cull
cargo build --release            # builds target/release/{cull, cull-proxy}

# 2 — run as a proxy (point your agent's base URL at http://localhost:8787)
CULL_UPSTREAM=https://api.anthropic.com ./target/release/cull-proxy

# 3 — or use the CLI on any stdin
cat big.rs | ./target/release/cull skeletonize --path big.rs    # drop fn bodies, keep structure
ps aux     | ./target/release/cull compact-lossy --max-rows 30 --max-field 110
```

Proxy env: `CULL_UPSTREAM` (default `https://api.anthropic.com`), `CULL_PORT` (`8787`),
`CULL_RECENCY` (`4`), `CULL_ENABLED` (`true`), `CULL_CONTEXT_LIMIT` (`200000`). Response headers report
what it did: `x-cull-net-tokens`, `x-cull-dropped`, `x-cull-aggression`, `x-cull-verbosity-spike`,
`x-cull-halted`.

## Proof

<p align="center"><img src="docs/assets/savings.svg" alt="cull token reduction by content type" width="680"></p>

Measured in this repo (o200k tokens), reproducible with the commands shown:

| What | Result | Reproduce |
|---|---:|---|
| **AST code skeletonization** on 31 real Rust files | **65.1%** smaller (57,199 → 19,988) | `cull skeletonize --path <f>` |
| **`ps aux`** vs RTK at equal fidelity (same rows + columns) | **~4.5% smaller**, 5/5 trials | `ps aux \| cull compact-lossy --max-rows 30 --max-field 110` |
| **Lossless** JSON / log columnar re-encode | smaller, **byte-recoverable** | `cull compress` |

Code reads are ~67–76% of a coding agent's tokens ([SWE-Pruner, ACL 2026](https://arxiv.org/abs/2601.16746)),
so skeletonization is the single biggest lever. Competitive head-to-head harnesses (vs Headroom,
LLMLingua-2, lean-ctx, RTK) live in `crates/cull-bench/benchmarks/`; at **equal fidelity** cull matches
or beats each on every content type, and is the only one with a lossless mode and cross-turn dedup.

## Compared to

cull runs **locally**, is **lossless by default**, is **cache-correct**, and **closes the loop** on
output — none of the others do all four.

|  | Scope | Deploy | Local | Lossless default | Output-aware |
|---|---|---|:-:|:-:|:-:|
| **cull** | tools · logs · files · JSON · history | proxy · library · CLI | ✅ | ✅ | ✅ |
| [Headroom](https://github.com/chopratejas/headroom) | all context | proxy · lib · MCP | ✅ | ❌ (reversible via cache) | ❌ |
| [RTK](https://github.com/rtk-ai/rtk) | CLI command outputs | CLI wrapper | ✅ | ❌ | ❌ |
| [lean-ctx](https://github.com/yvgude/lean-ctx) | CLI commands, MCP tools | CLI · MCP | ✅ | ❌ | ❌ |
| LLMLingua-2 | prose / RAG | library (ML model) | ✅ | ❌ | ❌ |
| OpenAI / Anthropic native compaction | conversation history | provider-native | ❌ | ❌ | ❌ |

## When to use · When to skip

**Great fit if you…**
- run coding agents and want savings without losing information by default
- care about the provider cache staying warm (cull won't break the prefix)
- want code reads compressed structurally (signatures kept, bodies elidable on demand)

**Skip it if you…**
- only use a single provider's native compaction and don't need a cross-provider proxy
- run in a sandbox where a local proxy process can't run

<details>
<summary><b>What's inside</b></summary>

- **Lossless passes** — supersession (drop superseded tool outputs), IVM/delta (re-reads → diffs),
  envelope + exact dedup, columnar JSON & log re-encoding, JSON-Schema slimming, reasoning-trace
  pruning, query-relevance pruning.
- **Opt-in lossy** — large-array row-capping, per-line field truncation, token-level telegraphic NL
  compaction, **AST code skeletonization** (tree-sitter: rust/python/js/ts/go).
- **Closed-loop controller** — per-session aggression from cache-hit-rate, output-verbosity, and
  context-fill signals; cache-prefix-boundary aware; bounded body buffering and upstream timeouts.

</details>

## Architecture

| Crate | Role |
|---|---|
| `cull-core` | the compression engine: segmenter, passes, planner, lossless + lossy transforms, skeletonizer |
| `cull-tokenize` | fast approximate token counter (chars/4) |
| `cull-cache` | provider cache models / hit-rate floors |
| `cull-proxy` | the HTTP proxy + closed-loop controller + sensors |
| `cull-cli` | the `cull` command |
| `cull-bench` | competitive benchmarks (not published) |

## Status & limitations

- **No published release** — v0.1.0 is tagged but not yet pushed to crates.io.
- **3 startup `.expect()` calls in `cull-proxy/main.rs`** — these fail-fast on bind/listen failure
  (appropriate), but the proxy has not been stress-tested against hostile input in a live environment.
- The context-fill signal counts the serialized request (incl. JSON envelope), so it slightly
  over-estimates true fill (conservative — errs toward compressing sooner).
- A `>2 MB` *streaming* response whose final usage event straddles the 64 KB tail buffer may skip one
  verbosity sample (non-fatal).
- **Never run against a live model API** — verified against mock upstreams + 154 unit/integration tests.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). CI runs `cargo fmt --check`, `cargo clippy -D warnings`,
`cargo test`, and a release build — please make sure those pass locally.

## License

MIT — see [LICENSE](LICENSE).
