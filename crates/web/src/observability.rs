use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::AppState;

pub async fn healthz() -> StatusCode {
    StatusCode::OK
}

pub async fn readyz(State(state): State<AppState>) -> Response {
    if let Err(error) = sqlx::query_scalar::<_, i32>("select 1")
        .fetch_one(&state.pool)
        .await
    {
        tracing::warn!(error = %error, "readiness: database unavailable");

        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "unavailable", "dependency": "database"})),
        )
            .into_response();
    }

    if let Err(error) = state.limiter.ping().await {
        tracing::warn!(error = %error, "readiness: redis unavailable");

        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "unavailable", "dependency": "redis"})),
        )
            .into_response();
    }

    (StatusCode::OK, Json(json!({"status": "ok"}))).into_response()
}

pub async fn metrics(State(state): State<AppState>) -> Response {
    let Some(handle) = &state.metrics else {
        return (StatusCode::OK, "# metrics disabled\n").into_response();
    };

    (
        [(CONTENT_TYPE, "text/plain; version=0.0.4")],
        handle.render(),
    )
        .into_response()
}
