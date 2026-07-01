<div align="center">
  <img src="docs/assets/logo.svg" alt="tare — lossless context compression for LLM coding agents" width="720">
</div>

<p align="center"><strong>lossless by default · keyword-relevance (neural opt-in) · cache-correct · closed-loop · proxy · library · CLI · MCP · local</strong></p>

<p align="center">
  <a href="https://github.com/mstuart/tare/actions/workflows/ci.yml"><img src="https://github.com/mstuart/tare/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://www.npmjs.com/package/tare-ai"><img src="https://img.shields.io/npm/v/tare-ai?label=npm" alt="npm"></a>
  <img src="https://img.shields.io/badge/rust-1.82%2B-orange.svg" alt="Rust 1.82+">
</p>

<p align="center">
  <a href="#get-started-60-seconds">Install</a> ·
  <a href="#use-with-your-claude-subscription--mcp-no-api-key">Subscription</a> ·
  <a href="#wrap-your-agent">Wrap</a> ·
  <a href="#how-it-works-30-seconds">How it works</a> ·
  <a href="#proof">Proof</a> ·
  <a href="#compared-to">Compared to</a> ·
  <a href="#when-to-use--when-to-skip">When to use</a> ·
  <a href="#status--limitations">Status</a>
</p>

---

tare sits between an agent and the model API and shrinks the context window. Unlike most compressors
it is **lossless by default**, **cache-correct** (it never rewrites the provider's cached prefix), and
**closed-loop** — it watches the model's *output* and adapts. Opt in and it compresses aggressively:
row-capping, field-truncation, telegraphic NL, and AST code skeletonization.

## What it does

- **Proxy** — `tare-proxy`, point your agent's base URL at it; zero code changes, any language.
- **Library** — call the `tare-core` engine directly from Rust.
- **CLI** — `tare compress | slim-schema | compact-lossy | skeletonize | compact-html | compact-csv | deref-images | doctor | perf | learn | dashboard | output-savings | update | wrap | unwrap` — transforms, diagnostics, and ops.
- **MCP server** — `tare-mcp` exposes `tare_skeletonize` / `tare_compact_lossy` / `tare_compress` /
  `tare_deref_images` plus a reversible **`tare_expand`** (retrieve any original by id), and persistent
  cross-session memory (`tare_remember` / `tare_recall` / `tare_forget` / `tare_memory_stats`) to any MCP client.
- **Lossless by default** — re-encodes tool output, logs, and JSON into a denser *equivalent* form;
  it only drops information when you explicitly opt in.
- **Cache-correct** — detects the provider's cache breakpoint and only compresses the dynamic suffix,
  so your 90%-discount prefix cache keeps hitting.
- **Closed-loop** — watches output verbosity (the *compression paradox*), context fill, and cache
  hit-rate, and dials compression up or down per session.

## Why

- **Lossless wins where it can.** Tool output, logs, and JSON re-encode losslessly into a far denser
  form; only drop information when the caller accepts it.
- **The cache is the economy.** Provider prefix caches discount cached tokens ~10×. Perturb the
  cached prefix and a 90% discount becomes 0%. tare only ever rewrites the dynamic suffix.
- **Compression has a feedback loop.** Over-compressing makes models *compensate with verbose
  output*, so total cost can rise even as input falls. tare is the only proxy that watches output and
  backs off.

## How it works (30 seconds)

```
 Your agent / app  (Claude Code, Cursor, Codex, your own loop…)
      │  prompts · tool outputs · logs · file reads · RAG results
      ▼
  ┌──────────────────────────────────────────────────────────┐
  │  tare   (runs locally — your data and API key stay here)  │
  │  ──────────────────────────────────────────────────────  │
  │  cache-boundary detect → only touch the dynamic suffix    │
  │  lossless passes:  supersession · IVM/delta · dedup ·     │
  │                    columnar JSON/log · schema-slim ·      │
  │                    query-relevance (keyword)              │
  │  opt-in lossy:     row-cap · field-truncate · telegraphic │
  │                    · AST code skeletonization             │
  │  closed-loop controller:  cache-hit-rate (halt) ·         │
  │                    output-verbosity (back off) ·          │
  │                    context-fill (compress harder)         │
  └──────────────────────────────────────────────────────────┘
      │  compressed request                ▲  response streamed back unchanged
      ▼                                     │  (output tokens observed for the loop)
 LLM provider  (Anthropic /v1/messages · OpenAI /v1/chat/completions)
```

## Get started (60 seconds)

```bash
# 1 — install (no Rust toolchain needed)
curl -fsSL https://raw.githubusercontent.com/mstuart/tare/main/install.sh | sh   # → ~/.local/bin
# or:  npm install -g tare-ai
# or:  cargo install tare-cli                               # on crates.io from v0.2.0
# or:  docker pull ghcr.io/mstuart/tare                    # published to GHCR on each tagged release
# or:  git clone https://github.com/mstuart/tare && cd tare && cargo build --release

# 2 — run as a proxy (point your agent's base URL at http://localhost:8787; zero code changes)
TARE_UPSTREAM=https://api.anthropic.com tare-proxy
# or via Docker:
docker run --rm -e TARE_UPSTREAM=https://api.anthropic.com -p 8787:8787 ghcr.io/mstuart/tare

# 3 — or use the CLI on any stdin
cat big.rs | tare skeletonize --path big.rs    # drop fn bodies, keep structure
ps aux     | tare compact-lossy --max-rows 30 --max-field 110
```

Proxy env: `TARE_UPSTREAM` (default `https://api.anthropic.com`), `TARE_PORT` (`8787`),
`TARE_RECENCY` (`4`), `TARE_ENABLED` (`true`), `TARE_CONTEXT_LIMIT` (`200000`), `TARE_OUTPUT_HOLDOUT`
(`0` — fraction of sessions that bypass compression so `output-savings` can A/B the output tokens),
`TARE_LOG` (unset — set it to log one line per turn with the compression report).
Response headers report what it did: `x-tare-input-tokens`, `x-tare-net-tokens`, `x-tare-dropped`,
`x-tare-aggression`, `x-tare-verbosity-spike`, `x-tare-halted`. Admin surface: `GET /admin/stats`
(cumulative-savings JSON) and `POST /admin/runtime-env` (hot-sync `TARE_ENABLED`/`TARE_RECENCY` live
with no restart — send `Content-Type: application/json`).

### Python

```bash
pip install tare-compress          # on PyPI from v0.2.0
```

```python
import tare

skeleton = tare.skeletonize(open("big.rs").read(), "big.rs")
slim     = tare.compact_html(raw_html)
out      = tare.compress(blocks_json, task="fix the login bug")
```

All functions: `compress`, `skeletonize`, `compact_lossy`, `compact_html`, `compact_csv`,
`slim_schema`, `telegraphic`, `deref_images`, `crush`, `expand`. The wheel is abi3 — one
build per platform, works on Python 3.9+.

## Integrations

### Python (`tare-compress`)

```python
from tare.integrations import (
    compress_messages,
    LiteLLMHandler,
    CompressionMiddleware,
    langchain_chat_model,
    agno_model,
    strands_model,
    anthropic_with_tare,
    openai_with_tare,
)
```

All imports are lazy — the module loads cleanly even when the underlying framework is not installed.

| Export | What it does | Where compression runs |
|---|---|---|
| `compress_messages(messages, task="")` | Core helper — converts OpenAI-style messages to tare blocks, compresses, and returns a condensed `list[dict]` | in-process (Rust binding) |
| `LiteLLMHandler(task="")` | `CustomLogger`-compatible callback for litellm; mutates `messages` in-place before each sync or async call. Register with `litellm.callbacks = [LiteLLMHandler()]` | in-process |
| `CompressionMiddleware(app, task="")` | ASGI middleware (Starlette / FastAPI / raw ASGI) — intercepts JSON chat bodies containing `messages` and compresses them transparently | in-process |
| `langchain_chat_model(base)` | Subclass factory — returns a tare-compressing subclass of any LangChain `BaseChatModel` with a `tare_task` field | in-process |
| `agno_model(base)` | Subclass factory — returns a tare-compressing subclass of any Agno `Model` | in-process |
| `strands_model(base)` | Subclass factory — returns a tare-compressing subclass of any Strands `Model` (Bedrock-style messages) | in-process |
| `anthropic_with_tare(client_kwargs=None, base_url="http://127.0.0.1:8787")` | Returns an `anthropic.Anthropic` client pointed at the tare proxy | **local proxy** |
| `openai_with_tare(client_kwargs=None, base_url="http://127.0.0.1:8787")` | Returns an `openai.OpenAI` client pointed at the tare proxy | **local proxy** |

The first six compress **in-process** via the Rust binding — no proxy process required.
`anthropic_with_tare` and `openai_with_tare` route through the local proxy instead; start it first with `tare-proxy`.

### JS / TS (`tare-ai`)

```js
const { withTare, tareMiddleware, startProxy, tareBaseUrl } = require("tare-ai");
// or ESM / TypeScript:
import { withTare, tareMiddleware, startProxy, tareBaseUrl } from "tare-ai";
```

All JS / TS adapters route through the **local proxy** (`tare-proxy`, default port `8787`).

| Export | What it does |
|---|---|
| `withTare(clientOptions?, port?)` | Merges the proxy `baseURL` into SDK client options — works with both the Anthropic and OpenAI JS SDKs: `new Anthropic(withTare({ apiKey: "..." }))` |
| `tareMiddleware(port?)` | Returns a `LanguageModelV1Middleware`-shaped object for the Vercel AI SDK; attaches the proxy URL as `_tare_base`. Actual HTTP routing still requires `withTare` on the underlying provider client |
| `startProxy(opts?)` | Spawns the vendored `tare-proxy` binary and returns `{ child, baseUrl, stop }` |
| `tareBaseUrl(port?)` | Returns `"http://127.0.0.1:<port>"` (utility) |

## Use with your Claude subscription — MCP, no API key

The proxy forwards whatever auth the client sends, so it works two ways. With a **billable API key** the
client sends `x-api-key` and the proxy passes it through. On a **Claude Pro/Max subscription** instead,
point Claude Code's `ANTHROPIC_BASE_URL` at the proxy — it forwards your subscription OAuth token
upstream, **no API key required** (`scripts/live-smoke-sub.sh` runs exactly this round-trip).

Prefer not to redirect a base URL at all? Use the MCP server: `tare-mcp` is a local stdio process your agent
launches and calls as tools — it never calls the model itself, so it needs **no API key** and rides on
whatever auth the host already has (your Claude Code `/login` subscription, or any MCP client).

```bash
# easiest — no build; npx fetches the prebuilt binary on first run:
claude mcp add tare -s user -- npx -y -p tare-ai tare-mcp

# or from a source build (user scope = every project; -s project writes .mcp.json instead):
cargo build --release -p tare-mcp
claude mcp add tare -s user "$(pwd)/target/release/tare-mcp"
```

That records a standard MCP entry. Commit it as a project `.mcp.json` to share with a repo, or paste the
same block into any other MCP client (Cursor, Codex, …):

```json
{
  "mcpServers": {
    "tare": { "command": "npx", "args": ["-y", "-p", "tare-ai", "tare-mcp"] }
  }
}
```

**Claude Desktop:** add that same `mcpServers` block to
`~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) and restart.

Tools: `tare_skeletonize`, `tare_compact_lossy`, `tare_compress`, `tare_deref_images` (replace inline
base64 images with compact `[tare-image id=… fmt=… ~NKB]` markers; originals retrievable via
`tare_expand`), a reversible `tare_expand` (retrieve any original by id), `tare_stats`, and the
cross-session memory tools `tare_remember` / `tare_recall` / `tare_forget` / `tare_memory_stats` —
JSON-RPC 2.0 over stdio (MCP protocol `2024-11-05`). The memory store is shared across any agents pointed
at the same `tare-mcp` process or the same `$TARE_MEMORY` db path (default `~/.config/tare/memory.db`).

## Wrap your agent

`tare wrap <agent>` starts the proxy and launches a supported CLI agent through it in one step,
forwarding `ANTHROPIC_BASE_URL`, `OPENAI_BASE_URL`, and `OPENAI_API_BASE` to the agent process for
the duration of that invocation. Wrapping is ENV-based and ephemeral — no global state is written.

```bash
tare wrap claude               # start proxy + launch Claude Code through it
tare wrap claude -- --print    # pass extra flags to the agent (after --)
tare wrap claude --print       # dry-run: show what would run without starting anything
tare wrap claude --port 9000   # use a custom proxy port
```

**Supported agents**

| Agent | Mode | Notes |
|---|---|---|
| `claude` | auto-launch | Claude Code CLI |
| `codex` | auto-launch | OpenAI Codex CLI |
| `aider` | auto-launch | aider CLI |
| `goose` | auto-launch | Block's Goose |
| `openhands` | auto-launch | OpenHands CLI |
| `opencode` | auto-launch | opencode CLI |
| `openclaw` | auto-launch | openclaw CLI |
| `vibe` | auto-launch | Vibe CLI |
| `cursor` | manual setup | prints base-URL instructions for Cursor's settings |
| `cline` | manual setup | prints base-URL instructions for the Cline extension |
| `continue` | manual setup | prints base-URL instructions for Continue extension |
| `cortex` | manual setup | prints base-URL instructions for the Cortex library |

Auto-launch agents: the proxy starts in the background, the agent binary is exec'd with the three
env vars set, and the proxy is killed when the agent exits. Manual-setup agents: `tare wrap` prints
step-by-step instructions for pointing that tool's base-URL setting at the proxy — no binary is
launched.

`tare unwrap <agent>` prints a reminder that wrapping is ephemeral and points to where to remove
any base-URL override if it was configured directly in the agent's settings.

## Proof

<p align="center"><img src="docs/assets/savings.svg" alt="tare token reduction by content type" width="680"></p>

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

Code reads are ~67–76% of a coding agent's tokens ([SWE-Pruner, ACL 2026](https://arxiv.org/abs/2601.16746)),
so skeletonization is the single biggest lever. The numbers above are tare-only; competitive
head-to-head harnesses (vs Headroom, LLMLingua-2, lean-ctx, RTK) live in
`crates/tare-bench/benchmarks/` — run the scripts there (with the competitor tools installed) to
reproduce the comparison; at **equal fidelity** tare matches or beats each, and is the only one with
a lossless mode and cross-turn dedup.

## Compared to

tare runs **locally**, is **lossless by default**, is **cache-correct**, and **closes the loop** on
output — none of the others do all four.

|  | Scope | Deploy | Local | Lossless default | Output-aware |
|---|---|---|:-:|:-:|:-:|
| **tare** | tools · logs · files · JSON · history | proxy · library · CLI · MCP | ✅ | ✅ | ✅ |
| [Headroom](https://github.com/chopratejas/headroom) | all context | proxy · lib · MCP | ✅ | ❌ (reversible via cache) | ❌ |
| [RTK](https://github.com/rtk-ai/rtk) | CLI command outputs | CLI wrapper | ✅ | ❌ | ❌ |
| [lean-ctx](https://github.com/yvgude/lean-ctx) | CLI commands, MCP tools | CLI · MCP | ✅ | ❌ | ❌ |
| LLMLingua-2 | prose / RAG | library (ML model) | ✅ | ❌ | ❌ |
| OpenAI / Anthropic native compaction | conversation history | provider-native | ❌ | ❌ | ❌ |

## When to use · When to skip

**Great fit if you…**
- run coding agents and want savings without losing information by default
- care about the provider cache staying warm (tare won't break the prefix)
- want code reads compressed structurally (signatures kept, bodies elidable on demand)

**Skip it if you…**
- only use a single provider's native compaction and don't need a cross-provider proxy
- run in a sandbox where a local proxy process can't run

<details>
<summary><b>What's inside</b></summary>

- **Lossless passes** — supersession (drop superseded tool outputs), IVM/delta (re-reads → diffs),
  envelope + exact dedup, columnar JSON & log re-encoding, JSON-Schema slimming, reasoning-trace
  pruning, query-relevance pruning (keyword/symbol-based by default; neural embeddings opt-in via the `neural-embed` feature).
- **Opt-in lossy** — large-array row-capping, per-line field truncation, token-level telegraphic NL
  compaction, **AST code skeletonization** (tree-sitter: rust/python/js/ts/go/java/c/c++/perl).
- **Closed-loop controller** — per-session aggression from cache-hit-rate, output-verbosity, and
  context-fill signals; cache-prefix-boundary aware; bounded body buffering and upstream timeouts.

</details>

## Architecture

| Crate | Role |
|---|---|
| `tare-core` | the compression engine: segmenter, passes, planner, lossless + lossy transforms, skeletonizer |
| `tare-tokenize` | fast approximate token counter (chars/4) |
| `tare-cache` | provider cache models / hit-rate floors |
| `tare-proxy` | the HTTP proxy + closed-loop controller + sensors |
| `tare-cli` | the `tare` command |
| `tare-memory` | persistent cross-session memory: SQLite-backed remember/recall with content-hash dedup and multi-source provenance |
| `tare-mcp` | MCP (stdio) server: compression tools + a reversible `tare_expand` + memory tools |
| `tare-py` | Python bindings (PyPI: `tare-compress`): abi3 wheel, py3.9+, exposes all core transforms |
| `tare-bench` | competitive benchmarks (not published) |

## Cargo features

| Feature | Default | Description |
|---|:-:|---|
| `neural-embed` | off | Semantic relevance via [fastembed](https://github.com/Anyscale/fastembed-rs). Replaces the default keyword/symbol relevance pass with **exact cosine ranking** over neural embeddings — no HNSW index, no approximate search. Downloads an embedding model on first use. |

The default build uses **keyword/symbol matching** for query-relevance pruning and has no external model dependency. Enable semantic relevance with:

```bash
cargo build --release --features neural-embed   # downloads an embedding model on first use
```

> Exact cosine is used (not HNSW or any approximate index) because at relevance-pass scale — a handful of candidate segments per turn — exact ranking is faster and strictly more accurate than approximate nearest-neighbour.

## Diagnostics & tuning

```bash
# Health check — engine self-test, tokenizer sanity, config, proxy probe, learned-profile status.
# Exits non-zero if any check fails.
tare doctor

# Measure compression savings and speed on a built-in sample corpus.
tare perf --sample

# Offline corpus analysis — reads files under ./logs, derives compression settings,
# and writes ~/.config/tare/profile.json.  The proxy auto-loads this file on startup.
# (Not online RL: learn analyses static files and produces a persisted profile.)
tare learn --from ./logs

# Live savings dashboard (polls the proxy's /admin/stats); --once prints a single snapshot.
tare dashboard

# Estimate OUTPUT-token reduction via an A/B holdout (run the proxy with TARE_OUTPUT_HOLDOUT=0.1).
tare output-savings

# Self-upgrade to the latest GitHub release (--check only reports, never modifies).
tare update --check
```

## Status & limitations

- **Installable today** — the [binary installer](#get-started-60-seconds), `npm install -g tare-ai`,
  and `docker pull ghcr.io/mstuart/tare` are live; every tagged release ships checksummed binaries
  for macOS (arm64 / x86_64) and Linux (x86_64). crates.io (`cargo install tare-cli`) and PyPI
  (`pip install tare-compress`) publish from v0.2.0. Build from source or `cargo install --git` to
  track HEAD between releases.
- **Tested** — 228 unit, integration, and property tests across 22 suites; `cargo fmt --check`,
  `clippy -D warnings`, and `cargo deny` (advisories · licenses · bans) gate every commit in CI.
  Verified end-to-end against the live Anthropic API: a full round-trip through `tare-proxy` on a
  Claude subscription (`scripts/live-smoke-sub.sh`), plus the MCP server driven over real stdio
  JSON-RPC.
- **Deploy it as a local sidecar** — tare runs next to your agent and forwards your credentials
  upstream without logging or persisting them; treat it as a trusted component on your own machine
  or network rather than shared multi-tenant infrastructure ([SECURITY.md](SECURITY.md)). Startup
  failures (HTTP client build, port bind, serve) exit with a clear `[tare-proxy] fatal: …` message
  and a non-zero status, never a panic backtrace.

**Known edges**

- The context-fill signal counts the serialized request (incl. JSON envelope), so it slightly
  over-estimates true fill (conservative — errs toward compressing sooner).
- A `>2 MB` *streaming* response whose final usage event straddles the 64 KB tail buffer may skip one
  verbosity sample (non-fatal).

**Non-goals (intentional scope, not gaps)**

- **Trained ML text-compressor.** tare does not ship Headroom's trained ML text-compressor and has no
  plans to. There is no training infrastructure, and on the included harness (`crates/tare-bench/`) tare's
  lossless engine already matches or beats LLMLingua-2 at equal fidelity. Shipping model weights would add
  download overhead, inference latency, and a non-trivial failure mode for no measurable improvement on the
  targets tare is built for.
- **Voice/audio compression.** tare is a text-context tool — it operates on text entering the context
  window. Audio is deliberately out of scope. Transcribe externally (Whisper, Deepgram, etc.) and feed the
  transcript through `tare compress`; that is the intended path.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). CI runs `cargo fmt --check`, `cargo clippy -D warnings`,
`cargo test`, and a release build — please make sure those pass locally.

## License

MIT — see [LICENSE](LICENSE).
