mod support;

use serde_json::json;

#[tokio::test]
async fn duplicate_idempotency_key_creates_one_event() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    support::create_endpoint(&db.pool, tenant.id).await;

    let payload = json!({"id": "in_1"});
    let first = hookrelay_store::events::ingest(
        &db.pool,
        hookrelay_store::events::IngestEvent {
            id: uuid::Uuid::now_v7(),
            tenant_id: tenant.id,
            event_type: "invoice.paid",
            payload: &payload,
            idempotency_key: Some("evt-1"),
        },
    )
    .await
    .unwrap();

    let second = hookrelay_store::events::ingest(
        &db.pool,
        hookrelay_store::events::IngestEvent {
            id: uuid::Uuid::now_v7(),
            tenant_id: tenant.id,
            event_type: "invoice.paid",
            payload: &payload,
            idempotency_key: Some("evt-1"),
        },
    )
    .await
    .unwrap();

    assert!(first.created);
    assert_eq!(first.delivery_ids.len(), 1);
    assert!(!second.created);
    assert_eq!(second.event.id, first.event.id);
    assert!(second.delivery_ids.is_empty());
}

#[tokio::test]
async fn fan_out_respects_event_type_filter() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    support::create_endpoint_with_types(&db.pool, tenant.id, &["invoice.paid"]).await;
    support::create_endpoint_with_types(&db.pool, tenant.id, &["user.created"]).await;
    support::create_endpoint_with_types(&db.pool, tenant.id, &[]).await;

    let ingested = support::ingest(&db.pool, tenant.id, "invoice.paid").await;

    assert_eq!(ingested.delivery_ids.len(), 2);
}

#[tokio::test]
async fn skips_paused_endpoints() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let endpoint = support::create_endpoint(&db.pool, tenant.id).await;
    hookrelay_store::endpoints::set_status(&db.pool, tenant.id, endpoint.id, "paused")
        .await
        .unwrap();

    let ingested = support::ingest(&db.pool, tenant.id, "invoice.paid").await;

    assert!(ingested.delivery_ids.is_empty());
}

#[tokio::test]
async fn events_without_idempotency_key_are_not_deduplicated() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();

    let payload = json!({});
    for _ in 0..2 {
        hookrelay_store::events::ingest(
            &db.pool,
            hookrelay_store::events::IngestEvent {
                id: uuid::Uuid::now_v7(),
                tenant_id: tenant.id,
                event_type: "ping",
                payload: &payload,
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    }

    let count: i64 = sqlx::query_scalar("select count(*) from events where tenant_id = $1")
        .bind(tenant.id)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    assert_eq!(count, 2);
}
