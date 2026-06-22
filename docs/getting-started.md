# Getting started

## Build

```bash
git clone https://github.com/mstuart/tare && cd tare
cargo build --release          # builds target/release/{tare, tare-proxy}
```

Or with Docker:

```bash
docker compose up --build      # runs tare-proxy on :8787
```

## As a proxy

Point your agent's API base URL at tare; it forwards to the real upstream and compresses on the way.

```bash
TARE_UPSTREAM=https://api.anthropic.com TARE_PORT=8787 ./target/release/tare-proxy
```

It speaks Anthropic (`/v1/messages`) and OpenAI (`/v1/chat/completions`).

| Env var | Default | Meaning |
|---|---|---|
| `TARE_UPSTREAM` | `https://api.anthropic.com` | upstream API base URL |
| `TARE_PORT` | `8787` | listen port |
| `TARE_RECENCY` | `4` | tool outputs always kept regardless of relevance |
| `TARE_ENABLED` | `true` | set `0`/`false` for byte-exact passthrough |
| `TARE_CONTEXT_LIMIT` | `200000` | model context window (drives the fill-based aggression dial) |

Response headers report what it did: `x-tare-net-tokens`, `x-tare-dropped`, `x-tare-aggression`,
`x-tare-verbosity-spike`, `x-tare-halted`.

## As a CLI

```bash
cat ctx.json   | tare compress --task "fix the auth bug"
cat big.rs     | tare skeletonize --path big.rs
ps aux         | tare compact-lossy --max-rows 30 --max-field 110
```

See the [CLI reference](cli.md) for all subcommands.

## As an MCP server

`tare-mcp` is a stdio MCP server. Point any MCP client at the `tare-mcp` binary; it exposes
`tare_skeletonize`, `tare_compact_lossy`, `tare_compress`, `tare_stats`, and a reversible
**`tare_expand`** — when a tool compacts something it returns an `id`, and `tare_expand({id})` returns
the exact original (CCR-style retrieval), so the agent can drill back in on demand.

```jsonc
// example MCP client config
{ "mcpServers": { "tare": { "command": "tare-mcp" } } }
```
