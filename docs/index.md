# tare

**Lossless-by-default context compression for LLM coding agents.**

tare sits between an agent and the model API and shrinks the context window. Unlike most compressors
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
    v0.2.0 released — binary installer, npm, and Docker/GHCR are live. 228 tests, benchmarked, and
    verified end-to-end against the live Anthropic API. See the README's Status & limitations.
