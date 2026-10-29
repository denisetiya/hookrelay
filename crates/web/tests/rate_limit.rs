mod support;

use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use hookrelay_web::rate_limit::{RateDecision, RateLimiter};
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn allows_requests_under_capacity() {
    let (connection, _container) = support::test_redis().await;
    let limiter = RateLimiter::new(connection, 2, 1.0);

    assert!(matches!(
        limiter.check("prefix-a").await.unwrap(),
        RateDecision::Allowed { remaining: 1 }
    ));
    assert!(matches!(
        limiter.check("prefix-a").await.unwrap(),
        RateDecision::Allowed { remaining: 0 }
    ));
    assert!(matches!(
        limiter.check("prefix-a").await.unwrap(),
        RateDecision::Denied { .. }
    ));
}

#[tokio::test]
async fn refills_over_time() {
    let (connection, _container) = support::test_redis().await;
    let limiter = RateLimiter::new(connection, 1, 2.0);

    limiter.check("prefix-b").await.unwrap();
    assert!(matches!(
        limiter.check("prefix-b").await.unwrap(),
        RateDecision::Denied { .. }
    ));

    tokio::time::sleep(Duration::from_millis(700)).await;

    assert!(matches!(
        limiter.check("prefix-b").await.unwrap(),
        RateDecision::Allowed { .. }
    ));
}

#[tokio::test]
async fn api_returns_429_with_retry_after() {
    let db = support::test_db().await;
    let (_, key) = support::seed_key(&db.pool).await;
    let (connection, _container) = support::test_redis().await;
    let limiter = RateLimiter::new(connection, 1, 0.5);
    let app = support::app_with_limiter(db.pool.clone(), limiter);

    let request = || {
        Request::builder()
            .method("POST")
            .uri("/v1/events")
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"type": "ping", "payload": {}}).to_string(),
            ))
            .unwrap()
    };

    let first = app.clone().oneshot(request()).await.unwrap();
    let second = app.oneshot(request()).await.unwrap();

    assert_eq!(first.status(), StatusCode::CREATED);
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(second.headers().contains_key(header::RETRY_AFTER));

    let body = to_bytes(second.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["code"], "rate_limited");
}
