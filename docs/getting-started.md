# Getting started

## Build

```bash
git clone https://github.com/mstuart/cull && cd cull
cargo build --release          # builds target/release/{cull, cull-proxy}
```

Or with Docker:

```bash
docker compose up --build      # runs cull-proxy on :8787
```

## As a proxy

Point your agent's API base URL at cull; it forwards to the real upstream and compresses on the way.

```bash
CULL_UPSTREAM=https://api.anthropic.com CULL_PORT=8787 ./target/release/cull-proxy
```

It speaks Anthropic (`/v1/messages`) and OpenAI (`/v1/chat/completions`).

| Env var | Default | Meaning |
|---|---|---|
| `CULL_UPSTREAM` | `https://api.anthropic.com` | upstream API base URL |
| `CULL_PORT` | `8787` | listen port |
| `CULL_RECENCY` | `4` | tool outputs always kept regardless of relevance |
| `CULL_ENABLED` | `true` | set `0`/`false` for byte-exact passthrough |
| `CULL_CONTEXT_LIMIT` | `200000` | model context window (drives the fill-based aggression dial) |

Response headers report what it did: `x-cull-net-tokens`, `x-cull-dropped`, `x-cull-aggression`,
`x-cull-verbosity-spike`, `x-cull-halted`.

## As a CLI

```bash
cat ctx.json   | cull compress --task "fix the auth bug"
cat big.rs     | cull skeletonize --path big.rs
ps aux         | cull compact-lossy --max-rows 30 --max-field 110
```

See the [CLI reference](cli.md) for all subcommands.
