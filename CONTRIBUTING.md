# Contributing to tare

Thanks for your interest! tare is a Rust workspace; contributions of bug fixes, tests, benchmarks,
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

- `tare-core` — the compression engine (segmenter, passes, planner, lossy transforms, skeletonizer)
- `tare-tokenize` — fast approximate token counter
- `tare-cache` — provider cache models
- `tare-proxy` — the HTTP proxy + closed-loop controller
- `tare-cli` — the `tare` binary
- `tare-bench` — competitive benchmarks (not published)

## Notes

- **`neural-embed` feature:** `tare-core` has an optional `neural-embed` feature that pulls in an
  ONNX runtime (`fastembed`) and downloads model files on first use. It is off by default; you do not
  need it for normal development.
- **Benchmarks:** the harnesses in `crates/tare-bench/benchmarks/` are Python scripts that compare
  against external tools (Headroom, LLMLingua-2, lean-ctx, RTK). They require those tools installed
  and the relevant env vars (e.g. `TARE_LLMLINGUA_PY`, `TARE_HEADROOM_PY`) pointed at a venv.

## Pull requests

- Keep changes focused; add or update tests for behavior changes.
- Default branch is `main`. Branch from it, open a PR, and ensure CI is green.
- For new skeletonizer languages, add the grammar to `tare-core` and a round-trip test in
  `code_skeleton.rs`.
