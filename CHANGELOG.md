# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-07-01

### Added

- **CLI**: `doctor`, `perf`, `learn`, `dashboard`, `output-savings`, `update`, and `wrap` / `unwrap`
  (one-command agent integration — launch Claude Code, Codex, aider, goose, and others through the
  proxy); new `compact-html`, `compact-csv`, and `deref-images` transforms; a `--version` flag.
- **Python bindings** (`tare-py`, PyPI: `tare-compress`): abi3 wheel (Python 3.9+) exposing all core
  transforms, plus `tare.integrations` adapters for litellm, ASGI middleware, LangChain, Agno,
  Strands, and proxy-wired Anthropic/OpenAI SDK clients.
- **npm package** (`tare-ai`): prebuilt binaries plus JS/TS adapters (`withTare`, `tareMiddleware`,
  `startProxy`, `tareBaseUrl`).
- **MCP**: `tare_deref_images` and persistent cross-session memory tools (`tare_remember`,
  `tare_recall`, `tare_forget`, `tare_memory_stats`) backed by the new `tare-memory` crate
  (SQLite, content-hash dedup, multi-source provenance).
- **Core**: image dereferencing, opt-in semantic relevance (`neural-embed` feature, exact cosine),
  AST skeletonization for Java, C, C++, and Perl, full-range losslessness guarantee, and
  learned-profile support (`tare learn`).
- **Proxy**: admin surface (`GET /admin/stats`, `POST /admin/runtime-env` hot-sync), output A/B
  holdout (`TARE_OUTPUT_HOLDOUT`), learned-profile auto-load at startup, opt-in per-turn logging
  (`TARE_LOG`), and the `x-tare-input-tokens` response header.
- **Distribution**: `curl | sh` installer (checksummed binaries for macOS arm64/x86_64 and Linux
  x86_64) and a Docker image on GHCR.

### Fixed

- pyo3 bumped 0.22 → 0.29, clearing RUSTSEC-2025-0020 and RUSTSEC-2026-0177.
- Poison-safe embed mutex; non-panicking `json_crush`; runtime guard for plan/locs mismatch.

## [0.1.0] - 2026-06-29

Initial public release.

### Added

- **Lossless compression pipeline**: supersession, IVM/delta (re-reads → diffs), envelope + exact
  dedup, columnar JSON & log re-encoding, JSON-Schema slimming, reasoning-trace pruning, and
  query-relevance pruning.
- **Opt-in lossy levers**: large-array row-capping, per-line field truncation, token-level
  telegraphic NL compaction, and **AST code skeletonization** (tree-sitter; keep
  signatures/types/imports, drop function bodies — reversible by re-reading).
- **HTTP proxy** (`tare-proxy`) speaking Anthropic (`/v1/messages`) and OpenAI
  (`/v1/chat/completions`), with a **closed-loop controller**: per-session aggression driven by
  cache-hit-rate (halt), output-verbosity (back off), and context-fill (compress harder). Cache-
  prefix-boundary aware; bounded body buffering and upstream timeouts.
- **CLI** (`tare`): `compress`, `slim-schema`, `compact-lossy`, `skeletonize`.
- **MCP server** (`tare-mcp`): a stdio JSON-RPC server exposing `tare_compress`, `tare_skeletonize`,
  `tare_compact_lossy`, a reversible **`tare_expand`** (retrieve originals by id), and `tare_stats`.
- Competitive benchmarks under `crates/tare-bench/benchmarks/`.

[Unreleased]: https://github.com/mstuart/tare/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/mstuart/tare/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/mstuart/tare/releases/tag/v0.1.0
