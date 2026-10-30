# Hookrelay

Webhook delivery service: accepts events, fans them out to subscriber endpoints,
signs every request, retries failures with backoff, and keeps an auditable
delivery log with replay.

## What it handles

- At-least-once delivery with per-delivery idempotency keys
- HMAC-SHA256 signatures with timestamp replay protection and secret rotation
- Per-endpoint ordering without a global lock
- Retries with exponential backoff and jitter, then a dead letter queue
- Manual replay that preserves the original attempt history
- SSRF protection for user-supplied URLs, including pinned DNS resolution
- Per-key rate limiting
- Dashboard for endpoints, events, deliveries, and replay
- Prometheus metrics and readiness checks
- Worker metrics served on a separate address, since each worker is its own process

## Stack

Rust 2024, axum, sqlx, Postgres 17, Redis 8, Askama + HTMX, Caddy.

Postgres is both the store and the queue. Events and their deliveries commit in
one transaction, so an accepted event can never be missing work. Workers claim
due rows with `FOR UPDATE SKIP LOCKED`; ordered endpoints additionally take a
transaction-scoped advisory lock, which allows at most one in-flight delivery
per endpoint without a table-wide lock.

## Layout

```
crates/core/     domain logic, no I/O: signing, backoff, outcome, SSRF ranges
crates/store/    sqlx repositories and migrations
crates/sender/   HTTP delivery with timeouts, redirect refusal, response caps
crates/worker/   claim loop, execution, retries, lease reaper
crates/web/      JSON API, HTMX dashboard, auth, rate limiting, metrics
crates/bin/      webhookd: serve, worker, migrate, bootstrap
migrations/      Postgres schema
```

## Local run

```bash
docker compose up -d postgres redis
cargo run -p hookrelay-bin -- migrate
cargo run -p hookrelay-bin -- bootstrap --name demo   # prints an API key once
cargo run -p hookrelay-bin -- serve                   # http://127.0.0.1:8090
cargo run -p hookrelay-bin -- worker --id worker-1
```

Publish an event:

```bash
curl -X POST http://127.0.0.1:8090/v1/events \
  -H "Authorization: Bearer $HOOKRELAY_API_KEY" \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: invoice-42' \
  -d '{"type":"invoice.paid","payload":{"id":"in_42","amount":4200}}'
```

Create an endpoint:

```bash
curl -X POST http://127.0.0.1:8090/v1/endpoints \
  -H "Authorization: Bearer $HOOKRELAY_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"url":"https://example.com/hooks"}'
```

The response includes the signing secret exactly once.

## Signature verification

Each request carries:

```
X-Hookrelay-Signature: v1,<unix-timestamp>,<hex-hmac>
X-Hookrelay-Id: <delivery-id>
X-Hookrelay-Event: <event-id>
X-Hookrelay-Attempt: <attempt-number>
Idempotency-Key: <delivery-id>
```

The digest is HMAC-SHA256 over `<timestamp>.<body>`. Reject timestamps outside
300 seconds. During rotation the previous secret stays valid for 24 hours.

## Dashboard

`http://127.0.0.1:8090/login` accepts an API key. Pages: overview, endpoints,
events, deliveries, dead letters, delivery detail with attempt timeline and
replay. The UI is server rendered with HTMX, no client build step.

## Tests

```bash
cargo test
```

Unit tests cover signing, backoff, outcome classification, and SSRF ranges.
Integration tests use testcontainers for Postgres and Redis and cover fan-out,
idempotency, ordering under concurrent claims, retries, dead letters, replay,
auth, rate limiting, and the dashboard. Chaos tests abort a worker mid-delivery
and verify the lease reaper returns the row, and run a failing receiver through
recovery.

## Load test

```bash
k6 run -e HOOKRELAY_API_KEY=<key> -e HOOKRELAY_URL=http://localhost:8090 scripts/load.js
```

No measured numbers are published yet. Run the command on the target hardware
and record the output here before claiming performance.

## Deploy

```bash
cp .env.example .env    # set HOOKRELAY_SESSION_SECRET and HOOKRELAY_DOMAIN
docker compose up --build -d
```

Compose runs Postgres, Redis, a migration job, the API, two workers, and Caddy
for TLS. The API is exposed on `:8090`; Caddy fronts the domain.

## Operational notes

- The API exposes `/metrics`. Workers expose their own metrics on `METRICS_ADDR`
  (default `127.0.0.1:9090`) because counters live in the worker process.
- Rate limiting fails open when Redis is down; ingest availability is preferred
  over strict limits, and the path is logged.
- The dashboard session cookie is HMAC signed, HTTP-only, and expires after a
  day.
- Endpoint secrets are stored as bytes because they must be used to sign. A
  production deployment should encrypt them at rest with a KMS key.
- Single region, single database. Sharding and cross-region replication are out
  of scope.
