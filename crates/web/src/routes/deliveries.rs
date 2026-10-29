use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use hookrelay_store::deliveries::{
    AttemptRow, DeliveryDetail, DeliveryFilter, DeliveryRow, list, replay,
};

use crate::AppState;
use crate::auth::TenantId;
use crate::error::AppError;

#[derive(Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
    pub endpoint_id: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Serialize)]
pub struct DeliveryResponse {
    pub id: Uuid,
    pub event_id: Uuid,
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

#[derive(Serialize)]
pub struct AttemptResponse {
    pub attempt_no: i16,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status_code: Option<i16>,
    pub latency_ms: Option<i32>,
    pub error: Option<String>,
    pub response_excerpt: Option<String>,
}

#[derive(Serialize)]
pub struct DeliveryDetailResponse {
    pub delivery: DeliveryResponse,
    pub attempts: Vec<AttemptResponse>,
    pub payload: serde_json::Value,
}

pub async fn index(
    State(state): State<AppState>,
    TenantId(tenant_id): TenantId,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<DeliveryResponse>>, AppError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);

    let rows = list(
        &state.pool,
        DeliveryFilter {
            tenant_id,
            status: query.status,
            endpoint_id: query.endpoint_id,
            limit,
        },
    )
    .await?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

pub async fn show(
    State(state): State<AppState>,
    TenantId(tenant_id): TenantId,
    Path(id): Path<Uuid>,
) -> Result<Json<DeliveryDetailResponse>, AppError> {
    let detail = hookrelay_store::deliveries::find(&state.pool, tenant_id, id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(detail.into()))
}

pub async fn replay_delivery(
    State(state): State<AppState>,
    TenantId(tenant_id): TenantId,
    Path(id): Path<Uuid>,
) -> Result<Json<DeliveryResponse>, AppError> {
    let row = replay(&state.pool, tenant_id, id).await?;

    Ok(Json(row.into()))
}

impl From<DeliveryRow> for DeliveryResponse {
    fn from(row: DeliveryRow) -> Self {
        Self {
            id: row.id,
            event_id: row.event_id,
            event_type: row.event_type,
            endpoint_id: row.endpoint_id,
            endpoint_url: row.endpoint_url,
            status: row.status,
            attempt: row.attempt,
            next_attempt_at: row.next_attempt_at,
            last_status_code: row.last_status_code,
            last_error: row.last_error,
            created_at: row.created_at,
            completed_at: row.completed_at,
        }
    }
}

impl From<AttemptRow> for AttemptResponse {
    fn from(row: AttemptRow) -> Self {
        Self {
            attempt_no: row.attempt_no,
            started_at: row.started_at,
            finished_at: row.finished_at,
            status_code: row.status_code,
            latency_ms: row.latency_ms,
            error: row.error,
            response_excerpt: row.response_excerpt,
        }
    }
}

impl From<DeliveryDetail> for DeliveryDetailResponse {
    fn from(detail: DeliveryDetail) -> Self {
        Self {
            delivery: detail.delivery.into(),
            attempts: detail.attempts.into_iter().map(Into::into).collect(),
            payload: detail.payload,
        }
    }
}
