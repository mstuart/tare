# CLI reference

The `tare` binary applies one-shot transforms to stdin and writes to stdout. Fidelity/diagnostic
output goes to stderr.

## `tare compress`

Run the full **lossless** pipeline over a JSON context (Anthropic/OpenAI message array on stdin).

```bash
cat ctx.json | tare compress --task "fix the auth bug" [--report] [--budget N]
```

- `--task` — the current query; drives query-conditioned relevance pruning.
- `--report` — emit per-segment drop reasons to stderr.
- `--budget` — optional hard token budget; evict lowest-priority context to fit.

## `tare slim-schema`

Strip pure JSON-Schema metadata (`$schema`, `title`, `$id`, `examples`, …) from tool/function
definitions, preserving names, types, `required`, and descriptions. Opt-in lossy.

```bash
cat tools.json | tare slim-schema
```

## `tare compact-lossy`

Aggressively compact a large JSON array / tabular or log output. Keeps head+tail rows, anomalies,
alert lines, and query-relevant rows; drops the uniform bulk. Opt-in lossy.

```bash
ps aux | tare compact-lossy --boundary 3 --max-rows 30 --max-field 110 --task "high cpu"
```

- `--boundary` — head/tail rows always kept (default 3).
- `--max-rows` — cap kept lines (boundary/alert/relevant always kept).
- `--max-field` — truncate each kept line to N chars.
- `--task` — keep units relevant to the query.

## `tare skeletonize`

Drop function/method bodies from a source file, keeping signatures, types, imports, and doc comments.
Reversible by re-reading. Supports rust/python/js/ts/go (by extension).

```bash
cat big.rs | tare skeletonize --path big.rs
```

## `tare doctor`

Health check: engine self-test (json_crush round-trip, code skeleton, tokenizer sanity), resolved
config report, best-effort proxy probe (TCP connect), and learned-profile status. Exits non-zero if
any check (`✗`) fails; warnings (`⚠`) are advisory.

No flags.

```bash
tare doctor
```

## `tare perf`

Measure compression savings and wall-clock speed. Prints a table of original tokens, lossless tokens,
lossless ratio, lossy tokens, and time per source. Omit `--input` (or pass `--sample`) to run on the
built-in representative corpus.

- `--input PATH` — file or directory to benchmark; files classified by extension.
- `--sample` — use the built-in sample corpus (same as omitting `--input`).

```bash
tare perf --sample
tare perf --input ./src
```

## `tare learn`

Offline corpus analysis: reads every file under `DIR`, classifies each by extension
(rs/py/js/ts/tsx/go → code; json → JSON; log → log; everything else → prose), measures lossless and
lossy compression ratios, derives compression settings, and writes the result as
`~/.config/tare/profile.json` (override with `$TARE_PROFILE`; `$XDG_CONFIG_HOME` is respected).
The proxy reads this profile automatically on startup. This is static analysis of a local corpus, not
online learning.

- `--from DIR` — directory to read source/data files from (required).

```bash
tare learn --from ./logs
# Learned profile from: ./logs
#   files processed    : 42
#   measured ratio     : 1.847x (lossless baseline)
#   written to         : /Users/you/.config/tare/profile.json
```
