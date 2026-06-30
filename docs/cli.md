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

## `tare dashboard`

Live savings panel that polls the proxy's `GET /admin/stats` and redraws every `--interval-ms`.

- `--port N` — proxy port (defaults to `$TARE_PORT` or `8787`).
- `--once` — print a single snapshot and exit (for scripting/CI).
- `--interval-ms N` — refresh interval in milliseconds (default `1000`).

```bash
tare dashboard --once
```

## `tare output-savings`

Estimates OUTPUT-token reduction by comparing the proxy's shaped vs. holdout A/B arms, with a 95%
confidence interval. Requires the proxy to run with `TARE_OUTPUT_HOLDOUT > 0` (the fraction of sessions
that bypass compression as a control arm).

- `--port N` — proxy port (defaults to `$TARE_PORT` or `8787`).

```bash
TARE_OUTPUT_HOLDOUT=0.1 tare-proxy &      # 10% of sessions form the control arm
tare output-savings
# Output reduction: 31.7% (95% CI 27.7%..35.7%) [n_shaped=900, n_holdout=100]
```

## `tare update`

Compares the running version against the latest GitHub release. With `--check` it only reports; without
it, it detects the install method from the binary path (npm vs. the `curl` installer) and re-runs it.

- `--check` — only report the latest version; make no changes.

```bash
tare update --check
# current: v0.1.0
# latest : v0.1.0
# → already up to date.
```

## `tare wrap`

Start `tare-proxy` and launch a coding agent through it in one step. For auto-launch agents the proxy
starts in the background, the agent binary is exec'd with `ANTHROPIC_BASE_URL`, `OPENAI_BASE_URL`, and
`OPENAI_API_BASE` set to point at the proxy, and the proxy is killed when the agent exits. For
manual-setup agents (GUI / VS Code extensions) the command prints step-by-step instructions for
pointing that tool's base-URL setting at the proxy — no binary is launched.

Wrapping is ENV-based and ephemeral — no persistent global state is written.

```bash
tare wrap <agent> [--port N] [--print] [-- <agent-args>…]
```

- `<agent>` — one of: `claude`, `codex`, `aider`, `goose`, `openhands`, `opencode`, `openclaw`,
  `vibe` (auto-launch); `cursor`, `cline`, `continue`, `cortex` (manual setup).
- `--port N` — proxy port (defaults to `$TARE_PORT` or `8787`).
- `--print` — dry-run: print what would run and exit without starting anything.
- `-- <args>` — extra arguments forwarded verbatim to the agent binary (auto-launch only).

**Agent matrix**

| Agent | Mode |
|---|---|
| `claude` | auto-launch |
| `codex` | auto-launch |
| `aider` | auto-launch |
| `goose` | auto-launch |
| `openhands` | auto-launch |
| `opencode` | auto-launch |
| `openclaw` | auto-launch |
| `vibe` | auto-launch |
| `cursor` | manual setup |
| `cline` | manual setup |
| `continue` | manual setup |
| `cortex` | manual setup |

```bash
tare wrap claude                          # start proxy + launch Claude Code
tare wrap claude --print                  # dry-run: show what would happen
tare wrap claude --port 9000              # custom proxy port
tare wrap aider -- --model gpt-4o        # pass extra flags to the agent
tare wrap cursor                          # print Cursor base-URL setup instructions
```

## `tare unwrap`

Print a reminder that wrapping is ENV-based and ephemeral. If you configured a base-URL override
directly in an agent's settings, `tare unwrap` tells you where to remove it.

```bash
tare unwrap <agent>
```

- `<agent>` — same set as `tare wrap`.

```bash
tare unwrap claude
# Wrapping is ENV-based and ephemeral: `tare wrap` sets ANTHROPIC_BASE_URL,
# OPENAI_BASE_URL, and OPENAI_API_BASE only for the duration of that invocation —
# there is no persistent global state to remove.
# If you configured a base-URL override directly in claude's settings, remove it there.
```
