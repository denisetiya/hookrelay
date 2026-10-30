mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use hookrelay_core::backoff::BackoffPolicy;
use hookrelay_sender::{Sender, SenderConfig};
use hookrelay_worker::{Worker, WorkerConfig};

#[derive(Clone)]
struct Flaky {
    failures: usize,
    delay: Duration,
    seen: Arc<AtomicUsize>,
}

async fn handle(State(state): State<Flaky>) -> StatusCode {
    let attempt = state.seen.fetch_add(1, Ordering::SeqCst) + 1;

    if !state.delay.is_zero() {
        tokio::time::sleep(state.delay).await;
    }

    if attempt <= state.failures {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::OK
    }
}

async fn spawn_receiver(failures: usize, delay: Duration) -> String {
    let state = Flaky {
        failures,
        delay,
        seen: Arc::default(),
    };
    let app = Router::new().route("/hook", post(handle)).with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{addr}/hook")
}

fn worker_with(
    pool: &sqlx::PgPool,
    lease: Duration,
    max_concurrency: usize,
    base: Duration,
) -> Worker {
    let sender = Sender::new(SenderConfig {
        allow_private_targets: true,
        connect_timeout: Duration::from_millis(500),
        request_timeout: Duration::from_secs(2),
        ..SenderConfig::default()
    });

    Worker::new(
        pool.clone(),
        sender,
        WorkerConfig {
            worker_id: "chaos".to_owned(),
            batch_size: 10,
            lease,
            poll_interval: Duration::from_millis(20),
            max_concurrency,
            backoff: BackoffPolicy {
                base,
                factor: 2,
                max: Duration::from_secs(5),
                max_attempts: 8,
            },
        },
    )
}

#[tokio::test]
async fn worker_crash_mid_delivery_returns_to_queue() {
    let url = spawn_receiver(0, Duration::from_millis(500)).await;
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let endpoint = support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    support::configure_endpoint(&db.pool, endpoint.id, &url, 8).await;
    let ingested = support::publish(&db.pool, tenant.id, "invoice.paid").await;

    let worker = worker_with(
        &db.pool,
        Duration::from_millis(200),
        4,
        Duration::from_millis(50),
    );
    let task = tokio::spawn(async move { worker.run_once().await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    task.abort();

    tokio::time::sleep(Duration::from_millis(300)).await;
    let reaped = hookrelay_worker::reap_expired(&db.pool).await.unwrap();

    assert_eq!(reaped, 1);

    let status: String = sqlx::query_scalar("select status from deliveries where id = $1")
        .bind(ingested.delivery_ids[0])
        .fetch_one(&db.pool)
        .await
        .unwrap();

    assert_eq!(status, "retrying");
}

#[tokio::test]
async fn receiver_outage_then_recovery_delivers_once() {
    let url = spawn_receiver(2, Duration::ZERO).await;
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let endpoint = support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    support::configure_endpoint(&db.pool, endpoint.id, &url, 8).await;
    let ingested = support::publish(&db.pool, tenant.id, "invoice.paid").await;
    let worker = worker_with(
        &db.pool,
        Duration::from_secs(30),
        4,
        Duration::from_millis(100),
    );

    let mut status = String::new();
    for _ in 0..10 {
        worker.run_once().await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        status = sqlx::query_scalar("select status from deliveries where id = $1")
            .bind(ingested.delivery_ids[0])
            .fetch_one(&db.pool)
            .await
            .unwrap();

        if status == "succeeded" {
            break;
        }
    }

    let attempts: i64 =
        sqlx::query_scalar("select count(*) from delivery_attempts where delivery_id = $1")
            .bind(ingested.delivery_ids[0])
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let successes: i64 = sqlx::query_scalar(
        "select count(*) from delivery_attempts where delivery_id = $1 and status_code = 200",
    )
    .bind(ingested.delivery_ids[0])
    .fetch_one(&db.pool)
    .await
    .unwrap();

    assert_eq!(status, "succeeded");
    assert_eq!(attempts, 3);
    assert_eq!(successes, 1);
}

#[tokio::test]
async fn slow_receiver_does_not_block_other_endpoints() {
    let slow_url = spawn_receiver(0, Duration::from_millis(400)).await;
    let fast_url = spawn_receiver(0, Duration::ZERO).await;
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let slow = support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    let fast = support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;

    support::configure_endpoint(&db.pool, slow.id, &slow_url, 8).await;
    support::configure_endpoint(&db.pool, fast.id, &fast_url, 8).await;

    support::publish(&db.pool, tenant.id, "invoice.paid").await;

    let worker = worker_with(
        &db.pool,
        Duration::from_secs(30),
        2,
        Duration::from_millis(100),
    );
    let started = Instant::now();
    worker.run_once().await.unwrap();
    let elapsed = started.elapsed();

    let succeeded: i64 =
        sqlx::query_scalar("select count(*) from deliveries where status = 'succeeded'")
            .fetch_one(&db.pool)
            .await
            .unwrap();

    assert_eq!(succeeded, 2);
    assert!(
        elapsed < Duration::from_millis(800),
        "deliveries appear serialized: {elapsed:?}"
    );
}
