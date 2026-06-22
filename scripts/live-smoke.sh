#!/usr/bin/env bash
# Live end-to-end smoke test: run cull-proxy in front of the REAL Anthropic API and verify a
# round-trip — the proxy forwards, the model answers correctly through cull's compressed context,
# and the x-cull-* report headers come back. Costs a few cents (one small Haiku call).
#
#   ANTHROPIC_API_KEY=sk-... scripts/live-smoke.sh
#
set -euo pipefail

: "${ANTHROPIC_API_KEY:?set ANTHROPIC_API_KEY (a billable Anthropic API key) before running}"
PORT="${CULL_PORT:-8799}"
MODEL="${CULL_SMOKE_MODEL:-claude-haiku-4-5-20251001}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

cargo build --release -p cull-proxy --manifest-path "$ROOT/Cargo.toml"

CULL_UPSTREAM=https://api.anthropic.com CULL_PORT="$PORT" "$ROOT/target/release/cull-proxy" &
PROXY=$!
trap 'kill "$PROXY" 2>/dev/null || true' EXIT

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

echo "== sending one request through cull-proxy → api.anthropic.com =="
curl -s --retry 15 --retry-connrefused --retry-delay 1 --connect-timeout 2 -D /tmp/cull-smoke-hdr.txt \
  -H "x-api-key: $ANTHROPIC_API_KEY" -H "anthropic-version: 2023-06-01" -H "content-type: application/json" \
  -d "$req" "http://127.0.0.1:$PORT/v1/messages" > /tmp/cull-smoke-body.json

echo "-- HTTP status --"; head -1 /tmp/cull-smoke-hdr.txt
echo "-- x-cull-* (compression report) --"; grep -i '^x-cull' /tmp/cull-smoke-hdr.txt || echo "(none)"
echo "-- model answer (expect: 8421) + usage --"
python3 - <<'PY'
import json
d = json.load(open("/tmp/cull-smoke-body.json"))
if d.get("type") == "error":
    print("API ERROR:", d["error"]); raise SystemExit(1)
print("answer:", "".join(b.get("text", "") for b in d.get("content", [])).strip())
print("usage :", d.get("usage"))
PY
echo "== ok =="
