use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::StoreError;
use crate::models::Endpoint;

pub const VALID_STATUSES: [&str; 3] = ["active", "paused", "disabled"];
pub const VALID_ORDERING_MODES: [&str; 2] = ["endpoint", "none"];

pub struct NewEndpoint<'a> {
    pub tenant_id: Uuid,
    pub url: &'a str,
    pub description: Option<&'a str>,
    pub secret: &'a [u8],
    pub event_types: &'a [String],
    pub ordering_mode: &'a str,
}

pub async fn create(pool: &sqlx::PgPool, new: NewEndpoint<'_>) -> Result<Endpoint, StoreError> {
    if !VALID_ORDERING_MODES.contains(&new.ordering_mode) {
        return Err(StoreError::Conflict("invalid ordering mode"));
    }

    let endpoint = sqlx::query_as::<_, Endpoint>(
        "insert into endpoints
            (id, tenant_id, url, description, secret_current, event_types, ordering_mode)
         values ($1, $2, $3, $4, $5, $6, $7)
         returning id, tenant_id, url, description, secret_current, secret_previous,
                   secret_rotated_at, event_types, status, max_attempts, rate_limit_per_sec,
                   max_concurrency, ordering_mode, created_at",
    )
    .bind(Uuid::now_v7())
    .bind(new.tenant_id)
    .bind(new.url)
    .bind(new.description)
    .bind(new.secret)
    .bind(new.event_types)
    .bind(new.ordering_mode)
    .fetch_one(pool)
    .await?;

    Ok(endpoint)
}

pub async fn list_for_tenant(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
) -> Result<Vec<Endpoint>, StoreError> {
    let endpoints = sqlx::query_as::<_, Endpoint>(
        "select id, tenant_id, url, description, secret_current, secret_previous,
                secret_rotated_at, event_types, status, max_attempts, rate_limit_per_sec,
                max_concurrency, ordering_mode, created_at
         from endpoints
         where tenant_id = $1
         order by created_at",
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;

    Ok(endpoints)
}

pub async fn find(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> Result<Option<Endpoint>, StoreError> {
    let endpoint = sqlx::query_as::<_, Endpoint>(
        "select id, tenant_id, url, description, secret_current, secret_previous,
                secret_rotated_at, event_types, status, max_attempts, rate_limit_per_sec,
                max_concurrency, ordering_mode, created_at
         from endpoints
         where tenant_id = $1 and id = $2",
    )
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(endpoint)
}

pub async fn set_status(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    id: Uuid,
    status: &str,
) -> Result<Endpoint, StoreError> {
    if !VALID_STATUSES.contains(&status) {
        return Err(StoreError::Conflict("invalid endpoint status"));
    }

    let endpoint = sqlx::query_as::<_, Endpoint>(
        "update endpoints set status = $3
         where tenant_id = $1 and id = $2
         returning id, tenant_id, url, description, secret_current, secret_previous,
                   secret_rotated_at, event_types, status, max_attempts, rate_limit_per_sec,
                   max_concurrency, ordering_mode, created_at",
    )
    .bind(tenant_id)
    .bind(id)
    .bind(status)
    .fetch_optional(pool)
    .await?
    .ok_or(StoreError::NotFound)?;

    Ok(endpoint)
}

pub async fn rotate_secret(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    id: Uuid,
    new_secret: &[u8],
    now: DateTime<Utc>,
) -> Result<Endpoint, StoreError> {
    let endpoint = sqlx::query_as::<_, Endpoint>(
        "update endpoints
         set secret_previous = secret_current,
             secret_current = $3,
             secret_rotated_at = $4
         where tenant_id = $1 and id = $2
         returning id, tenant_id, url, description, secret_current, secret_previous,
                   secret_rotated_at, event_types, status, max_attempts, rate_limit_per_sec,
                   max_concurrency, ordering_mode, created_at",
    )
    .bind(tenant_id)
    .bind(id)
    .bind(new_secret)
    .bind(now)
    .fetch_optional(pool)
    .await?
    .ok_or(StoreError::NotFound)?;

    Ok(endpoint)
}
