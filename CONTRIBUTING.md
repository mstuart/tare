# Contributing to cull

Thanks for your interest! cull is a Rust workspace; contributions of bug fixes, tests, benchmarks,
and language support for the skeletonizer are all welcome.

## Development

```bash
cargo build --workspace          # build everything
cargo test --workspace           # run the full test suite
cargo fmt --all                  # format (CI enforces `--check`)
cargo clippy --workspace --all-targets -- -D warnings   # lint (CI enforces)
```

CI runs exactly those four checks — please make sure they pass locally before opening a PR.

## Layout

- `cull-core` — the compression engine (segmenter, passes, planner, lossy transforms, skeletonizer)
- `cull-tokenize` — fast approximate token counter
- `cull-cache` — provider cache models
- `cull-proxy` — the HTTP proxy + closed-loop controller
- `cull-cli` — the `cull` binary
- `cull-bench` — competitive benchmarks (not published)

## Notes

- **`neural-embed` feature:** `cull-core` has an optional `neural-embed` feature that pulls in an
  ONNX runtime (`fastembed`) and downloads model files on first use. It is off by default; you do not
  need it for normal development.
- **Benchmarks:** the harnesses in `crates/cull-bench/benchmarks/` are Python scripts that compare
  against external tools (Headroom, LLMLingua-2, lean-ctx, RTK). They require those tools installed
  and the relevant env vars (e.g. `CULL_LLMLINGUA_PY`, `CULL_HEADROOM_PY`) pointed at a venv.

## Pull requests

- Keep changes focused; add or update tests for behavior changes.
- Default branch is `main`. Branch from it, open a PR, and ensure CI is green.
- For new skeletonizer languages, add the grammar to `cull-core` and a round-trip test in
  `code_skeleton.rs`.
