# Benchmarks

Measured in this repo (o200k tokens), reproducible with the commands shown.

| What | Result | Reproduce |
|---|---:|---|
| AST code skeletonization on 31 real Rust files | **65.1%** smaller (57,199 → 19,988) | `cull skeletonize --path <f>` |
| `ps aux` vs RTK at equal fidelity (same rows + columns) | **~4.5% smaller**, 5/5 trials | `ps aux \| cull compact-lossy --max-rows 30 --max-field 110` |
| Lossless JSON / log columnar re-encode | smaller, **byte-recoverable** | `cull compress` |

Code reads are ~67–76% of a coding agent's tokens
([SWE-Pruner, ACL 2026](https://arxiv.org/abs/2601.16746)), so skeletonization is the single biggest
lever.

## Competitive harnesses

Head-to-head comparisons against Headroom, LLMLingua-2, lean-ctx, and RTK live in
`crates/cull-bench/benchmarks/` (Python scripts that require the competitor tools installed). At
**equal fidelity**, cull matches or beats each on every content type, and is the only one with a
lossless mode and cross-turn dedup.

!!! note
    Numbers above are measured against mock upstreams and local inputs. cull has not yet been
    exercised against a live model API end-to-end; a live smoke test requires an API key and is the
    one remaining validation gap before a 1.0.
