mod support;

use hookrelay_worker::reap_expired;

#[tokio::test]
async fn expired_lease_returns_delivery_to_queue() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    let ingested = support::publish(&db.pool, tenant.id, "invoice.paid").await;
    let delivery_id = ingested.delivery_ids[0];

    sqlx::query(
        "update deliveries
         set status = 'in_flight',
             attempt = 1,
             lease_owner = 'dead-worker',
             lease_until = now() - interval '1 second'
         where id = $1",
    )
    .bind(delivery_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let reaped = reap_expired(&db.pool).await.unwrap();

    assert_eq!(reaped, 1);

    let status: String = sqlx::query_scalar("select status from deliveries where id = $1")
        .bind(delivery_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    assert_eq!(status, "retrying");
}

#[tokio::test]
async fn lease_reaper_leaves_live_leases_alone() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    support::seed_endpoint(&db.pool, tenant.id, "endpoint").await;
    let ingested = support::publish(&db.pool, tenant.id, "invoice.paid").await;
    let delivery_id = ingested.delivery_ids[0];

    sqlx::query(
        "update deliveries
         set status = 'in_flight',
             attempt = 1,
             lease_owner = 'live-worker',
             lease_until = now() + interval '30 seconds'
         where id = $1",
    )
    .bind(delivery_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let reaped = reap_expired(&db.pool).await.unwrap();

    assert_eq!(reaped, 0);
}
