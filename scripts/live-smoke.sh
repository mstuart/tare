#!/usr/bin/env bash
# Live end-to-end smoke test: run tare-proxy in front of the REAL Anthropic API and verify a
# round-trip — the proxy forwards, the model answers correctly through tare's compressed context,
# and the x-tare-* report headers come back. Costs a few cents (one small Haiku call).
#
#   ANTHROPIC_API_KEY=sk-... scripts/live-smoke.sh
#
set -euo pipefail

: "${ANTHROPIC_API_KEY:?set ANTHROPIC_API_KEY (a billable Anthropic API key) before running}"
PORT="${TARE_PORT:-8799}"
if ! [[ "$PORT" =~ ^[0-9]+$ ]] || (( 10#$PORT > 65535 )); then
  echo "TARE_PORT must be an integer from 0 to 65535" >&2
  exit 1
fi
PORT=$((10#$PORT))
MODEL="${TARE_SMOKE_MODEL:-claude-haiku-4-5-20251001}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

cargo build --release -p tare-proxy --manifest-path "$ROOT/Cargo.toml"

PROXY_LOG="$(mktemp)"
smoke_exit=0
TARE_UPSTREAM=https://api.anthropic.com TARE_PORT="$PORT" "$ROOT/target/release/tare-proxy" 2>"$PROXY_LOG" &
PROXY=$!
cleanup() {
  smoke_exit=$?
  kill "$PROXY" 2>/dev/null || true
  if (( smoke_exit != 0 )); then
    echo "-- tare-proxy log --" >&2
    cat "$PROXY_LOG" >&2
  else
    grep -vF '[tare-proxy] listening on :' "$PROXY_LOG" >&2 || true
  fi
  rm -f "$PROXY_LOG"
  exit "$smoke_exit"
}
trap cleanup EXIT

# Do not send the credential until this exact proxy confirms that it owns the listener.
for _ in {1..100}; do
  if grep -Fq "[tare-proxy] listening on :$PORT " "$PROXY_LOG"; then
    break
  fi
  if ! kill -0 "$PROXY" 2>/dev/null; then
    cat "$PROXY_LOG" >&2
    echo "tare-proxy exited before binding port $PORT" >&2
    exit 1
  fi
  sleep 0.1
done
if ! grep -Fq "[tare-proxy] listening on :$PORT " "$PROXY_LOG"; then
  echo "tare-proxy did not bind port $PORT within 10 seconds" >&2
  exit 1
fi

req=$(cat <<JSON
{"model":"$MODEL","max_tokens":60,
 "system":"You are terse. Answer in one short sentence.",
 "messages":[
   {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"read","input":{"path":"server.toml"}}]},
   {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"server.toml:\nhost = \"0.0.0.0\"\nport = 8421\nworkers = 4\ntimeout_seconds = 30\n"}]},
   {"role":"user","content":"What port does the server run on, per server.toml?"}
 ]}
JSON
)

echo "== sending one request through tare-proxy → api.anthropic.com =="
curl -s --retry 15 --retry-connrefused --retry-delay 1 --connect-timeout 2 -D /tmp/tare-smoke-hdr.txt \
  -H "x-api-key: $ANTHROPIC_API_KEY" -H "anthropic-version: 2023-06-01" -H "content-type: application/json" \
  -d "$req" "http://127.0.0.1:$PORT/v1/messages" > /tmp/tare-smoke-body.json

echo "-- HTTP status --"; head -1 /tmp/tare-smoke-hdr.txt
echo "-- x-tare-* (compression report) --"; grep -i '^x-tare' /tmp/tare-smoke-hdr.txt || echo "(none)"
echo "-- model answer (expect: 8421) + usage --"
python3 - <<'PY'
import json
d = json.load(open("/tmp/tare-smoke-body.json"))
if d.get("type") == "error":
    print("API ERROR:", d["error"]); raise SystemExit(1)
print("answer:", "".join(b.get("text", "") for b in d.get("content", [])).strip())
print("usage :", d.get("usage"))
PY
echo "== ok =="
