use uuid::Uuid;

use crate::error::StoreError;
use crate::models::ApiKey;

pub async fn create(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    prefix: &str,
    key_hash: &str,
) -> Result<ApiKey, StoreError> {
    sqlx::query_as::<_, ApiKey>(
        "insert into api_keys (id, tenant_id, prefix, key_hash) values ($1, $2, $3, $4)
         returning id, tenant_id, prefix, key_hash, created_at, revoked_at",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(prefix)
    .bind(key_hash)
    .fetch_one(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            StoreError::Conflict("api key prefix already exists")
        }
        other => StoreError::Db(other),
    })
}

pub async fn find_active_by_prefix(
    pool: &sqlx::PgPool,
    prefix: &str,
) -> Result<Option<ApiKey>, StoreError> {
    let key = sqlx::query_as::<_, ApiKey>(
        "select id, tenant_id, prefix, key_hash, created_at, revoked_at
         from api_keys
         where prefix = $1 and revoked_at is null",
    )
    .bind(prefix)
    .fetch_optional(pool)
    .await?;

    Ok(key)
}

pub async fn revoke(pool: &sqlx::PgPool, tenant_id: Uuid, id: Uuid) -> Result<(), StoreError> {
    let result = sqlx::query(
        "update api_keys set revoked_at = now()
         where id = $1 and tenant_id = $2 and revoked_at is null",
    )
    .bind(id)
    .bind(tenant_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(StoreError::NotFound);
    }

    Ok(())
}
