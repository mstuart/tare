#!/usr/bin/env bash
# Live end-to-end smoke test on a Claude subscription — NO API key. Runs tare-proxy in front of the
# REAL Anthropic API and drives it with the actual `claude` CLI (base URL redirected at the proxy),
# exactly how you'd run tare in front of Claude Code day to day. The client sends its subscription
# OAuth token; tare-proxy forwards it upstream (the `authorization` header is in FORWARD_HEADERS), so
# the proxy never needs a key of its own. Costs a few cents (one small Haiku call).
#
#   claude /login                 # once, to a Pro/Max subscription, if not already logged in
#   scripts/live-smoke-sub.sh
#
set -euo pipefail

command -v claude >/dev/null 2>&1 || {
  echo "need the 'claude' CLI on PATH (and 'claude /login' to a Pro/Max subscription)" >&2
  exit 1
}
PORT="${TARE_PORT:-8799}"
MODEL="${TARE_SMOKE_MODEL:-haiku}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROXY_LOG="$(mktemp -t tare-sub-proxy.XXXXXX)"

cargo build --release -p tare-proxy --manifest-path "$ROOT/Cargo.toml"

TARE_LOG=1 TARE_UPSTREAM=https://api.anthropic.com TARE_PORT="$PORT" \
  "$ROOT/target/release/tare-proxy" 2>"$PROXY_LOG" &
PROXY=$!
trap 'kill "$PROXY" 2>/dev/null || true' EXIT

# Wait for the listener (up to ~5s).
for _ in $(seq 1 50); do
  lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1 && break
  sleep 0.1
done

echo "== driving the real claude CLI through tare-proxy (subscription, no API key) =="
ans=$(env -u ANTHROPIC_API_KEY ANTHROPIC_BASE_URL="http://127.0.0.1:$PORT" \
  claude -p "Reply with ONLY the number, no words. What port is configured here? config = {service: gateway, host: 0.0.0.0, port: 8421, workers: 4}" \
  --model "$MODEL" 2>/dev/null | tr -cd '0-9')

echo "-- model answer (expect 8421): ${ans:-<empty>}"
echo "-- tare-proxy report (in/net/dropped/aggression per turn) --"
grep -E 'tare-proxy\] [0-9]' "$PROXY_LOG" || echo "(no request reached the proxy)"

if [ "$ans" = "8421" ]; then
  echo "== ok: round-trip through tare-proxy on the subscription succeeded =="
else
  echo "!! unexpected answer — check auth (claude /login) and the proxy report above" >&2
  exit 1
fi
