#![allow(clippy::result_large_err)]

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_TYPE, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::Deserialize;
use uuid::Uuid;

use hookrelay_store::deliveries::{DeliveryFilter, DeliveryRow};
use hookrelay_store::endpoints::NewEndpoint;

use crate::AppState;
use crate::auth;
use crate::session::{self, Session};

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct DeliveryView {
    pub id: String,
    pub event_type: String,
    pub endpoint_url: String,
    pub status: String,
    pub attempt: i16,
    pub last_status_code: Option<i16>,
    pub created_at: String,
}

impl From<DeliveryRow> for DeliveryView {
    fn from(row: DeliveryRow) -> Self {
        Self {
            id: row.id.to_string(),
            event_type: row.event_type,
            endpoint_url: row.endpoint_url,
            status: row.status,
            attempt: row.attempt,
            last_status_code: row.last_status_code,
            created_at: format_time(row.created_at),
        }
    }
}

pub struct EndpointView {
    pub id: String,
    pub url: String,
    pub status: String,
    pub ordering_mode: String,
    pub event_types: String,
}

pub struct EventView {
    pub id: String,
    pub event_type: String,
    pub created_at: String,
}

pub struct AttemptView {
    pub attempt_no: i16,
    pub status_code: Option<i16>,
    pub latency_ms: Option<i32>,
    pub error: Option<String>,
    pub started_at: String,
}

pub struct CountsView {
    pub total: i64,
    pub dead: i64,
    pub retrying: i64,
}

fn format_time(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage {
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "overview.html")]
struct OverviewPage {
    counts: CountsView,
    recent: Vec<DeliveryView>,
}

#[derive(Template)]
#[template(path = "endpoints.html")]
struct EndpointsPage {
    endpoints: Vec<EndpointView>,
    secret: Option<String>,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "events.html")]
struct EventsPage {
    events: Vec<EventView>,
}

#[derive(Template)]
#[template(path = "event_detail.html")]
struct EventDetailPage {
    event: EventView,
    payload: String,
}

#[derive(Template)]
#[template(path = "deliveries.html")]
struct DeliveriesPage {
    deliveries: Vec<DeliveryView>,
    status: String,
}

#[derive(Template)]
#[template(path = "delivery_detail.html")]
struct DeliveryDetailPage {
    delivery: DeliveryView,
    attempts: Vec<AttemptView>,
    payload: String,
}

#[derive(Template)]
#[template(path = "partials/delivery_row.html")]
struct DeliveryRowTemplate {
    row: DeliveryView,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn render(template: impl Template) -> Result<Html<String>, Response> {
    template.render().map(Html).map_err(|error| {
        tracing::error!(error = %error, "template render failed");
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    })
}

fn session_of(state: &AppState, headers: &HeaderMap) -> Result<Session, Response> {
    session::from_headers(&state.session_secret, headers)
        .ok_or_else(|| Redirect::to("/login").into_response())
}

fn store_error(error: impl std::fmt::Display) -> Response {
    tracing::error!(error = %error, "dashboard query failed");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn endpoint_views(endpoints: Vec<hookrelay_store::models::Endpoint>) -> Vec<EndpointView> {
    endpoints
        .into_iter()
        .map(|endpoint| EndpointView {
            id: endpoint.id.to_string(),
            url: endpoint.url,
            status: endpoint.status,
            ordering_mode: endpoint.ordering_mode,
            event_types: if endpoint.event_types.is_empty() {
                "all events".to_owned()
            } else {
                endpoint.event_types.join(", ")
            },
        })
        .collect()
}

async fn endpoints_page(
    state: &AppState,
    tenant_id: Uuid,
    secret: Option<String>,
    error: Option<String>,
) -> Result<Html<String>, Response> {
    let endpoints = hookrelay_store::endpoints::list_for_tenant(&state.pool, tenant_id)
        .await
        .map_err(store_error)?;

    render(EndpointsPage {
        endpoints: endpoint_views(endpoints),
        secret,
        error,
    })
}

fn random_secret() -> Vec<u8> {
    let mut secret = vec![0_u8; 32];
    rand::thread_rng().fill_bytes(&mut secret);

    secret
}

// ---------------------------------------------------------------------------
// Auth pages
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginForm {
    pub api_key: String,
}

pub async fn login_form() -> Result<Html<String>, Response> {
    render(LoginPage { error: None })
}

pub async fn login(State(state): State<AppState>, Form(form): Form<LoginForm>) -> Response {
    let Some((prefix, secret)) = auth::parse_key(form.api_key.trim()) else {
        return login_error("Invalid API key");
    };

    let key = match hookrelay_store::api_keys::find_active_by_prefix(&state.pool, prefix).await {
        Ok(Some(key)) => key,
        Ok(None) => return login_error("Invalid API key"),
        Err(error) => return store_error(error),
    };

    let hash = key.key_hash.clone();
    let secret = secret.to_owned();
    let valid = tokio::task::spawn_blocking(move || auth::verify(&hash, &secret)).await;

    if !matches!(valid, Ok(true)) {
        return login_error("Invalid API key");
    }

    let value = session::issue(&state.session_secret, key.tenant_id, &key.prefix);
    let cookie = session::cookie(&value);

    let mut response = Redirect::to("/").into_response();
    if let Ok(header) = cookie.parse() {
        response.headers_mut().insert(SET_COOKIE, header);
    }

    response
}

pub async fn logout() -> Response {
    let mut response = Redirect::to("/login").into_response();
    if let Ok(header) = session::cleared_cookie().parse() {
        response.headers_mut().insert(SET_COOKIE, header);
    }

    response
}

fn login_error(message: &str) -> Response {
    match render(LoginPage {
        error: Some(message.to_owned()),
    }) {
        Ok(html) => (StatusCode::UNAUTHORIZED, html).into_response(),
        Err(response) => response,
    }
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

pub async fn overview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, Response> {
    let session = session_of(&state, &headers)?;
    let counts = hookrelay_store::deliveries::counts(&state.pool, session.tenant_id)
        .await
        .map_err(store_error)?;
    let recent = hookrelay_store::deliveries::list(
        &state.pool,
        DeliveryFilter {
            tenant_id: session.tenant_id,
            status: None,
            endpoint_id: None,
            limit: 10,
        },
    )
    .await
    .map_err(store_error)?;

    render(OverviewPage {
        counts: CountsView {
            total: counts.total,
            dead: counts.dead,
            retrying: counts.retrying,
        },
        recent: recent.into_iter().map(Into::into).collect(),
    })
}

pub async fn endpoints(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, Response> {
    let session = session_of(&state, &headers)?;

    endpoints_page(&state, session.tenant_id, None, None).await
}

#[derive(Deserialize)]
pub struct CreateEndpointForm {
    pub url: String,
    #[serde(default = "default_ordering")]
    pub ordering_mode: String,
}

fn default_ordering() -> String {
    "endpoint".to_owned()
}

pub async fn create_endpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CreateEndpointForm>,
) -> Result<Html<String>, Response> {
    let session = session_of(&state, &headers)?;
    let url = form.url.trim();

    if !url.starts_with("https://") && !(state.allow_http_targets && url.starts_with("http://")) {
        return endpoints_page(
            &state,
            session.tenant_id,
            None,
            Some("URL must use https".to_owned()),
        )
        .await;
    }

    let secret = random_secret();
    let created = hookrelay_store::endpoints::create(
        &state.pool,
        NewEndpoint {
            tenant_id: session.tenant_id,
            url,
            description: None,
            secret: &secret,
            event_types: &[],
            ordering_mode: &form.ordering_mode,
        },
    )
    .await;

    match created {
        Ok(_) => endpoints_page(&state, session.tenant_id, Some(hex::encode(secret)), None).await,
        Err(error) => {
            endpoints_page(&state, session.tenant_id, None, Some(error.to_string())).await
        }
    }
}

pub async fn toggle_endpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let session = match session_of(&state, &headers) {
        Ok(session) => session,
        Err(response) => return response,
    };

    let endpoint = match hookrelay_store::endpoints::find(&state.pool, session.tenant_id, id).await
    {
        Ok(Some(endpoint)) => endpoint,
        Ok(None) => return not_found(),
        Err(error) => return store_error(error),
    };

    let next = if endpoint.status == "active" {
        "paused"
    } else {
        "active"
    };

    match hookrelay_store::endpoints::set_status(&state.pool, session.tenant_id, id, next).await {
        Ok(_) => Redirect::to("/endpoints").into_response(),
        Err(error) => store_error(error),
    }
}

pub async fn rotate_endpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Html<String>, Response> {
    let session = session_of(&state, &headers)?;
    let secret = random_secret();

    hookrelay_store::endpoints::rotate_secret(
        &state.pool,
        session.tenant_id,
        id,
        &secret,
        Utc::now(),
    )
    .await
    .map_err(store_error)?;

    endpoints_page(&state, session.tenant_id, Some(hex::encode(secret)), None).await
}

pub async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, Response> {
    let session = session_of(&state, &headers)?;
    let events = hookrelay_store::events::list(&state.pool, session.tenant_id, 50)
        .await
        .map_err(store_error)?;

    render(EventsPage {
        events: events
            .into_iter()
            .map(|event| EventView {
                id: event.id.to_string(),
                event_type: event.event_type,
                created_at: format_time(event.created_at),
            })
            .collect(),
    })
}

pub async fn event_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Html<String>, Response> {
    let session = session_of(&state, &headers)?;
    let event = hookrelay_store::events::find(&state.pool, session.tenant_id, id)
        .await
        .map_err(store_error)?
        .ok_or_else(not_found)?;
    let payload = serde_json::to_string_pretty(&event.payload).unwrap_or_default();

    render(EventDetailPage {
        event: EventView {
            id: event.id.to_string(),
            event_type: event.event_type,
            created_at: format_time(event.created_at),
        },
        payload,
    })
}

#[derive(Deserialize)]
pub struct DeliveryQuery {
    pub status: Option<String>,
}

pub async fn deliveries(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DeliveryQuery>,
) -> Result<Html<String>, Response> {
    let session = session_of(&state, &headers)?;
    let status = query.status.unwrap_or_default();
    let deliveries = hookrelay_store::deliveries::list(
        &state.pool,
        DeliveryFilter {
            tenant_id: session.tenant_id,
            status: if status.is_empty() {
                None
            } else {
                Some(status.clone())
            },
            endpoint_id: None,
            limit: 50,
        },
    )
    .await
    .map_err(store_error)?;

    render(DeliveriesPage {
        deliveries: deliveries.into_iter().map(Into::into).collect(),
        status,
    })
}

pub async fn delivery_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Html<String>, Response> {
    let session = session_of(&state, &headers)?;
    let detail = hookrelay_store::deliveries::find(&state.pool, session.tenant_id, id)
        .await
        .map_err(store_error)?
        .ok_or_else(not_found)?;
    let payload = serde_json::to_string_pretty(&detail.payload).unwrap_or_default();

    render(DeliveryDetailPage {
        delivery: detail.delivery.into(),
        attempts: detail
            .attempts
            .into_iter()
            .map(|attempt| AttemptView {
                attempt_no: attempt.attempt_no,
                status_code: attempt.status_code,
                latency_ms: attempt.latency_ms,
                error: attempt.error,
                started_at: format_time(attempt.started_at),
            })
            .collect(),
        payload,
    })
}

pub async fn dead(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, Response> {
    let session = session_of(&state, &headers)?;
    let deliveries = hookrelay_store::deliveries::list(
        &state.pool,
        DeliveryFilter {
            tenant_id: session.tenant_id,
            status: Some("dead".to_owned()),
            endpoint_id: None,
            limit: 100,
        },
    )
    .await
    .map_err(store_error)?;

    render(DeliveriesPage {
        deliveries: deliveries.into_iter().map(Into::into).collect(),
        status: "dead".to_owned(),
    })
}

pub async fn replay_partial(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Html<String>, Response> {
    let session = session_of(&state, &headers)?;

    let replayed = hookrelay_store::deliveries::replay(&state.pool, session.tenant_id, id)
        .await
        .map_err(|error| (StatusCode::CONFLICT, error.to_string()).into_response())?;

    render(DeliveryRowTemplate {
        row: replayed.into(),
    })
}

pub async fn htmx() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "application/javascript")],
        include_bytes!("../static/htmx.min.js"),
    )
}
