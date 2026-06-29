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
  │                    query-relevance                        │
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
| `tare-mcp` | MCP stdio server: compression tools + a reversible `tare_expand` |
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
