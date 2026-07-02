# Integrations

tare integrates in-process (Python), via the local proxy (any language), or as an MCP server.
This page is the full adapter reference; the [README](../README.md) has the short version.

## Python — `tare-compress`

```bash
pip install tare-compress
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

### Framework adapters

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
`anthropic_with_tare` and `openai_with_tare` route through the local proxy instead; start it first
with `tare-proxy`.

## JS / TS — `tare-ai`

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
