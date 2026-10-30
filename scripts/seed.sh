#!/usr/bin/env bash
set -euo pipefail

# Creates an endpoint and publishes one event against a running instance.
# Requires HOOKRELAY_API_KEY from `docker compose run --rm migrate bootstrap --name demo`
# or `cargo run -p hookrelay-bin -- bootstrap --name demo`.

key="${HOOKRELAY_API_KEY:?set HOOKRELAY_API_KEY}"
base="${HOOKRELAY_URL:-http://localhost:8090}"
endpoint_url="${HOOKRELAY_TARGET:-https://example.com/hooks}"

curl -fsS -X POST "$base/v1/endpoints" \
  -H "Authorization: Bearer $key" \
  -H 'Content-Type: application/json' \
  -d "{\"url\":\"$endpoint_url\"}"

curl -fsS -X POST "$base/v1/events" \
  -H "Authorization: Bearer $key" \
  -H 'Content-Type: application/json' \
  -H "Idempotency-Key: seed-$(date +%s)" \
  -d '{"type":"invoice.paid","payload":{"id":"in_demo","amount":4200}}'
