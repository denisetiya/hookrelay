use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use hookrelay_store::events::{IngestEvent, ingest};

use crate::AppState;
use crate::auth::TenantId;
use crate::error::AppError;

#[derive(Deserialize)]
pub struct CreateEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: serde_json::Value,
}

#[derive(Serialize)]
pub struct EventResponse {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub event_type: String,
    pub created_at: DateTime<Utc>,
}

pub async fn create(
    State(state): State<AppState>,
    TenantId(tenant_id): TenantId,
    headers: HeaderMap,
    Json(body): Json<CreateEvent>,
) -> Result<(StatusCode, Json<EventResponse>), AppError> {
    let event_type = body.event_type.trim();

    if event_type.is_empty() || event_type.len() > 200 {
        return Err(AppError::Validation(
            "type must be between 1 and 200 characters".to_owned(),
        ));
    }

    let idempotency_key = headers
        .get("idempotency-key")
        .map(|value| {
            value
                .to_str()
                .map_err(|_| AppError::Validation("idempotency key is not valid utf-8".to_owned()))
        })
        .transpose()?;

    if idempotency_key.is_some_and(|key| key.is_empty() || key.len() > 255) {
        return Err(AppError::Validation(
            "idempotency key must be between 1 and 255 characters".to_owned(),
        ));
    }

    let ingested = ingest(
        &state.pool,
        IngestEvent {
            id: Uuid::now_v7(),
            tenant_id,
            event_type,
            payload: &body.payload,
            idempotency_key,
        },
    )
    .await?;

    let status = if ingested.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };

    Ok((
        status,
        Json(EventResponse {
            id: ingested.event.id,
            event_type: ingested.event.event_type,
            created_at: ingested.event.created_at,
        }),
    ))
}

pub async fn show(
    State(state): State<AppState>,
    TenantId(tenant_id): TenantId,
    Path(id): Path<Uuid>,
) -> Result<Json<EventResponse>, AppError> {
    let event = hookrelay_store::events::find(&state.pool, tenant_id, id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(EventResponse {
        id: event.id,
        event_type: event.event_type,
        created_at: event.created_at,
    }))
}
