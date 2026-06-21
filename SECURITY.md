# Security Policy

## Reporting a vulnerability

Please report security issues **privately** — do not open a public issue.

- Preferred: open a [GitHub private security advisory](https://github.com/mstuart/cull/security/advisories/new).
- Or email: **mstuart@users.noreply.github.com**

Please include reproduction steps and the affected version/commit. You can expect an acknowledgement
within a few days.

## Scope notes

cull's proxy sits in the request path and **forwards your provider API key** (`x-api-key` /
`Authorization`) to the configured upstream — it never logs or persists it, but treat the proxy as a
trusted component on your own network. The proxy buffers request bodies up to a fixed cap
(`MAX_BODY_BYTES`, 32 MB) and applies upstream timeouts; reports of ways to bypass these resource
limits, crash a worker, or cause the proxy to mishandle credentials are in scope.
