use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::error::StoreError;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Event {
    pub id: Uuid,
    pub tenant_id: Uuid,
    #[sqlx(rename = "type")]
    pub event_type: String,
    pub payload: Value,
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct IngestEvent<'a> {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event_type: &'a str,
    pub payload: &'a Value,
    pub idempotency_key: Option<&'a str>,
}

#[derive(Debug)]
pub struct Ingested {
    pub event: Event,
    pub created: bool,
    pub delivery_ids: Vec<Uuid>,
}

/// Writes the event and its deliveries in one transaction. A duplicate
/// idempotency key returns the original event without fanning out again.
pub async fn ingest(pool: &sqlx::PgPool, input: IngestEvent<'_>) -> Result<Ingested, StoreError> {
    let mut tx = pool.begin().await?;

    let inserted = sqlx::query_as::<_, Event>(
        "insert into events (id, tenant_id, type, payload, idempotency_key)
         values ($1, $2, $3, $4, $5)
         on conflict (tenant_id, idempotency_key) do nothing
         returning id, tenant_id, type, payload, idempotency_key, created_at",
    )
    .bind(input.id)
    .bind(input.tenant_id)
    .bind(input.event_type)
    .bind(input.payload)
    .bind(input.idempotency_key)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(event) = inserted else {
        let existing = sqlx::query_as::<_, Event>(
            "select id, tenant_id, type, payload, idempotency_key, created_at
             from events
             where tenant_id = $1 and idempotency_key = $2",
        )
        .bind(input.tenant_id)
        .bind(input.idempotency_key)
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;

        return Ok(Ingested {
            event: existing,
            created: false,
            delivery_ids: Vec::new(),
        });
    };

    let endpoint_ids: Vec<Uuid> = sqlx::query_scalar(
        "select id
         from endpoints
         where tenant_id = $1
           and status = 'active'
           and (cardinality(event_types) = 0 or $2 = any (event_types))
         order by created_at",
    )
    .bind(input.tenant_id)
    .bind(input.event_type)
    .fetch_all(&mut *tx)
    .await?;

    let delivery_ids: Vec<Uuid> = endpoint_ids.iter().map(|_| Uuid::now_v7()).collect();

    if !endpoint_ids.is_empty() {
        sqlx::query(
            "insert into deliveries (id, event_id, endpoint_id, next_attempt_at)
             select unnest($1::uuid[]), $2, unnest($3::uuid[]), now()",
        )
        .bind(&delivery_ids)
        .bind(event.id)
        .bind(&endpoint_ids)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(Ingested {
        event,
        created: true,
        delivery_ids,
    })
}

pub async fn find(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> Result<Option<Event>, StoreError> {
    let event = sqlx::query_as::<_, Event>(
        "select id, tenant_id, type, payload, idempotency_key, created_at
         from events
         where tenant_id = $1 and id = $2",
    )
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(event)
}
