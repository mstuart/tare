<div align="center">
  <img src="docs/assets/logo.svg" alt="tare — lossless context compression for LLM coding agents" width="720">
</div>

<p align="center"><strong>Cut your coding agent's context 44–73% — lossless by default, cache-safe, output-aware.</strong></p>

<p align="center">
  <a href="https://github.com/mstuart/tare/actions/workflows/ci.yml"><img src="https://github.com/mstuart/tare/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://www.npmjs.com/package/tare-ai"><img src="https://img.shields.io/npm/v/tare-ai?label=npm" alt="npm"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <img src="https://img.shields.io/badge/rust-1.82%2B-orange.svg" alt="Rust 1.82+">
</p>

<p align="center">
  <a href="#get-started-60-seconds">Install</a> ·
  <a href="#proof">Proof</a> ·
  <a href="#compared-to">Compared to</a> ·
  <a href="#use-with-your-claude-subscription--mcp-no-api-key">Claude / MCP</a> ·
  <a href="#integrations">Integrations</a> ·
  <a href="#status--limitations">Status</a>
</p>

---

tare sits between your agent and the model API and shrinks the context window. Three properties make
it different:

- **Lossless by default** — tool output, logs, and JSON are re-encoded into a denser *equivalent*
  form. Information is dropped only when you opt in (row-caps, telegraphic NL, AST code
  skeletonization).
- **Cache-correct** — provider prefix caches discount cached tokens ~10×, and one rewritten byte in
  the prefix forfeits it. tare detects the cache breakpoint and compresses only the dynamic suffix.
- **Output-aware** — over-compression makes models compensate with verbose output, so total cost can
  rise as input falls. tare watches output tokens per turn and backs off when they spike.

## What you get

- **`tare-proxy`** — point your agent's base URL at it; zero code changes. Speaks Anthropic
  (`/v1/messages`) and OpenAI (`/v1/chat/completions`).
- **`tare` CLI** — `compress`, `skeletonize`, `compact-lossy`, and 12 more subcommands
  ([full reference](docs/cli.md)).
- **`tare-mcp`** — compression + persistent cross-session memory as MCP tools, for any MCP client.
- **Libraries** — `tare-core` (Rust), [`tare-compress`](#integrations) (Python),
  [`tare-ai`](#integrations) (JS/TS).

## Proof

<p align="center"><img src="docs/assets/savings.svg" alt="tare token reduction by content type" width="680"></p>

All numbers are measured by running `target/release/tare` on the committed corpus with tiktoken
o200k_base (v0.13.0). Results live in `crates/tare-bench/results/`; reproduce with
`python3 crates/tare-bench/run_proof.py`.

| Name | Content type | Command | Input tokens | Output tokens | Reduction |
|---|---|---|---:|---:|---:|
| cargo\_packages | json\_array | `tare compact-lossy` | 6,906 | 3,625 | **47.5%** |
| ps\_aux | tabular | `tare compact-lossy` | 1,545 | 802 | **48.1%** |
| app\_log | logs | `tare compact-lossy` | 13,217 | 6,551 | **50.4%** |
| agent\_context | agent\_context | `tare compress` | 15,130 | 8,499 | **43.8%** |
| server\_rs | code | `tare skeletonize --path server.rs` | 5,930 | 1,582 | **73.3%** |
| json\_crush\_rs | code | `tare skeletonize --path json_crush.rs` | 3,937 | 1,607 | **59.2%** |
| readme\_prose | prose | `tare compact-lossy` | 5,732 | 2,727 | **52.4%** |

Code reads are ~67–76% of a coding agent's tokens ([SWE-Pruner, ACL 2026](https://arxiv.org/abs/2601.16746)),
so skeletonization is the biggest lever. Head-to-head harnesses against Headroom, LLMLingua-2,
lean-ctx, and RTK live in [`crates/tare-bench/benchmarks/`](docs/benchmarks.md).

## Compared to

|  | Scope | Deploy | Local | Lossless default | Output-aware¹ |
|---|---|---|:-:|:-:|:-:|
| **tare** | tools · logs · files · JSON · history | proxy · library · CLI · MCP | ✅ | ✅ | ✅ |
| [Headroom](https://github.com/chopratejas/headroom) | all context | proxy · lib · MCP | ✅ | ❌ (reversible via cache) | ❌ |
| [RTK](https://github.com/rtk-ai/rtk) | CLI command outputs | CLI wrapper | ✅ | ❌ | ❌ |
| [lean-ctx](https://github.com/yvgude/lean-ctx) | CLI commands, MCP tools | CLI · MCP | ✅ | ❌ | ❌ |
| LLMLingua-2 | prose / RAG | library (ML model) | ✅ | ❌ | ❌ |
| OpenAI / Anthropic native compaction | conversation history | provider-native | ❌ | ❌ | ❌ |

¹ *Output-aware* = reads the model's output token count each turn and reduces compression aggression
when verbosity spikes (the `x-tare-verbosity-spike` signal in
[`crates/tare-proxy/src/server.rs`](crates/tare-proxy/src/server.rs)).

## When to use · When to skip

**Great fit if you…**
- run coding agents and want savings without losing information by default
- care about the provider cache staying warm (tare won't break the prefix)
- want code reads compressed structurally (signatures kept, bodies elidable on demand)

**Skip it if you…**
- only use a single provider's native compaction and don't need a cross-provider proxy
- run in a sandbox where a local proxy process can't run

## Get started (60 seconds)

```bash
# 1 — install (no Rust toolchain needed)
curl -fsSL https://raw.githubusercontent.com/mstuart/tare/main/install.sh | sh   # → ~/.local/bin
# or:  npm install -g tare-ai
# or:  docker pull ghcr.io/mstuart/tare
# or:  cargo install tare-cli                    # crates.io publish pending
# or:  git clone https://github.com/mstuart/tare && cd tare && cargo build --release

# 2 — run as a proxy (point your agent's base URL at http://localhost:8787)
TARE_UPSTREAM=https://api.anthropic.com tare-proxy

# 3 — or use the CLI on any stdin
cat big.rs | tare skeletonize --path big.rs    # drop fn bodies, keep structure
ps aux     | tare compact-lossy --max-rows 30 --max-field 110
```

| Env var | Default | Meaning |
|---|---|---|
| `TARE_UPSTREAM` | `https://api.anthropic.com` | upstream API base URL |
| `TARE_PORT` | `8787` | listen port |
| `TARE_RECENCY` | `4` | recent tool outputs always kept |
| `TARE_ENABLED` | `true` | set `0`/`false` for byte-exact passthrough |
| `TARE_CONTEXT_LIMIT` | `200000` | model context window (drives the fill dial) |
| `TARE_OUTPUT_HOLDOUT` | `0` | fraction of sessions left uncompressed (A/B for `tare output-savings`) |
| `TARE_LOG` | unset | set it to log one line per turn with the compression report |

Response headers (`x-tare-input-tokens`, `x-tare-net-tokens`, `x-tare-dropped`, `x-tare-aggression`,
`x-tare-verbosity-spike`, `x-tare-halted`) report what each turn did; `GET /admin/stats` and
`POST /admin/runtime-env` expose live stats and hot config. Details: [getting started](docs/getting-started.md).

## Integrations

```python
# pip install tare-compress   (PyPI publish pending)
import tare
out = tare.compress(blocks_json, task="fix the login bug")   # in-process, no proxy needed
```

```js
// npm install tare-ai
import { withTare } from "tare-ai";
const client = new Anthropic(withTare({ apiKey: "..." }));   // routes through the local proxy
```

Python ships all core transforms in-process plus adapters for litellm, ASGI, LangChain, Agno, and
Strands; JS/TS ships proxy helpers for the Anthropic/OpenAI SDKs and the Vercel AI SDK.
**Full adapter reference: [docs/integrations.md](docs/integrations.md).**

## Use with your Claude subscription — MCP, no API key

The proxy forwards whatever auth the client sends — a billable `x-api-key`, or your Claude Pro/Max
**subscription OAuth token** when you point Claude Code's `ANTHROPIC_BASE_URL` at it
(`scripts/live-smoke-sub.sh` runs exactly this round-trip). Prefer no base-URL change at all? Run
`tare-mcp`: a local stdio server your agent calls as tools — it never calls the model itself, so it
needs no API key.

```bash
# easiest — no build; npx fetches the prebuilt binary on first run:
claude mcp add tare -s user -- npx -y -p tare-ai tare-mcp
```

```json
{
  "mcpServers": {
    "tare": { "command": "npx", "args": ["-y", "-p", "tare-ai", "tare-mcp"] }
  }
}
```

Paste the same block into any MCP client (Cursor, Codex, Claude Desktop's
`claude_desktop_config.json`, …). It exposes 10 tools: compression (`tare_compress`,
`tare_skeletonize`, `tare_compact_lossy`, `tare_deref_images`), a reversible `tare_expand`,
`tare_stats`, and cross-session memory — [full list](docs/getting-started.md).

## Wrap your agent

One command starts the proxy and launches your CLI agent through it — ENV-based and ephemeral:

```bash
tare wrap claude               # start proxy + launch Claude Code through it
tare wrap codex --port 9000    # 12 agents supported: claude, codex, aider, goose, …
tare wrap claude --print       # dry-run: show what would run
```

Full agent matrix and modes: [docs/cli.md](docs/cli.md).

## Status & limitations

- **Tested** — verified end-to-end against the live Anthropic API (a full proxy round-trip on a
  Claude subscription, plus the MCP server over real stdio JSON-RPC), on top of 228 unit,
  integration, and property tests; `fmt` / `clippy -D warnings` / `cargo deny` gate every commit.
- **Deploy it as a local sidecar** — tare runs next to your agent and forwards your credentials
  upstream without logging or persisting them; treat it as a trusted component on your own machine,
  not shared multi-tenant infrastructure ([SECURITY.md](SECURITY.md)). Startup failures exit with a
  clear `[tare-proxy] fatal: …` message, never a panic backtrace.

**Known edges**

- Proxy and CLI token counts are approximate (`tare-tokenize`, chars/4); the benchmark numbers above
  are measured with tiktoken.
- The context-fill signal counts the serialized request (incl. JSON envelope), so it slightly
  over-estimates fill — conservative, errs toward compressing sooner.
- A `>2 MB` *streaming* response whose final usage event straddles the 64 KB tail buffer may skip
  one verbosity sample (non-fatal).

**Non-goals** — no trained ML text-compressor (no weights to download, no inference latency) and no
audio: transcribe externally and feed the transcript through `tare compress`.

## Architecture

Nine crates: engine (`tare-core`), proxy + closed-loop controller (`tare-proxy`), CLI, MCP server,
SQLite memory, tokenizer, cache models, Python bindings, bench harness. Optional `neural-embed`
feature swaps keyword relevance for exact-cosine neural embeddings.
**Diagram and crate guide: [docs/architecture.md](docs/architecture.md).**

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). CI runs `cargo fmt --check`, `cargo clippy -D warnings`,
`cargo test`, and a release build — please make sure those pass locally.

## License

MIT — see [LICENSE](LICENSE).
