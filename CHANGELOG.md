# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0]

Initial public release.

### Added

- **Lossless compression pipeline**: supersession, IVM/delta (re-reads → diffs), envelope + exact
  dedup, columnar JSON & log re-encoding, JSON-Schema slimming, reasoning-trace pruning, and
  query-relevance pruning.
- **Opt-in lossy levers**: large-array row-capping, per-line field truncation, token-level
  telegraphic NL compaction, and **AST code skeletonization** (tree-sitter; keep
  signatures/types/imports, drop function bodies — reversible by re-reading).
- **HTTP proxy** (`cull-proxy`) speaking Anthropic (`/v1/messages`) and OpenAI
  (`/v1/chat/completions`), with a **closed-loop controller**: per-session aggression driven by
  cache-hit-rate (halt), output-verbosity (back off), and context-fill (compress harder). Cache-
  prefix-boundary aware; bounded body buffering and upstream timeouts.
- **CLI** (`cull`): `compress`, `slim-schema`, `compact-lossy`, `skeletonize`.
- Competitive benchmarks under `crates/cull-bench/benchmarks/`.

[Unreleased]: https://github.com/mstuart/cull/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/mstuart/cull/releases/tag/v0.1.0
