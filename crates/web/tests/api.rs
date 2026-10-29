mod support;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    key: Option<&str>,
    idempotency: Option<&str>,
    body: Body,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");

    if let Some(key) = key {
        request = request.header(header::AUTHORIZATION, format!("Bearer {key}"));
    }
    if let Some(idempotency) = idempotency {
        request = request.header("idempotency-key", idempotency);
    }

    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

    (status, json)
}

fn event_body() -> Body {
    Body::from(json!({"type": "invoice.paid", "payload": {"id": "in_1"}}).to_string())
}

#[tokio::test]
async fn rejects_missing_api_key() {
    let db = support::test_db().await;
    let (status, body) = call(
        &support::app(db.pool.clone()),
        "POST",
        "/v1/events",
        None,
        None,
        event_body(),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn rejects_unknown_key_prefix() {
    let db = support::test_db().await;
    let (status, _) = call(
        &support::app(db.pool.clone()),
        "POST",
        "/v1/events",
        Some("hr_live_nope_secret"),
        None,
        event_body(),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn accepts_valid_key_and_ingests_event() {
    let db = support::test_db().await;
    let (tenant_id, key) = support::seed_key(&db.pool).await;
    let app = support::app(db.pool.clone());

    let (status, body) = call(&app, "POST", "/v1/events", Some(&key), None, event_body()).await;

    assert_eq!(status, StatusCode::CREATED);
    let event_id = body["id"].as_str().unwrap();

    let stored: String =
        sqlx::query_scalar("select type from events where id = $1 and tenant_id = $2")
            .bind(uuid::Uuid::parse_str(event_id).unwrap())
            .bind(tenant_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();

    assert_eq!(stored, "invoice.paid");
}

#[tokio::test]
async fn repeated_idempotency_key_returns_200_and_same_id() {
    let db = support::test_db().await;
    let (_, key) = support::seed_key(&db.pool).await;
    let app = support::app(db.pool.clone());

    let (first_status, first) = call(
        &app,
        "POST",
        "/v1/events",
        Some(&key),
        Some("evt-1"),
        event_body(),
    )
    .await;
    let (second_status, second) = call(
        &app,
        "POST",
        "/v1/events",
        Some(&key),
        Some("evt-1"),
        event_body(),
    )
    .await;

    assert_eq!(first_status, StatusCode::CREATED);
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(first["id"], second["id"]);
}

#[tokio::test]
async fn rejects_payload_over_limit() {
    let db = support::test_db().await;
    let (_, key) = support::seed_key(&db.pool).await;
    let app = support::app(db.pool.clone());
    let payload = json!({"type": "big", "payload": {"blob": "a".repeat(300 * 1024)}});

    let (status, _) = call(
        &app,
        "POST",
        "/v1/events",
        Some(&key),
        None,
        Body::from(payload.to_string()),
    )
    .await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn rotates_secret_and_returns_it_once() {
    let db = support::test_db().await;
    let (_, key) = support::seed_key(&db.pool).await;
    let app = support::app(db.pool.clone());

    let (created_status, created) = call(
        &app,
        "POST",
        "/v1/endpoints",
        Some(&key),
        None,
        Body::from(json!({"url": "https://example.com/hooks"}).to_string()),
    )
    .await;

    assert_eq!(created_status, StatusCode::CREATED);
    assert!(
        created["secret"]
            .as_str()
            .is_some_and(|value| value.len() == 64)
    );

    let endpoint_id = created["id"].as_str().unwrap();
    let (rotated_status, rotated) = call(
        &app,
        "POST",
        &format!("/v1/endpoints/{endpoint_id}/rotate-secret"),
        Some(&key),
        None,
        Body::empty(),
    )
    .await;

    assert_eq!(rotated_status, StatusCode::OK);
    assert_ne!(rotated["secret"], created["secret"]);

    let (_, listed) = call(
        &app,
        "GET",
        "/v1/endpoints",
        Some(&key),
        None,
        Body::empty(),
    )
    .await;

    assert!(listed[0].get("secret").is_none());
}
