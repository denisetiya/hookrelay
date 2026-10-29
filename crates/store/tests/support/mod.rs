// Each integration test binary compiles its own copy of this module, so helpers
// used by only some of them would otherwise trip dead_code under -D warnings.
#![allow(dead_code)]

use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

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

pub async fn create_endpoint(
    pool: &sqlx::PgPool,
    tenant_id: uuid::Uuid,
) -> hookrelay_store::models::Endpoint {
    create_endpoint_with_types(pool, tenant_id, &[]).await
}

pub async fn create_endpoint_with_types(
    pool: &sqlx::PgPool,
    tenant_id: uuid::Uuid,
    event_types: &[&str],
) -> hookrelay_store::models::Endpoint {
    let types: Vec<String> = event_types
        .iter()
        .map(|value| (*value).to_owned())
        .collect();

    hookrelay_store::endpoints::create(
        pool,
        hookrelay_store::endpoints::NewEndpoint {
            tenant_id,
            url: "https://example.com/hooks",
            description: None,
            secret: b"0123456789abcdef0123456789abcdef",
            event_types: &types,
            ordering_mode: "endpoint",
        },
    )
    .await
    .unwrap()
}

pub async fn ingest(
    pool: &sqlx::PgPool,
    tenant_id: uuid::Uuid,
    event_type: &str,
) -> hookrelay_store::events::Ingested {
    hookrelay_store::events::ingest(
        pool,
        hookrelay_store::events::IngestEvent {
            id: uuid::Uuid::now_v7(),
            tenant_id,
            event_type,
            payload: &serde_json::json!({}),
            idempotency_key: None,
        },
    )
    .await
    .unwrap()
}
