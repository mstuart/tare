# tare Compression Proof

**Tokenizer:** tiktoken o200k_base
**Binary:** `target/release/tare` (release build)

| Name | Content Type | Command | Input Tokens | Output Tokens | Ratio | Reduction | ms |
|------|-------------|---------|-------------|--------------|-------|-----------|-----|
| cargo_packages | json_array | `tare compact-lossy` | 6,906 | 3,625 | 0.525 | 47.5% | 8.5 |
| ps_aux | tabular | `tare compact-lossy` | 1,545 | 802 | 0.519 | 48.1% | 3.0 |
| app_log | logs | `tare compact-lossy` | 13,217 | 6,551 | 0.496 | 50.4% | 3.4 |
| agent_context | agent_context | `tare compress` | 15,130 | 8,499 | 0.562 | 43.8% | 10.8 |
| server_rs | code | `tare skeletonize --path server.rs` | 5,930 | 1,582 | 0.267 | 73.3% | 5.2 |
| json_crush_rs | code | `tare skeletonize --path json_crush.rs` | 3,937 | 1,607 | 0.408 | 59.2% | 4.5 |
| readme_prose | prose | `tare compact-lossy` | 5,732 | 2,727 | 0.476 | 52.4% | 3.4 |

## Notes

- `compact-lossy`: input token count is the raw text (not JSON-wrapped stdin)
  so the ratio reflects real LLM context savings.
- `skeletonize`: bodies elided, signatures/types/imports kept; passthrough if nothing elidable.
- `compress`: input is concatenated block texts; output is the compressed context string.
  Superseded bash outputs are dropped by the dedup pass.

All numbers are REAL — produced by running the actual release binary on the committed corpus.
