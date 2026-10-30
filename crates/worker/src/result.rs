use chrono::{DateTime, Utc};
use hookrelay_core::backoff::BackoffPolicy;
use hookrelay_core::outcome::next_delay;
use hookrelay_sender::{SendOutcome, SendReport};
use uuid::Uuid;

use crate::claim::ClaimedDelivery;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalStatus {
    Succeeded,
    Retrying,
    Dead,
}

/// Persists the outcome of one attempt and the attempt log row in a single
/// transaction, so a crash never leaves a delivery without its history.
pub async fn record_result(
    pool: &sqlx::PgPool,
    claimed: &ClaimedDelivery,
    report: &SendReport,
    backoff: &BackoffPolicy,
    sample: f32,
    started_at: DateTime<Utc>,
) -> Result<FinalStatus, sqlx::Error> {
    let (status, status_code, error, excerpt, next_attempt_at) = match &report.outcome {
        SendOutcome::Delivered { status, excerpt } => (
            "succeeded",
            Some(*status as i16),
            None,
            Some(excerpt.clone()),
            None,
        ),
        SendOutcome::Permanent { status, error } => (
            "dead",
            status.map(|value| value as i16),
            Some(error.clone()),
            None,
            None,
        ),
        SendOutcome::Retryable {
            status,
            error,
            retry_after,
        } => {
            let attempt = u8::try_from(claimed.attempt).unwrap_or(u8::MAX);
            let mut policy = *backoff;
            policy.max_attempts = u8::try_from(claimed.max_attempts).unwrap_or(u8::MAX);

            match next_delay(attempt, *retry_after, &policy, sample) {
                Some(delay) => (
                    "retrying",
                    status.map(|value| value as i16),
                    Some(error.clone()),
                    None,
                    Some(delay),
                ),
                None => (
                    "dead",
                    status.map(|value| value as i16),
                    Some(error.clone()),
                    None,
                    None,
                ),
            }
        }
    };

    let mut tx = pool.begin().await?;

    match next_attempt_at {
        Some(delay) => {
            sqlx::query(
                "update deliveries
                 set status = $2,
                     next_attempt_at = now() + ($3::bigint * interval '1 millisecond'),
                     last_status_code = $4,
                     last_error = $5,
                     response_excerpt = $6,
                     completed_at = null,
                     lease_owner = null,
                     lease_until = null
                 where id = $1",
            )
            .bind(claimed.delivery_id)
            .bind(status)
            .bind(delay.as_millis() as i64)
            .bind(status_code)
            .bind(&error)
            .bind(&excerpt)
            .execute(&mut *tx)
            .await?;
        }
        None => {
            sqlx::query(
                "update deliveries
                 set status = $2,
                     last_status_code = $3,
                     last_error = $4,
                     response_excerpt = $5,
                     completed_at = now(),
                     lease_owner = null,
                     lease_until = null
                 where id = $1",
            )
            .bind(claimed.delivery_id)
            .bind(status)
            .bind(status_code)
            .bind(&error)
            .bind(&excerpt)
            .execute(&mut *tx)
            .await?;
        }
    }

    sqlx::query(
        "insert into delivery_attempts
            (delivery_id, attempt_no, started_at, finished_at, status_code, latency_ms, error, response_excerpt)
         values ($1, $2, $3, now(), $4, $5, $6, $7)",
    )
    .bind(claimed.delivery_id)
    .bind(claimed.attempt)
    .bind(started_at)
    .bind(status_code)
    .bind(report.latency.as_millis() as i32)
    .bind(&error)
    .bind(&excerpt)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    metrics::counter!("hookrelay_deliveries_total", "status" => status.to_owned()).increment(1);
    metrics::histogram!("hookrelay_delivery_latency_seconds").record(report.latency.as_secs_f64());

    Ok(match status {
        "succeeded" => FinalStatus::Succeeded,
        "dead" => FinalStatus::Dead,
        _ => FinalStatus::Retrying,
    })
}

/// Returns expired in-flight deliveries to the queue. This is the crash safety
/// net: a worker killed mid-delivery must not strand the row.
pub async fn reap_expired(pool: &sqlx::PgPool) -> Result<u64, sqlx::Error> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "update deliveries
         set status = 'retrying',
             next_attempt_at = now(),
             lease_owner = null,
             lease_until = null
         where status = 'in_flight' and lease_until < now()
         returning id",
    )
    .fetch_all(pool)
    .await?;

    Ok(ids.len() as u64)
}

pub async fn release_leases(pool: &sqlx::PgPool, worker_id: &str) -> Result<u64, sqlx::Error> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "update deliveries
         set status = 'retrying',
             next_attempt_at = now(),
             lease_owner = null,
             lease_until = null
         where lease_owner = $1 and status = 'in_flight'
         returning id",
    )
    .bind(worker_id)
    .fetch_all(pool)
    .await?;

    Ok(ids.len() as u64)
}
