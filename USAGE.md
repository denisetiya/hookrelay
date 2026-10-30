# Hookrelay usage guide

This guide covers running Hookrelay, publishing events, receiving webhooks,
operating the dashboard, and deploying it.

## 1. Requirements

- Rust 1.90 or newer
- Docker with Compose (for Postgres and Redis)
- A receiver URL that is reachable over HTTPS. Local HTTP is allowed only in
  development with `HOOKRELAY_ALLOW_PRIVATE_TARGETS=1`.

## 2. Configuration

Copy `.env.example` to `.env` and adjust:

| Variable | Required | Default | Purpose |
| --- | --- | --- | --- |
| `DATABASE_URL` | yes | none | Postgres connection string |
| `REDIS_URL` | yes | none | Redis connection string, used for rate limiting |
| `HOOKRELAY_SESSION_SECRET` | yes | none | HMAC key for dashboard sessions, at least 32 characters |
| `BIND_ADDR` | no | `127.0.0.1:8090` | API and dashboard listener |
| `METRICS_ADDR` | no | `127.0.0.1:9090` | Worker metrics listener |
| `LOG_FORMAT` | no | `pretty` | `pretty` or `json` |
| `RUST_LOG` | no | `info` | Log filter |
| `HOOKRELAY_ALLOW_PRIVATE_TARGETS` | no | unset | `1` allows HTTP and private IP targets. Development only. |

`HOOKRELAY_ALLOW_PRIVATE_TARGETS=1` disables the SSRF guard. Never set it on a
public deployment.

## 3. Start locally

```bash
docker compose up -d postgres redis

cargo run -p hookrelay-bin -- migrate
cargo run -p hookrelay-bin -- bootstrap --name demo
```

`bootstrap` prints an API key once:

```
tenant: demo
api key: hr_live_abcdefghijkl_<secret>
store this key now; it cannot be recovered.
```

Then run the API and at least one worker in separate terminals:

```bash
cargo run -p hookrelay-bin -- serve
cargo run -p hookrelay-bin -- worker --id worker-1
```

Open `http://127.0.0.1:8090/login` and paste the API key.

## 4. Authentication

Every API request uses a bearer token:

```
Authorization: Bearer hr_live_<prefix>_<secret>
```

Keys are created by `bootstrap` and stored as Argon2 hashes. There is no key
listing or revocation endpoint yet. To revoke one manually:

```sql
update api_keys set revoked_at = now() where prefix = '<prefix>';
```

Errors use a consistent shape:

```json
{"error":{"code":"unauthorized","message":"invalid api key"}}
```

## 5. Publish an event

```bash
curl -X POST http://127.0.0.1:8090/v1/events \
  -H "Authorization: Bearer $HOOKRELAY_API_KEY" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: invoice-42" \
  -d '{
    "type": "invoice.paid",
    "payload": {"id": "in_42", "amount": 4200, "currency": "USD"}
  }'
```

Response `201 Created`:

```json
{"id":"01a1...","type":"invoice.paid","created_at":"2026-10-30T01:23:15Z"}
```

Rules:

- `type` must be 1 to 200 characters.
- `payload` can be any JSON value. The body limit is 256 KiB.
- `Idempotency-Key` is optional, 1 to 255 characters, unique per tenant.
- Repeating the same key returns `200 OK` with the original event id and does
  not fan out again.
- The event is fanned out to every active endpoint whose `event_types` is empty
  or contains the event type.

Retrieve an event, including its payload:

```bash
curl http://127.0.0.1:8090/v1/events/<event-id> \
  -H "Authorization: Bearer $HOOKRELAY_API_KEY"
```

## 6. Manage endpoints

### Create

```bash
curl -X POST http://127.0.0.1:8090/v1/endpoints \
  -H "Authorization: Bearer $HOOKRELAY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://example.com/hooks",
    "description": "Billing service",
    "event_types": ["invoice.paid", "invoice.refunded"],
    "ordering_mode": "endpoint"
  }'
```

Fields:

| Field | Required | Default | Notes |
| --- | --- | --- | --- |
| `url` | yes | none | `https` only, unless private targets are enabled |
| `description` | no | null | Free text |
| `event_types` | no | `[]` | Empty means all events |
| `ordering_mode` | no | `endpoint` | `endpoint` is serial, `none` is parallel |

Response `201 Created` includes `secret`, a 64 character hex value. This is the
only time the secret is returned. Store it in the receiver's configuration.

### List

```bash
curl http://127.0.0.1:8090/v1/endpoints \
  -H "Authorization: Bearer $HOOKRELAY_API_KEY"
```

Secrets are never included in list responses.

### Pause, resume, disable

```bash
curl -X PATCH http://127.0.0.1:8090/v1/endpoints/<id> \
  -H "Authorization: Bearer $HOOKRELAY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"status":"paused"}'
```

Allowed values: `active`, `paused`, `disabled`. Paused and disabled endpoints
are skipped during fan-out.

### Rotate the signing secret

```bash
curl -X POST http://127.0.0.1:8090/v1/endpoints/<id>/rotate-secret \
  -H "Authorization: Bearer $HOOKRELAY_API_KEY"
```

The response contains the new secret once. For 24 hours after rotation,
requests also carry `X-Hookrelay-Signature-Previous` signed with the old secret.
The receiver should accept either header during the rollout window, then drop
the old secret.

Default retry settings (`max_attempts`, `rate_limit_per_sec`,
`max_concurrency`) can be changed in the database:

```sql
update endpoints set max_attempts = 12 where id = '<id>';
```

## 7. Inspect and replay deliveries

List recent deliveries:

```bash
curl "http://127.0.0.1:8090/v1/deliveries?status=dead&limit=20" \
  -H "Authorization: Bearer $HOOKRELAY_API_KEY"
```

Query parameters:

| Parameter | Values |
| --- | --- |
| `status` | `pending`, `in_flight`, `retrying`, `succeeded`, `dead` |
| `endpoint_id` | UUID |
| `limit` | 1 to 200, default 50 |

Delivery detail includes attempts and the event payload:

```bash
curl http://127.0.0.1:8090/v1/deliveries/<delivery-id> \
  -H "Authorization: Bearer $HOOKRELAY_API_KEY"
```

Replay a finished delivery:

```bash
curl -X POST http://127.0.0.1:8090/v1/deliveries/<delivery-id>/replay \
  -H "Authorization: Bearer $HOOKRELAY_API_KEY"
```

A new `pending` delivery is created with `replay_of` pointing at the original.
Only `succeeded` and `dead` deliveries can be replayed. Replaying an unfinished
delivery returns `409 Conflict`.

## 8. Receiving webhooks

Each request has:

```
Content-Type: application/json
X-Hookrelay-Signature: v1,<unix-timestamp>,<hex-hmac>
X-Hookrelay-Signature-Previous: v1,<unix-timestamp>,<hex-hmac>   (rotation only)
X-Hookrelay-Id: <delivery-id>
X-Hookrelay-Event: <event-id>
X-Hookrelay-Attempt: <attempt-number>
Idempotency-Key: <delivery-id>
```

The digest is HMAC-SHA256 over `<timestamp>.<raw-body>`. Reject timestamps more
than 300 seconds from now.

Verify with Python:

```python
import hashlib
import hmac
import time

def verify(secret_hex: str, header: str, raw_body: bytes) -> bool:
    version, timestamp, signature = header.split(",")
    if version != "v1":
        return False
    if abs(time.time() - int(timestamp)) > 300:
        return False

    expected = hmac.new(
        bytes.fromhex(secret_hex),
        f"{timestamp}.".encode() + raw_body,
        hashlib.sha256,
    ).hexdigest()

    return hmac.compare_digest(signature, expected)
```

Verify with Node:

```js
import crypto from "node:crypto";

function verify(secretHex, header, rawBody) {
  const [version, timestamp, signature] = header.split(",");
  if (version !== "v1") return false;
  if (Math.abs(Date.now() / 1000 - Number(timestamp)) > 300) return false;

  const expected = crypto
    .createHmac("sha256", Buffer.from(secretHex, "hex"))
    .update(`${timestamp}.`)
    .update(rawBody)
    .digest("hex");

  return crypto.timingSafeEqual(Buffer.from(signature), Buffer.from(expected));
}
```

Always compute the digest over the raw request body, before JSON parsing.

Receiver rules:

- Return any `2xx` to acknowledge.
- Deduplicate on `Idempotency-Key`, which stays the same across retries.
- Respond within 10 seconds. Slow responses are retried.
- Return `4xx` only when the event should never be retried. `408`, `425`, `429`,
  and `5xx` are retried.

## 9. Delivery semantics

| Response or condition | Result |
| --- | --- |
| `2xx` | `succeeded` |
| `408`, `425`, `429`, `5xx` | retried |
| Timeout, connection error, DNS failure | retried |
| Other `4xx` | `dead` immediately |
| `3xx` | `dead`, redirects are not followed |

Backoff starts at 5 seconds, doubles per attempt, has 20 percent jitter, caps at
1 hour, and stops at `max_attempts` (default 8). A `Retry-After` header in
seconds is honored up to 1 hour.

Ordering:

- `endpoint`: at most one delivery in flight per endpoint, oldest first.
- `none`: deliveries run in parallel.

If a worker crashes mid-delivery, the lease expires after 30 seconds and the
delivery returns to the queue. Delivery is at least once, so receivers must
deduplicate.

## 10. Dashboard

Sign in at `/login` with an API key. Pages:

- **Overview**: dead and retrying counts, recent deliveries.
- **Endpoints**: create, pause, resume, rotate secrets.
- **Events**: accepted events and payloads.
- **Deliveries**: filter by status, open detail, replay.
- **Dead letters**: failed deliveries that exhausted retries.

Replay from the list updates the row in place. Replay from a detail page
redirects to the new delivery.

Sessions are HMAC signed, HTTP-only, `SameSite=Lax`, and expire after 24 hours.

## 11. Health and metrics

```bash
curl http://127.0.0.1:8090/healthz   # process alive
curl http://127.0.0.1:8090/readyz    # database and Redis reachable
curl http://127.0.0.1:8090/metrics   # API process metrics
curl http://127.0.0.1:9090/metrics   # worker metrics
```

Worker metrics are separate because counters live in the worker process. In
Compose, `worker-1` serves on `9091` and `worker-2` on `9092`.

Metrics:

- `hookrelay_deliveries_total{status}`
- `hookrelay_delivery_latency_seconds`
- `hookrelay_queue_depth`
- `hookrelay_lease_expired_total`
- `hookrelay_claim_batch_seconds`

## 12. Rate limiting

Each API key has a token bucket of 120 requests with a refill of 2 requests per
second. When exhausted, the API returns `429` with `Retry-After`.

If Redis is unavailable, requests are allowed and a warning is logged. This is
deliberate: losing ingest is worse than briefly losing the limiter.

## 13. Deploy with Docker Compose

```bash
cp .env.example .env
# set HOOKRELAY_SESSION_SECRET, HOOKRELAY_DOMAIN, and strong database passwords
docker compose up --build -d
```

Services:

- `postgres` on host port 5433
- `redis` on host port 6380
- `migrate` runs once and exits
- `serve` on port 8090
- `worker-1`, `worker-2`
- `caddy` terminates TLS for `HOOKRELAY_DOMAIN`

Point the domain's DNS at the host before starting Caddy. Webhook targets must
use HTTPS; the SSRF guard rejects private and link-local addresses.

For a real deployment:

- Change the Postgres password in `docker-compose.yml` and `.env`.
- Back up the `pgdata` volume daily with `pg_dump`.
- Scrape `serve:8090/metrics` and each worker's metrics address.
- Run migrations as a separate job before starting new application versions.

## 14. Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `401 unauthorized` | Missing, malformed, or revoked key | Run `bootstrap` again or check `api_keys.revoked_at` |
| `422 url must use https` | Target uses `http` | Use HTTPS, or set `HOOKRELAY_ALLOW_PRIVATE_TARGETS=1` locally |
| Delivery dead with `blocked address` | SSRF guard rejected the resolved IP | Use a public address, or enable private targets in development |
| Deliveries stay `pending` | No worker running, or endpoint paused | Start a worker, check `endpoints.status` |
| `429 rate_limited` | API key bucket exhausted | Wait for `Retry-After` or use another key |
| Dashboard keeps redirecting to login | `HOOKRELAY_SESSION_SECRET` changed or cookie expired | Sign in again |
| `readyz` returns 503 | Postgres or Redis unreachable | Check `docker compose ps` and connection strings |
| Replay returns `409` | Delivery is still pending, in flight, or retrying | Wait until it is `succeeded` or `dead` |

## 15. Limitations

- One tenant per API key, and tenants are created only by the CLI.
- No API for key listing or revocation.
- Endpoint secrets are stored unencrypted in Postgres because they must be used
  to sign. Encrypt them at rest with a KMS key for production.
- Single region and a single Postgres primary.
- No custom request headers per endpoint.
- No published load numbers yet. See `scripts/load.js` and measure on your own
  hardware before quoting throughput.
