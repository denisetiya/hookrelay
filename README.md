# Hookrelay

Webhook delivery service: accepts events, fans them out to subscriber endpoints,
signs each request, retries failures with backoff, and keeps an auditable
delivery log with manual replay.

Status: scaffold. Domain logic and schema exist; API, worker, and dashboard are
not built yet.

## Why this exists

Delivering webhooks is easy until the receiver is down, slow, or malicious.
Hookrelay targets the parts that break in production:

- Signing and replay protection
- At-least-once delivery with idempotency keys
- Per-endpoint ordering without a global lock
- SSRF protection for user-supplied URLs
- Visible retry history, dead letters, and replay
- Backpressure and bounded concurrency

## Stack

Rust, axum, sqlx, Postgres, Redis (rate limiting only), Askama + HTMX.

Postgres is the queue: deliveries are claimed with `FOR UPDATE SKIP LOCKED`, so
there is no second source of truth and no dual-write problem. Events and their
deliveries are written in one transaction.

## Layout

```
crates/core/     domain logic, no I/O: signing, backoff, SSRF classification
crates/store/    sqlx repositories and migrations
crates/sender/   HTTP delivery with timeouts and response caps
crates/worker/   claim loop, leases, ordering
crates/web/      axum routes, Askama templates, HTMX handlers
crates/bin/      webhookd: serve, worker, migrate
migrations/      Postgres schema
docs/            design spec
```

## Local infrastructure

```
docker compose up -d
```

## Test

```
cargo test
```
