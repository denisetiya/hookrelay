mod support;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

async fn get(app: &Router, path: &str, cookie: Option<&str>) -> (StatusCode, String) {
    let mut request = Request::builder().method("GET").uri(path);

    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }

    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn redirects_to_login_without_session() {
    let db = support::test_db().await;
    let app = support::app(db.pool.clone());

    let (status, _) = get(&app, "/", None).await;

    assert_eq!(status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn overview_renders_with_empty_database() {
    let db = support::test_db().await;
    let (tenant_id, _) = support::seed_key(&db.pool).await;
    let app = support::app(db.pool.clone());
    let cookie = support::session_cookie(tenant_id);

    let (status, body) = get(&app, "/", Some(&cookie)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("No deliveries yet"));
}

#[tokio::test]
async fn dead_letters_show_replay_button() {
    let db = support::test_db().await;
    let (tenant_id, _) = support::seed_key(&db.pool).await;
    let delivery_id = support::seed_delivery(&db.pool, tenant_id).await;

    sqlx::query("update deliveries set status = 'dead', completed_at = now() where id = $1")
        .bind(delivery_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let app = support::app(db.pool.clone());
    let cookie = support::session_cookie(tenant_id);
    let (status, body) = get(&app, "/dead", Some(&cookie)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Replay"));
    assert!(body.contains("dead"));
}

#[tokio::test]
async fn replay_partial_returns_pending_row() {
    let db = support::test_db().await;
    let (tenant_id, _) = support::seed_key(&db.pool).await;
    let delivery_id = support::seed_delivery(&db.pool, tenant_id).await;

    sqlx::query("update deliveries set status = 'dead', completed_at = now() where id = $1")
        .bind(delivery_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let app = support::app(db.pool.clone());
    let cookie = support::session_cookie(tenant_id);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/partials/deliveries/{delivery_id}/replay"))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8_lossy(&body);

    assert!(body.contains("pending"));
}

#[tokio::test]
async fn every_nav_link_resolves() {
    let db = support::test_db().await;
    let (tenant_id, _) = support::seed_key(&db.pool).await;
    let app = support::app(db.pool.clone());
    let cookie = support::session_cookie(tenant_id);

    for path in ["/", "/endpoints", "/events", "/deliveries", "/dead"] {
        let (status, body) = get(&app, path, Some(&cookie)).await;

        assert_eq!(status, StatusCode::OK, "path {path}");
        assert!(
            !body.contains('\u{2014}'),
            "path {path} contains an em dash"
        );
    }
}
