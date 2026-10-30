use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::StoreError;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeliveryRow {
    pub id: Uuid,
    pub event_id: Uuid,
    #[sqlx(rename = "event_type")]
    pub event_type: String,
    pub endpoint_id: Uuid,
    pub endpoint_url: String,
    pub status: String,
    pub attempt: i16,
    pub next_attempt_at: DateTime<Utc>,
    pub last_status_code: Option<i16>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AttemptRow {
    pub attempt_no: i16,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status_code: Option<i16>,
    pub latency_ms: Option<i32>,
    pub error: Option<String>,
    pub response_excerpt: Option<String>,
}

pub struct DeliveryDetail {
    pub delivery: DeliveryRow,
    pub attempts: Vec<AttemptRow>,
    pub payload: serde_json::Value,
}

pub struct DeliveryFilter {
    pub tenant_id: Uuid,
    pub status: Option<String>,
    pub endpoint_id: Option<Uuid>,
    pub limit: i64,
}

const SELECT_ROW: &str = r#"
select d.id,
       d.event_id,
       ev.type as event_type,
       d.endpoint_id,
       e.url as endpoint_url,
       d.status,
       d.attempt,
       d.next_attempt_at,
       d.last_status_code,
       d.last_error,
       d.created_at,
       d.completed_at
from deliveries d
join endpoints e on e.id = d.endpoint_id
join events ev on ev.id = d.event_id
"#;

pub async fn list(
    pool: &sqlx::PgPool,
    filter: DeliveryFilter,
) -> Result<Vec<DeliveryRow>, StoreError> {
    let sql = format!(
        "{SELECT_ROW}
         where e.tenant_id = $1
           and ($2::text is null or d.status = $2)
           and ($3::uuid is null or d.endpoint_id = $3)
         order by d.created_at desc, d.sequence desc
         limit $4"
    );

    let rows = sqlx::query_as::<_, DeliveryRow>(&sql)
        .bind(filter.tenant_id)
        .bind(filter.status)
        .bind(filter.endpoint_id)
        .bind(filter.limit)
        .fetch_all(pool)
        .await?;

    Ok(rows)
}

pub async fn find(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> Result<Option<DeliveryDetail>, StoreError> {
    let sql = format!(
        "{SELECT_ROW}
         where e.tenant_id = $1 and d.id = $2"
    );

    let Some(delivery) = sqlx::query_as::<_, DeliveryRow>(&sql)
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(None);
    };

    let attempts = sqlx::query_as::<_, AttemptRow>(
        "select attempt_no, started_at, finished_at, status_code, latency_ms, error, response_excerpt
         from delivery_attempts
         where delivery_id = $1
         order by attempt_no",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    let payload: serde_json::Value = sqlx::query_scalar("select payload from events where id = $1")
        .bind(delivery.event_id)
        .fetch_one(pool)
        .await?;

    Ok(Some(DeliveryDetail {
        delivery,
        attempts,
        payload,
    }))
}

/// Replays a finished delivery by inserting a fresh pending row that points at
/// the original, so the audit trail stays intact.
pub async fn replay(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    id: Uuid,
) -> Result<DeliveryRow, StoreError> {
    let new_id = Uuid::now_v7();

    let inserted: Option<Uuid> = sqlx::query_scalar(
        "insert into deliveries (id, event_id, endpoint_id, status, attempt, next_attempt_at, replay_of)
         select $1, d.event_id, d.endpoint_id, 'pending', 0, now(), d.id
         from deliveries d
         join endpoints e on e.id = d.endpoint_id
         where d.id = $3 and e.tenant_id = $2 and d.status in ('succeeded', 'dead')
         returning id",
    )
    .bind(new_id)
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(pool)
    .await?;

    if inserted.is_none() {
        let source = find(pool, tenant_id, id).await?;

        return Err(match source {
            Some(_) => StoreError::Conflict("delivery is not finished"),
            None => StoreError::NotFound,
        });
    }

    let sql = format!("{SELECT_ROW} where e.tenant_id = $1 and d.id = $2");
    let row = sqlx::query_as::<_, DeliveryRow>(&sql)
        .bind(tenant_id)
        .bind(new_id)
        .fetch_one(pool)
        .await?;

    Ok(row)
}

#[derive(Debug, Clone, Copy, sqlx::FromRow)]
pub struct DeliveryCounts {
    pub total: i64,
    pub dead: i64,
    pub retrying: i64,
}

pub async fn counts(pool: &sqlx::PgPool, tenant_id: Uuid) -> Result<DeliveryCounts, StoreError> {
    let counts = sqlx::query_as::<_, DeliveryCounts>(
        "select count(*) as total,
                count(*) filter (where d.status = 'dead') as dead,
                count(*) filter (where d.status = 'retrying') as retrying
         from deliveries d
         join endpoints e on e.id = d.endpoint_id
         where e.tenant_id = $1",
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await?;

    Ok(counts)
}
