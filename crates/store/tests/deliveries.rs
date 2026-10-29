mod support;

use hookrelay_store::deliveries::{DeliveryFilter, find, list, replay};
use hookrelay_store::error::StoreError;

async fn dead_delivery(db: &support::TestDb, tenant_id: uuid::Uuid) -> uuid::Uuid {
    support::create_endpoint(&db.pool, tenant_id).await;
    let ingested = support::publish(&db.pool, tenant_id, "invoice.paid").await;
    let delivery_id = ingested.delivery_ids[0];

    sqlx::query("update deliveries set status = 'dead', completed_at = now() where id = $1")
        .bind(delivery_id)
        .execute(&db.pool)
        .await
        .unwrap();

    delivery_id
}

#[tokio::test]
async fn replay_creates_pending_copy_and_preserves_history() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let original_id = dead_delivery(&db, tenant.id).await;

    let replayed = replay(&db.pool, tenant.id, original_id).await.unwrap();

    assert_ne!(replayed.id, original_id);
    assert_eq!(replayed.status, "pending");
    assert_eq!(replayed.attempt, 0);

    let replay_of: Option<uuid::Uuid> =
        sqlx::query_scalar("select replay_of from deliveries where id = $1")
            .bind(replayed.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let original_status: String = sqlx::query_scalar("select status from deliveries where id = $1")
        .bind(original_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    assert_eq!(replay_of, Some(original_id));
    assert_eq!(original_status, "dead");
}

#[tokio::test]
async fn replay_rejects_foreign_tenant() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let other = hookrelay_store::tenants::create(&db.pool, "Other")
        .await
        .unwrap();
    let original_id = dead_delivery(&db, tenant.id).await;

    let result = replay(&db.pool, other.id, original_id).await;

    assert!(matches!(result, Err(StoreError::NotFound)));
}

#[tokio::test]
async fn replay_rejects_unfinished_delivery() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    support::create_endpoint(&db.pool, tenant.id).await;
    let ingested = support::publish(&db.pool, tenant.id, "invoice.paid").await;

    let result = replay(&db.pool, tenant.id, ingested.delivery_ids[0]).await;

    assert!(matches!(result, Err(StoreError::Conflict(_))));
}

#[tokio::test]
async fn lists_dead_letters_for_tenant_only() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let other = hookrelay_store::tenants::create(&db.pool, "Other")
        .await
        .unwrap();
    let mine = dead_delivery(&db, tenant.id).await;
    dead_delivery(&db, other.id).await;

    let rows = list(
        &db.pool,
        DeliveryFilter {
            tenant_id: tenant.id,
            status: Some("dead".to_owned()),
            endpoint_id: None,
            limit: 50,
        },
    )
    .await
    .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, mine);
    assert_eq!(rows[0].status, "dead");
}

#[tokio::test]
async fn find_includes_payload_and_attempts() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let delivery_id = dead_delivery(&db, tenant.id).await;

    sqlx::query(
        "insert into delivery_attempts
            (delivery_id, attempt_no, started_at, finished_at, status_code, latency_ms, error)
         values ($1, 1, now(), now(), 500, 42, 'receiver returned 500')",
    )
    .bind(delivery_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let detail = find(&db.pool, tenant.id, delivery_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(detail.payload, serde_json::json!({"ok": true}));
    assert_eq!(detail.attempts.len(), 1);
    assert_eq!(detail.attempts[0].status_code, Some(500));
    assert_eq!(detail.attempts[0].latency_ms, Some(42));
}
