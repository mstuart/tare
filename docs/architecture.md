# Architecture

```
 Your agent / app
      │  prompts · tool outputs · logs · file reads · RAG results
      ▼
  ┌──────────────────────────────────────────────────────────┐
  │  tare   (runs locally — your data and API key stay here)  │
  │  cache-boundary detect → only touch the dynamic suffix    │
  │  lossless passes:  supersession · IVM/delta · dedup ·     │
  │                    columnar JSON/log · schema-slim ·      │
  │                    query-relevance (keyword)              │
  │  opt-in lossy:     row-cap · field-truncate · telegraphic │
  │                    · AST code skeletonization             │
  │  closed-loop controller:  cache-hit-rate (halt) ·         │
  │                    output-verbosity (back off) ·          │
  │                    context-fill (compress harder)         │
  └──────────────────────────────────────────────────────────┘
      ▼
 LLM provider
```

## Crates

| Crate | Role |
|---|---|
| `tare-core` | the compression engine: segmenter, passes, planner, lossless + lossy transforms, skeletonizer |
| `tare-tokenize` | fast approximate token counter (chars/4) |
| `tare-cache` | provider cache models / hit-rate floors |
| `tare-proxy` | the HTTP proxy + closed-loop controller + sensors |
| `tare-cli` | the `tare` command |
| `tare-memory` | persistent cross-session memory: SQLite-backed remember/recall with content-hash dedup and multi-source provenance |
| `tare-mcp` | MCP stdio server: compression tools + a reversible `tare_expand` + memory tools |
| `tare-bench` | competitive benchmarks (not published) |

## The closed-loop controller

Most compressors optimize *input tokens removed*, one-way and blind. tare's proxy runs a per-session
controller that reads three live signals and dials aggression:

- **cache-hit-rate** — if compression is busting the provider's prefix cache, **halt** (passthrough).
- **output-verbosity** — over-compressing makes models *compensate with verbose output* (the
  "compression paradox"); when output spikes, **back off**.
- **context-fill** — as the window saturates, **compress harder** (skeletonize code, then engage the
  lossy tier).

It is cache-prefix-boundary aware: it never rewrites tokens before the provider's cache breakpoint, so
your cache discount survives.

## Lossless vs lossy

The default `compress` pipeline is **lossless** — every transform is reversible (columnar re-encode,
dedup, cross-turn delta). Lossy levers (row-cap, field-truncate, telegraphic NL, AST skeletonization)
are **opt-in**; the skeletonizer is reversible by re-reading the file.

## Cross-agent memory

`tare-memory` is a SQLite-backed store consumed by `tare-mcp`.  Four MCP tools expose it:

| Tool | Required args | Optional args | Returns |
|---|---|---|---|
| `tare_remember` | `content` | `source` (default `"mcp"`) | `id` of stored (or deduped existing) memory |
| `tare_recall` | `query` | `limit` (default 5) | matches ordered by term-hit score descending |
| `tare_forget` | `id` (integer) | — | whether the row existed |
| `tare_memory_stats` | — | — | total memory count + distinct source count |

Dedup is content-hash based (xxh3-64): storing identical content from two different agents returns the same
`id` and records both sources in the provenance table.  The store is **shared across all agents** that point
at the same `tare-mcp` process, or independently that set `$TARE_MEMORY` to the same db path (default
`~/.config/tare/memory.db`).

## Relevance modes

The **query-relevance** lossless pass keeps tool outputs and file reads that match the current task
query and drops lower-priority content. Two modes are available:

- **Keyword/symbol matching** (default) — no model download, no external dependency. This is what
  `cargo build --release` produces.
- **Neural semantic relevance** (opt-in) — exact cosine ranking over neural embeddings via
  [fastembed](https://github.com/Anyscale/fastembed-rs); enabled by building with the
  `neural-embed` cargo feature (`cargo build --release --features neural-embed`). Downloads an
  embedding model on first use. No HNSW or approximate index is used — exact cosine over the
  candidate segment set is fast enough at relevance-pass scale and is strictly more accurate.
