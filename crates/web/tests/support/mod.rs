// Each integration test binary compiles its own copy of this module, so helpers
// used by only some of them would otherwise trip dead_code under -D warnings.
#![allow(dead_code)]

use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHasher};
use redis::aio::ConnectionManager;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use uuid::Uuid;

pub struct TestDb {
    pub pool: sqlx::PgPool,
    _container: ContainerAsync<Postgres>,
}

pub async fn test_db() -> TestDb {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = hookrelay_store::db::connect(&url).await.unwrap();

    hookrelay_store::db::migrate(&pool).await.unwrap();

    TestDb {
        pool,
        _container: container,
    }
}

pub async fn seed_key(pool: &sqlx::PgPool) -> (Uuid, String) {
    let tenant = hookrelay_store::tenants::create(pool, "Acme")
        .await
        .unwrap();
    let prefix = "abcdefghijkl";
    let secret = "deadbeefcafe";

    let salt = SaltString::encode_b64(b"0123456789abcdef").unwrap();
    let hash = Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .unwrap()
        .to_string();

    hookrelay_store::api_keys::create(pool, tenant.id, prefix, &hash)
        .await
        .unwrap();

    (tenant.id, format!("hr_live_{prefix}_{secret}"))
}

pub fn app(pool: sqlx::PgPool) -> axum::Router {
    app_with_limiter(pool, hookrelay_web::rate_limit::RateLimiter::disabled())
}

pub fn app_with_limiter(
    pool: sqlx::PgPool,
    limiter: hookrelay_web::rate_limit::RateLimiter,
) -> axum::Router {
    hookrelay_web::router(hookrelay_web::AppState {
        pool,
        allow_http_targets: true,
        limiter,
        session_secret: b"0123456789abcdef0123456789abcdef".to_vec(),
        metrics: None,
    })
}

pub async fn test_redis() -> (ConnectionManager, ContainerAsync<Redis>) {
    let container = Redis::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let client = redis::Client::open(format!("redis://127.0.0.1:{port}")).unwrap();
    let manager = ConnectionManager::new(client).await.unwrap();

    (manager, container)
}

pub async fn seed_delivery(pool: &sqlx::PgPool, tenant_id: Uuid) -> Uuid {
    hookrelay_store::endpoints::create(
        pool,
        hookrelay_store::endpoints::NewEndpoint {
            tenant_id,
            url: "https://example.com/hooks",
            description: None,
            secret: b"0123456789abcdef0123456789abcdef",
            event_types: &[],
            ordering_mode: "endpoint",
        },
    )
    .await
    .unwrap();

    let ingested = hookrelay_store::events::ingest(
        pool,
        hookrelay_store::events::IngestEvent {
            id: Uuid::now_v7(),
            tenant_id,
            event_type: "invoice.paid",
            payload: &serde_json::json!({"id": "in_1"}),
            idempotency_key: None,
        },
    )
    .await
    .unwrap();

    ingested.delivery_ids[0]
}

pub fn session_cookie(tenant_id: Uuid) -> String {
    let value = hookrelay_web::session::issue(
        b"0123456789abcdef0123456789abcdef",
        tenant_id,
        "abcdefghijkl",
    );

    format!("hr_session={value}")
}

pub fn app_with_metrics(
    pool: sqlx::PgPool,
    metrics: metrics_exporter_prometheus::PrometheusHandle,
) -> axum::Router {
    hookrelay_web::router(hookrelay_web::AppState {
        pool,
        allow_http_targets: true,
        limiter: hookrelay_web::rate_limit::RateLimiter::disabled(),
        session_secret: b"0123456789abcdef0123456789abcdef".to_vec(),
        metrics: Some(metrics),
    })
}
