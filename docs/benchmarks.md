# Benchmarks

All numbers are real — measured by running `target/release/tare` on the committed corpus.
**Tokenizer:** tiktoken o200k_base (v0.13.0). Results committed to `crates/tare-bench/results/`;
reproduce with `python3 crates/tare-bench/run_proof.py`.

| Name | Content type | Command | Input tokens | Output tokens | Reduction |
|---|---|---|---:|---:|---:|
| cargo\_packages | json\_array | `tare compact-lossy` | 6,906 | 3,625 | **47.5%** |
| ps\_aux | tabular | `tare compact-lossy` | 1,545 | 802 | **48.1%** |
| app\_log | logs | `tare compact-lossy` | 13,217 | 6,551 | **50.4%** |
| agent\_context | agent\_context | `tare compress` | 15,130 | 8,499 | **43.8%** |
| server\_rs | code | `tare skeletonize --path server.rs` | 5,930 | 1,582 | **73.3%** |
| json\_crush\_rs | code | `tare skeletonize --path json_crush.rs` | 3,937 | 1,607 | **59.2%** |
| readme\_prose | prose | `tare compact-lossy` | 5,732 | 2,727 | **52.4%** |

Code reads are ~67–76% of a coding agent's tokens
([SWE-Pruner, ACL 2026](https://arxiv.org/abs/2601.16746)), so skeletonization is the single biggest
lever.

## Competitive harnesses

The numbers above are tare-only (no competitor tools required). Head-to-head comparisons against
Headroom, LLMLingua-2, lean-ctx, and RTK live in `crates/tare-bench/benchmarks/` (Python scripts
that require the competitor tools installed). Run the scripts there to reproduce; at **equal
fidelity**, tare matches or beats each — and is the only one with a lossless mode and cross-turn dedup.

!!! note
    tare has been smoke-tested end-to-end against the live Anthropic API — through the proxy on a
    Claude subscription (`scripts/live-smoke-sub.sh`) and via the MCP server over real stdio — but is
    not yet production-hardened or load-tested.
