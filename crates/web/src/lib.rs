pub mod auth;
pub mod error;
pub mod observability;
pub mod pages;
pub mod rate_limit;
pub mod routes;
pub mod session;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, patch, post};

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub allow_http_targets: bool,
    pub limiter: rate_limit::RateLimiter,
    pub session_secret: Vec<u8>,
    pub metrics: Option<metrics_exporter_prometheus::PrometheusHandle>,
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

    let public = Router::new()
        .route("/healthz", get(observability::healthz))
        .route("/readyz", get(observability::readyz))
        .route("/metrics", get(observability::metrics));

    let pages = Router::new()
        .route("/", get(pages::overview))
        .route("/login", get(pages::login_form).post(pages::login))
        .route("/logout", post(pages::logout))
        .route(
            "/endpoints",
            get(pages::endpoints).post(pages::create_endpoint),
        )
        .route("/endpoints/{id}/toggle", post(pages::toggle_endpoint))
        .route("/endpoints/{id}/rotate", post(pages::rotate_endpoint))
        .route("/events", get(pages::events))
        .route("/events/{id}", get(pages::event_detail))
        .route("/deliveries", get(pages::deliveries))
        .route("/deliveries/{id}", get(pages::delivery_detail))
        .route("/deliveries/{id}/replay", post(pages::replay_redirect))
        .route("/dead", get(pages::dead))
        .route(
            "/partials/deliveries/{id}/replay",
            post(pages::replay_partial),
        )
        .route("/static/htmx.min.js", get(pages::htmx));

    Router::new()
        .merge(public)
        .merge(api)
        .merge(pages)
        .layer(DefaultBodyLimit::max(256 * 1024))
        .with_state(state)
}
