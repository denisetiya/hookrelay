use std::time::Duration;

use serde_json::Value;
use uuid::Uuid;

pub struct ClaimedDelivery {
    pub delivery_id: Uuid,
    pub event_id: Uuid,
    pub endpoint_id: Uuid,
    pub attempt: i16,
    pub max_attempts: i16,
    pub url: String,
    pub secret_current: Vec<u8>,
    pub secret_previous: Option<Vec<u8>>,
    pub secret_rotated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub event_type: String,
    pub payload: Value,
    pub ordering_mode: String,
}

#[derive(sqlx::FromRow)]
struct ClaimedRow {
    delivery_id: Uuid,
    event_id: Uuid,
    endpoint_id: Uuid,
    attempt: i16,
    max_attempts: i16,
    url: String,
    secret_current: Vec<u8>,
    secret_previous: Option<Vec<u8>>,
    secret_rotated_at: Option<chrono::DateTime<chrono::Utc>>,
    #[sqlx(rename = "event_type")]
    event_type: String,
    payload: Value,
    ordering_mode: String,
}

impl From<ClaimedRow> for ClaimedDelivery {
    fn from(row: ClaimedRow) -> Self {
        Self {
            delivery_id: row.delivery_id,
            event_id: row.event_id,
            endpoint_id: row.endpoint_id,
            attempt: row.attempt,
            max_attempts: row.max_attempts,
            url: row.url,
            secret_current: row.secret_current,
            secret_previous: row.secret_previous,
            secret_rotated_at: row.secret_rotated_at,
            event_type: row.event_type,
            payload: row.payload,
            ordering_mode: row.ordering_mode,
        }
    }
}

// One delivery per ordered endpoint per batch. The advisory lock is transaction
// scoped, so another worker cannot pick a second delivery for the same endpoint
// while this claim is in flight.
const CLAIM_ORDERED_SQL: &str = r#"
select id
from (
    select distinct on (d.endpoint_id) d.id, d.next_attempt_at
    from deliveries d
    join endpoints e on e.id = d.endpoint_id
    where d.status in ('pending', 'retrying')
      and d.next_attempt_at <= now()
      and e.status = 'active'
      and e.ordering_mode = 'endpoint'
      and pg_try_advisory_xact_lock(hashtextextended(d.endpoint_id::text, 0))
      and not exists (
          select 1
          from deliveries x
          where x.endpoint_id = d.endpoint_id
            and x.status = 'in_flight'
      )
    order by d.endpoint_id, d.next_attempt_at, d.sequence
) picked
order by picked.next_attempt_at
limit $1
"#;

const CLAIM_UNORDERED_SQL: &str = r#"
select d.id
from deliveries d
join endpoints e on e.id = d.endpoint_id
where d.status in ('pending', 'retrying')
  and d.next_attempt_at <= now()
  and e.status = 'active'
  and e.ordering_mode = 'none'
order by d.next_attempt_at, d.sequence
for update of d skip locked
limit $1
"#;

const MARK_CLAIMED_SQL: &str = r#"
update deliveries
set status = 'in_flight',
    lease_owner = $1,
    lease_until = now() + ($2::bigint * interval '1 millisecond'),
    attempt = attempt + 1
where id = any ($3)
returning id
"#;

const LOAD_SQL: &str = r#"
select d.id as delivery_id,
       d.event_id,
       d.endpoint_id,
       d.attempt,
       e.max_attempts,
       e.url,
       e.secret_current,
       e.secret_previous,
       e.secret_rotated_at,
       ev.type as event_type,
       ev.payload,
       e.ordering_mode
from deliveries d
join endpoints e on e.id = d.endpoint_id
join events ev on ev.id = d.event_id
where d.id = any ($1)
order by d.sequence
"#;

pub async fn claim(
    pool: &sqlx::PgPool,
    worker_id: &str,
    batch: i64,
    lease: Duration,
) -> Result<Vec<ClaimedDelivery>, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let mut ids: Vec<Uuid> = sqlx::query_scalar(CLAIM_ORDERED_SQL)
        .bind(batch)
        .fetch_all(&mut *tx)
        .await?;

    let remaining = batch - ids.len() as i64;
    if remaining > 0 {
        let unordered: Vec<Uuid> = sqlx::query_scalar(CLAIM_UNORDERED_SQL)
            .bind(remaining)
            .fetch_all(&mut *tx)
            .await?;

        ids.extend(unordered);
    }

    if ids.is_empty() {
        tx.commit().await?;
        return Ok(Vec::new());
    }

    sqlx::query(MARK_CLAIMED_SQL)
        .bind(worker_id)
        .bind(lease.as_millis() as i64)
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await?;

    let rows = sqlx::query_as::<_, ClaimedRow>(LOAD_SQL)
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(rows.into_iter().map(Into::into).collect())
}
