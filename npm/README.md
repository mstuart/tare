# tare-ai

Lossless-by-default context compression for LLM coding agents — HTTP **proxy**, **CLI**, and **MCP server**.
This npm package is a thin wrapper: on install it downloads the self-contained, prebuilt binary for your
platform (macOS / Linux, arm64 / x64) from the project's GitHub Release. No Rust toolchain required.

```bash
npm install -g tare-ai        # puts tare, tare-proxy, tare-mcp on your PATH
```

Run the MCP server with no global install (ideal for an MCP client config):

```jsonc
{
  "mcpServers": {
    "tare": { "command": "npx", "args": ["-y", "-p", "tare-ai", "tare-mcp"] }
  }
}
```

CLI and proxy:

```bash
cat big.rs | tare skeletonize --path big.rs
TARE_UPSTREAM=https://api.anthropic.com tare-proxy
```

Full docs: https://github.com/mstuart/tare
