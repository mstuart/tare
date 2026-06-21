# cull

**Query-aware, cache-correct, lossless-by-default context compression for LLM coding agents.**

Cull sits between an agent and the model API and shrinks the context window. Unlike most
compressors it is **lossless by default**, **cache-correct** (it never rewrites the provider's cached
prefix), and **closed-loop** — it watches the model's *output* and adapts. When you opt in, it also
compresses aggressively (row-capping, field-truncation, telegraphic NL, and AST code skeletonization).

> **Status: pre-1.0, not yet production-hardened.** The engine is well-tested (154 tests) and beats
> incumbents on the included benchmarks, but it has not been run against a live model API and has
> rough edges listed under [Status & limitations](#status--limitations). Read that section before
> deploying.

---

## Why

Most context compressors optimize one number — *input tokens removed* — one-way and blind. Cull's
design rests on three observations the research bears out:

- **Lossless wins where it can.** Tool output, logs, and JSON re-encode losslessly into a far denser
  form; only drop information when the caller explicitly accepts it.
- **The cache is the economy.** Provider prefix caches discount cached tokens ~10×. A compressor that
  perturbs the cached prefix turns a 90% discount into 0%. Cull only ever rewrites the *dynamic
  suffix* after the cache breakpoint.
- **Compression has a feedback loop.** Over-compressing makes models *compensate with verbose
  output*, so total cost can rise even as input falls. Cull is the only proxy that watches output
  verbosity and backs off.

## Install

```bash
git clone <your-fork>/cull && cd cull
cargo build --release            # builds `cull` (CLI) and `cull-proxy`
```

Binaries land in `target/release/{cull,cull-proxy}`.

## Quick start

### As a proxy (transparent, recommended)

Point your agent's API base URL at Cull; it forwards to the real upstream and compresses on the way.

```bash
CULL_UPSTREAM=https://api.anthropic.com CULL_PORT=8787 ./target/release/cull-proxy
# then set your agent's base URL to http://localhost:8787
```

It speaks Anthropic (`/v1/messages`) and OpenAI (`/v1/chat/completions`). Response headers report what
it did: `x-cull-input-tokens`, `x-cull-net-tokens`, `x-cull-dropped`, `x-cull-aggression`,
`x-cull-verbosity-spike`, `x-cull-halted`.

| Env var | Default | Meaning |
|---|---|---|
| `CULL_UPSTREAM` | `https://api.anthropic.com` | upstream API base URL |
| `CULL_PORT` | `8787` | listen port |
| `CULL_RECENCY` | `4` | tool outputs always kept regardless of relevance |
| `CULL_ENABLED` | `true` | set `0`/`false` for byte-exact passthrough |
| `CULL_CONTEXT_LIMIT` | `200000` | model context window (drives the fill-based aggression dial) |

### As a CLI (one-shot transforms, read stdin)

```bash
cat ctx.json   | cull compress --task "fix the auth bug"   # lossless pipeline + fidelity report
cat tools.json | cull slim-schema                          # strip JSON-Schema metadata (lossy)
ps aux         | cull compact-lossy --max-rows 30 --max-field 110   # tabular row-cap + truncate (lossy)
cat big.rs     | cull skeletonize --path big.rs            # drop fn bodies, keep structure (lossy)
```

## How it works

**Lossless pipeline** (default `compress` / proxy): supersession (drop superseded tool outputs),
IVM/delta (re-reads become diffs), envelope + exact dedup, columnar JSON & log re-encoding,
JSON-Schema slimming, reasoning-trace pruning, and query-relevance pruning (symbol-overlap,
recency-protected). Every transform is verified reversible.

**Opt-in lossy levers**: large-array row-capping, per-line field truncation, token-level telegraphic
NL compaction, and **AST code skeletonization** (tree-sitter; keep signatures/types/imports/docs,
drop function bodies, reversible by re-reading).

**Closed-loop controller** (proxy): a per-session aggression dial driven by live signals —
cache-hit-rate (halt → passthrough when compression is busting the cache), output-verbosity (back off
when the model over-generates), and context-fill (compress harder as the window saturates). Levels:
*default lossless → tighten recency + skeletonize code → engage lossy*.

## Benchmarks

Measured in this repo:

- **AST skeletonization: 65.1% token reduction** across 31 real Rust source files (57,199 → 19,988
  o200k tokens), reproducible via `cull skeletonize`. Code reads are ~67–76% of a coding agent's
  tokens, so this is the largest single lever.
- **`ps aux` vs RTK at equal fidelity: ~4.5% smaller** (same rows + columns), 5/5 trials.

Competitive harnesses live in `crates/cull-bench/benchmarks/` (`headroom_vs_cull.py`, `three_way.py`,
`qa_accuracy_4way.py`, `real_trace_corpus.py`, `answer_equivalence.py`). They require the competitor
tools installed. On those benchmarks, **at equal fidelity** Cull matches or beats Headroom
(SmartCrusher), RTK, LLMLingua-2, and lean-ctx on every content type — and is the only one with a
lossless mode and cross-turn dedup. The harnesses in `crates/cull-bench/benchmarks/` are the
reproducible source of those claims.

## Architecture

| Crate | Role |
|---|---|
| `cull-core` | the compression engine: segmenter, passes, planner, lossless + lossy transforms, skeletonizer |
| `cull-tokenize` | fast approximate token counter (chars/4) |
| `cull-cache` | provider cache models / hit-rate floors |
| `cull-proxy` | the HTTP proxy + closed-loop controller + sensors |
| `cull-cli` | the `cull` command |
| `cull-bench` | competitive benchmarks |

## Status & limitations

Honest list of what stands between this and production:

- **Never run against a live model API** — verified only against mock upstreams in tests.
- **No CI** and **no published release** (`version = 0.0.0`, placeholder repo URL).
- **78 `unwrap()`/`expect()`/`panic!` in non-test code** — the network-facing panic surface is
  unaudited; a proxy should degrade, not crash, on hostile input.
- The context-fill signal counts the serialized request (incl. JSON envelope), so it slightly
  over-estimates true fill (conservative — errs toward compressing sooner).
- A `>2 MB` *streaming* response whose final usage event straddles the 64 KB tail buffer may skip one
  verbosity sample (non-fatal).
- User docs beyond this README are thin; the `docs/` tree is design specs + implementation plans.

## License

MIT — see [LICENSE](LICENSE).
