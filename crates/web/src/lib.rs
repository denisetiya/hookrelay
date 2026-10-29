pub mod auth;
pub mod error;
pub mod routes;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, patch, post};

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub allow_http_targets: bool,
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/v1/events", post(routes::events::create))
        .route("/v1/events/{id}", get(routes::events::show))
        .route(
            "/v1/endpoints",
            get(routes::endpoints::list).post(routes::endpoints::create),
        )
        .route("/v1/endpoints/{id}", patch(routes::endpoints::update))
        .route(
            "/v1/endpoints/{id}/rotate-secret",
            post(routes::endpoints::rotate_secret),
        )
        .route("/v1/deliveries", get(routes::deliveries::index))
        .route("/v1/deliveries/{id}", get(routes::deliveries::show))
        .route(
            "/v1/deliveries/{id}/replay",
            post(routes::deliveries::replay_delivery),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_api_key,
        ));

    Router::new()
        .merge(api)
        .layer(DefaultBodyLimit::max(256 * 1024))
        .with_state(state)
}
