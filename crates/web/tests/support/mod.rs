// Each integration test binary compiles its own copy of this module, so helpers
// used by only some of them would otherwise trip dead_code under -D warnings.
#![allow(dead_code)]

use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHasher};
use testcontainers_modules::postgres::Postgres;
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
    hookrelay_web::router(hookrelay_web::AppState {
        pool,
        allow_http_targets: true,
    })
}
