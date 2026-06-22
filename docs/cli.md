# CLI reference

The `cull` binary applies one-shot transforms to stdin and writes to stdout. Fidelity/diagnostic
output goes to stderr.

## `cull compress`

Run the full **lossless** pipeline over a JSON context (Anthropic/OpenAI message array on stdin).

```bash
cat ctx.json | cull compress --task "fix the auth bug" [--report] [--budget N]
```

- `--task` — the current query; drives query-conditioned relevance pruning.
- `--report` — emit per-segment drop reasons to stderr.
- `--budget` — optional hard token budget; evict lowest-priority context to fit.

## `cull slim-schema`

Strip pure JSON-Schema metadata (`$schema`, `title`, `$id`, `examples`, …) from tool/function
definitions, preserving names, types, `required`, and descriptions. Opt-in lossy.

```bash
cat tools.json | cull slim-schema
```

## `cull compact-lossy`

Aggressively compact a large JSON array / tabular or log output. Keeps head+tail rows, anomalies,
alert lines, and query-relevant rows; drops the uniform bulk. Opt-in lossy.

```bash
ps aux | cull compact-lossy --boundary 3 --max-rows 30 --max-field 110 --task "high cpu"
```

- `--boundary` — head/tail rows always kept (default 3).
- `--max-rows` — cap kept lines (boundary/alert/relevant always kept).
- `--max-field` — truncate each kept line to N chars.
- `--task` — keep units relevant to the query.

## `cull skeletonize`

Drop function/method bodies from a source file, keeping signatures, types, imports, and doc comments.
Reversible by re-reading. Supports rust/python/js/ts/go (by extension).

```bash
cat big.rs | cull skeletonize --path big.rs
```
