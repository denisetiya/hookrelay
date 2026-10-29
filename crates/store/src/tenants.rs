use uuid::Uuid;

use crate::error::StoreError;
use crate::models::Tenant;

pub async fn create(pool: &sqlx::PgPool, name: &str) -> Result<Tenant, StoreError> {
    let tenant = sqlx::query_as::<_, Tenant>(
        "insert into tenants (id, name) values ($1, $2)
         returning id, name, created_at",
    )
    .bind(Uuid::now_v7())
    .bind(name)
    .fetch_one(pool)
    .await?;

    Ok(tenant)
}
