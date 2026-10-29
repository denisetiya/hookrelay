mod support;

use std::time::Duration;

use hookrelay_worker::claim;

const LEASE: Duration = Duration::from_secs(30);

#[tokio::test]
async fn claims_due_delivery_with_event_and_endpoint() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    let ingested = support::publish(&db.pool, tenant.id, "invoice.paid").await;

    let claimed = claim(&db.pool, "w1", 10, LEASE).await.unwrap();

    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].delivery_id, ingested.delivery_ids[0]);
    assert_eq!(claimed[0].event_id, ingested.event.id);
    assert_eq!(claimed[0].url, "https://example.com/hooks");
    assert_eq!(claimed[0].attempt, 1);
    assert_eq!(claimed[0].payload, serde_json::json!({"ok": true}));
}

#[tokio::test]
async fn does_not_claim_future_delivery() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    support::publish(&db.pool, tenant.id, "invoice.paid").await;

    sqlx::query("update deliveries set next_attempt_at = now() + interval '1 hour'")
        .execute(&db.pool)
        .await
        .unwrap();

    let claimed = claim(&db.pool, "w1", 10, LEASE).await.unwrap();

    assert!(claimed.is_empty());
}

#[tokio::test]
async fn ordered_endpoint_never_has_two_in_flight() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    support::publish(&db.pool, tenant.id, "first").await;
    support::publish(&db.pool, tenant.id, "second").await;

    let first_claim = claim(&db.pool, "w1", 10, LEASE).await.unwrap();
    let second_claim = claim(&db.pool, "w2", 10, LEASE).await.unwrap();

    assert_eq!(first_claim.len(), 1);
    assert!(second_claim.is_empty());

    sqlx::query("update deliveries set status = 'succeeded' where status = 'in_flight'")
        .execute(&db.pool)
        .await
        .unwrap();

    let third_claim = claim(&db.pool, "w1", 10, LEASE).await.unwrap();

    assert_eq!(third_claim.len(), 1);
    assert_ne!(third_claim[0].delivery_id, first_claim[0].delivery_id);
}

#[tokio::test]
async fn unordered_endpoint_may_have_two_in_flight() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    support::seed_endpoint(&db.pool, tenant.id, "none").await;
    support::publish(&db.pool, tenant.id, "first").await;
    support::publish(&db.pool, tenant.id, "second").await;

    let claimed = claim(&db.pool, "w1", 10, LEASE).await.unwrap();

    assert_eq!(claimed.len(), 2);
}

#[tokio::test]
async fn concurrent_claims_do_not_double_claim() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    support::publish(&db.pool, tenant.id, "invoice.paid").await;

    let mut tasks = Vec::new();
    for worker in 0..4 {
        let pool = db.pool.clone();
        tasks.push(tokio::spawn(async move {
            claim(&pool, &format!("w{worker}"), 10, LEASE)
                .await
                .unwrap()
                .len()
        }));
    }

    let mut total = 0;
    for task in tasks {
        total += task.await.unwrap();
    }

    assert_eq!(total, 1);
}
