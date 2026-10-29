mod support;

use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::post;
use hookrelay_core::backoff::BackoffPolicy;
use hookrelay_sender::{Sender, SenderConfig};
use hookrelay_worker::{Worker, WorkerConfig};

async fn spawn_receiver(status: StatusCode) -> String {
    let app = Router::new().route("/hook", post(move || async move { status }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{addr}/hook")
}

fn worker(pool: &sqlx::PgPool) -> Worker {
    let sender = Sender::new(SenderConfig {
        allow_private_targets: true,
        connect_timeout: Duration::from_millis(500),
        request_timeout: Duration::from_millis(500),
        ..SenderConfig::default()
    });

    Worker::new(
        pool.clone(),
        sender,
        WorkerConfig {
            worker_id: "w1".to_owned(),
            batch_size: 10,
            lease: Duration::from_secs(30),
            poll_interval: Duration::from_millis(50),
            max_concurrency: 4,
            backoff: BackoffPolicy {
                base: Duration::from_secs(1),
                factor: 2,
                max: Duration::from_secs(30),
                max_attempts: 8,
            },
        },
    )
}

#[tokio::test]
async fn successful_delivery_is_marked_succeeded_with_attempt_log() {
    let url = spawn_receiver(StatusCode::OK).await;
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let endpoint = support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    support::configure_endpoint(&db.pool, endpoint.id, &url, 8).await;
    let ingested = support::publish(&db.pool, tenant.id, "invoice.paid").await;
    let delivery_id = ingested.delivery_ids[0];

    let processed = worker(&db.pool).run_once().await.unwrap();

    assert_eq!(processed, 1);

    let status: String = sqlx::query_scalar("select status from deliveries where id = $1")
        .bind(delivery_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let attempt: (i16, Option<i16>) = sqlx::query_as(
        "select attempt_no, status_code from delivery_attempts where delivery_id = $1",
    )
    .bind(delivery_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    assert_eq!(status, "succeeded");
    assert_eq!(attempt, (1, Some(200)));
}

#[tokio::test]
async fn retryable_failure_schedules_next_attempt() {
    let url = spawn_receiver(StatusCode::SERVICE_UNAVAILABLE).await;
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let endpoint = support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    support::configure_endpoint(&db.pool, endpoint.id, &url, 8).await;
    let ingested = support::publish(&db.pool, tenant.id, "invoice.paid").await;

    worker(&db.pool).run_once().await.unwrap();

    let row: (String, i16, bool, Option<String>) = sqlx::query_as(
        "select status, attempt, next_attempt_at > now(), last_error
         from deliveries where id = $1",
    )
    .bind(ingested.delivery_ids[0])
    .fetch_one(&db.pool)
    .await
    .unwrap();

    assert_eq!(row.0, "retrying");
    assert_eq!(row.1, 1);
    assert!(row.2);
    assert!(row.3.unwrap().contains("503"));
}

#[tokio::test]
async fn permanent_failure_marks_dead_without_retry() {
    let url = spawn_receiver(StatusCode::NOT_FOUND).await;
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let endpoint = support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    support::configure_endpoint(&db.pool, endpoint.id, &url, 8).await;
    let ingested = support::publish(&db.pool, tenant.id, "invoice.paid").await;

    worker(&db.pool).run_once().await.unwrap();

    let row: (String, bool) =
        sqlx::query_as("select status, completed_at is not null from deliveries where id = $1")
            .bind(ingested.delivery_ids[0])
            .fetch_one(&db.pool)
            .await
            .unwrap();

    assert_eq!(row, ("dead".to_owned(), true));
}

#[tokio::test]
async fn exhausting_attempts_marks_dead() {
    let url = spawn_receiver(StatusCode::INTERNAL_SERVER_ERROR).await;
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let endpoint = support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    support::configure_endpoint(&db.pool, endpoint.id, &url, 1).await;
    let ingested = support::publish(&db.pool, tenant.id, "invoice.paid").await;

    worker(&db.pool).run_once().await.unwrap();

    let status: String = sqlx::query_scalar("select status from deliveries where id = $1")
        .bind(ingested.delivery_ids[0])
        .fetch_one(&db.pool)
        .await
        .unwrap();

    assert_eq!(status, "dead");
}
