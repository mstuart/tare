# Benchmarks

Measured in this repo (o200k tokens), reproducible with the commands shown.

| What | Result | Reproduce |
|---|---:|---|
| AST code skeletonization on 31 real Rust files | **65.1%** smaller (57,199 → 19,988) | `tare skeletonize --path <f>` |
| `ps aux` vs RTK at equal fidelity (same rows + columns) | **~4.5% smaller**, 5/5 trials | `ps aux \| tare compact-lossy --max-rows 30 --max-field 110` |
| Lossless JSON / log columnar re-encode | smaller, **byte-recoverable** | `tare compress` |

Code reads are ~67–76% of a coding agent's tokens
([SWE-Pruner, ACL 2026](https://arxiv.org/abs/2601.16746)), so skeletonization is the single biggest
lever.

## Competitive harnesses

Head-to-head comparisons against Headroom, LLMLingua-2, lean-ctx, and RTK live in
`crates/tare-bench/benchmarks/` (Python scripts that require the competitor tools installed). Run the
scripts there to reproduce; at **equal fidelity**, tare matches or beats each — and is the only one
with a lossless mode and cross-turn dedup.

!!! note
    Numbers above are measured against local inputs. tare has been smoke-tested end-to-end against
    the live Anthropic API — through the proxy on a Claude subscription (`scripts/live-smoke-sub.sh`)
    and via the MCP server over real stdio — but is not yet production-hardened or load-tested.
