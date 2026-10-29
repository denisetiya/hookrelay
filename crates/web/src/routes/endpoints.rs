use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use hookrelay_store::endpoints::{NewEndpoint, VALID_ORDERING_MODES, VALID_STATUSES};
use hookrelay_store::models::Endpoint;

use crate::AppState;
use crate::auth::TenantId;
use crate::error::AppError;

#[derive(Serialize)]
pub struct EndpointResponse {
    pub id: Uuid,
    pub url: String,
    pub description: Option<String>,
    pub event_types: Vec<String>,
    pub status: String,
    pub ordering_mode: String,
    pub max_attempts: i16,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct CreatedEndpoint {
    #[serde(flatten)]
    pub endpoint: EndpointResponse,
    pub secret: String,
}

#[derive(Deserialize)]
pub struct CreateEndpoint {
    pub url: String,
    pub description: Option<String>,
    #[serde(default)]
    pub event_types: Vec<String>,
    #[serde(default = "default_ordering")]
    pub ordering_mode: String,
}

#[derive(Deserialize)]
pub struct UpdateEndpoint {
    pub status: String,
}

fn default_ordering() -> String {
    "endpoint".to_owned()
}

pub async fn list(
    State(state): State<AppState>,
    TenantId(tenant_id): TenantId,
) -> Result<Json<Vec<EndpointResponse>>, AppError> {
    let endpoints = hookrelay_store::endpoints::list_for_tenant(&state.pool, tenant_id).await?;

    Ok(Json(endpoints.into_iter().map(Into::into).collect()))
}

pub async fn create(
    State(state): State<AppState>,
    TenantId(tenant_id): TenantId,
    Json(body): Json<CreateEndpoint>,
) -> Result<(StatusCode, Json<CreatedEndpoint>), AppError> {
    validate_url(&body.url, state.allow_http_targets)?;

    if !VALID_ORDERING_MODES.contains(&body.ordering_mode.as_str()) {
        return Err(AppError::Validation(
            "ordering_mode must be endpoint or none".to_owned(),
        ));
    }

    let secret = random_secret();
    let endpoint = hookrelay_store::endpoints::create(
        &state.pool,
        NewEndpoint {
            tenant_id,
            url: body.url.trim(),
            description: body.description.as_deref(),
            secret: &secret,
            event_types: &body.event_types,
            ordering_mode: &body.ordering_mode,
        },
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(CreatedEndpoint {
            endpoint: endpoint.into(),
            secret: hex::encode(secret),
        }),
    ))
}

pub async fn update(
    State(state): State<AppState>,
    TenantId(tenant_id): TenantId,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateEndpoint>,
) -> Result<Json<EndpointResponse>, AppError> {
    if !VALID_STATUSES.contains(&body.status.as_str()) {
        return Err(AppError::Validation(
            "status must be active, paused, or disabled".to_owned(),
        ));
    }

    let endpoint =
        hookrelay_store::endpoints::set_status(&state.pool, tenant_id, id, &body.status).await?;

    Ok(Json(endpoint.into()))
}

pub async fn rotate_secret(
    State(state): State<AppState>,
    TenantId(tenant_id): TenantId,
    Path(id): Path<Uuid>,
) -> Result<Json<CreatedEndpoint>, AppError> {
    let secret = random_secret();
    let endpoint =
        hookrelay_store::endpoints::rotate_secret(&state.pool, tenant_id, id, &secret, Utc::now())
            .await?;

    Ok(Json(CreatedEndpoint {
        endpoint: endpoint.into(),
        secret: hex::encode(secret),
    }))
}

fn random_secret() -> Vec<u8> {
    let mut secret = vec![0_u8; 32];
    rand::thread_rng().fill_bytes(&mut secret);

    secret
}

fn validate_url(raw: &str, allow_http: bool) -> Result<(), AppError> {
    let url =
        Url::parse(raw).map_err(|error| AppError::Validation(format!("invalid url: {error}")))?;

    match url.scheme() {
        "https" => Ok(()),
        "http" if allow_http => Ok(()),
        _ => Err(AppError::Validation("url must use https".to_owned())),
    }
}

impl From<Endpoint> for EndpointResponse {
    fn from(endpoint: Endpoint) -> Self {
        Self {
            id: endpoint.id,
            url: endpoint.url,
            description: endpoint.description,
            event_types: endpoint.event_types,
            status: endpoint.status,
            ordering_mode: endpoint.ordering_mode,
            max_attempts: endpoint.max_attempts,
            created_at: endpoint.created_at,
        }
    }
}
