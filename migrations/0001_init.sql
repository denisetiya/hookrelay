-- Hookrelay initial schema.
-- One Postgres transaction writes an event and all of its deliveries, so a
-- crash can never leave an accepted event without work queued.

create table tenants (
    id          uuid primary key,
    name        text not null,
    created_at  timestamptz not null default now()
);

create table api_keys (
    id          uuid primary key,
    tenant_id   uuid not null references tenants (id) on delete cascade,
    prefix      text not null unique,
    key_hash    text not null,
    created_at  timestamptz not null default now(),
    revoked_at  timestamptz
);

create table endpoints (
    id                  uuid primary key,
    tenant_id           uuid not null references tenants (id) on delete cascade,
    url                 text not null,
    description         text,
    secret_current      bytea not null,
    secret_previous     bytea,
    secret_rotated_at   timestamptz,
    event_types         text[] not null default '{}',
    status              text not null default 'active'
                        check (status in ('active', 'paused', 'disabled')),
    max_attempts        smallint not null default 8 check (max_attempts between 1 and 20),
    rate_limit_per_sec  integer not null default 50 check (rate_limit_per_sec > 0),
    max_concurrency     smallint not null default 4 check (max_concurrency between 1 and 64),
    ordering_mode       text not null default 'endpoint'
                        check (ordering_mode in ('endpoint', 'none')),
    created_at          timestamptz not null default now()
);

create index endpoints_tenant_idx on endpoints (tenant_id);

create table events (
    id               uuid primary key,
    tenant_id        uuid not null references tenants (id) on delete cascade,
    type             text not null,
    payload          jsonb not null,
    idempotency_key  text,
    created_at       timestamptz not null default now(),
    -- NULL keys stay distinct, so only explicit retries are deduplicated.
    unique (tenant_id, idempotency_key)
);

create index events_tenant_created_idx on events (tenant_id, created_at desc);

create table deliveries (
    id                uuid primary key,
    event_id          uuid not null references events (id) on delete cascade,
    endpoint_id       uuid not null references endpoints (id) on delete cascade,
    status            text not null default 'pending'
                      check (status in ('pending', 'in_flight', 'succeeded', 'retrying', 'dead')),
    attempt           smallint not null default 0,
    sequence          bigint generated always as identity,
    next_attempt_at   timestamptz not null default now(),
    lease_owner       text,
    lease_until       timestamptz,
    last_status_code  smallint,
    last_error        text,
    response_excerpt  text,
    replay_of         uuid references deliveries (id) on delete set null,
    created_at        timestamptz not null default now(),
    completed_at      timestamptz
);

-- The claim query only ever scans work that is due.
create index deliveries_due_idx
    on deliveries (next_attempt_at)
    where status in ('pending', 'retrying');

-- Ordered endpoints may have at most one delivery in flight, which is what keeps
-- event order stable without a separate lock per endpoint.
create unique index deliveries_one_in_flight_per_endpoint
    on deliveries (endpoint_id)
    where status = 'in_flight';

create index deliveries_event_idx on deliveries (event_id);
create index deliveries_endpoint_created_idx on deliveries (endpoint_id, created_at desc);
create index deliveries_status_idx on deliveries (status);

create table delivery_attempts (
    id                bigint generated always as identity primary key,
    delivery_id       uuid not null references deliveries (id) on delete cascade,
    attempt_no        smallint not null,
    started_at        timestamptz not null,
    finished_at       timestamptz,
    status_code       smallint,
    latency_ms        integer,
    error             text,
    response_excerpt  text,
    unique (delivery_id, attempt_no)
);

create index delivery_attempts_delivery_idx on delivery_attempts (delivery_id, attempt_no);
