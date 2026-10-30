mod support;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hookrelay_web::AppState;
use hookrelay_web::rate_limit::RateLimiter;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

#[tokio::test]
async fn readiness_fails_when_database_is_down() {
    let pool = PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(200))
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/postgres")
        .unwrap();
    let app = hookrelay_web::router(AppState {
        pool,
        allow_http_targets: false,
        limiter: RateLimiter::disabled(),
        session_secret: b"0123456789abcdef0123456789abcdef".to_vec(),
        metrics: None,
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8_lossy(&body);

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("database"));
}

#[tokio::test]
async fn metrics_endpoint_exposes_known_metric_names() {
    let handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .unwrap();
    metrics::counter!("hookrelay_deliveries_total", "status" => "succeeded").increment(1);

    let db = support::test_db().await;
    let app = support::app_with_metrics(db.pool.clone(), handle);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8_lossy(&body);

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("hookrelay_deliveries_total"));
}
