# cull

**Lossless-by-default context compression for LLM coding agents.**

cull sits between an agent and the model API and shrinks the context window. Unlike most compressors
it is:

- **Lossless by default** — re-encodes tool output, logs, and JSON into a denser *equivalent* form;
  it only drops information when you explicitly opt in.
- **Cache-correct** — detects the provider's cache breakpoint and only compresses the dynamic suffix,
  so your 90%-discount prefix cache keeps hitting.
- **Closed-loop** — watches the model's *output* verbosity, context fill, and cache hit-rate, and
  dials compression up or down per session.

Use it three ways: as an HTTP **proxy** (zero code changes), as a Rust **library**, or as a **CLI**.

→ [Getting started](getting-started.md) · [Architecture](architecture.md) · [Benchmarks](benchmarks.md)

!!! note "Status"
    Pre-1.0. Well-tested (154 tests) and benchmarked, but not yet exercised against a live model API.
    See the README's Status section before deploying.
