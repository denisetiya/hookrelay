mod support;

#[tokio::test]
async fn creates_and_resolves_active_key() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();

    let created = hookrelay_store::api_keys::create(&db.pool, tenant.id, "hr_live_abc", "hash")
        .await
        .unwrap();

    let found = hookrelay_store::api_keys::find_active_by_prefix(&db.pool, &created.prefix)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found.id, created.id);
    assert_eq!(found.tenant_id, tenant.id);
}

#[tokio::test]
async fn revoked_key_is_no_longer_active() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let key = hookrelay_store::api_keys::create(&db.pool, tenant.id, "hr_live_abc", "hash")
        .await
        .unwrap();

    hookrelay_store::api_keys::revoke(&db.pool, tenant.id, key.id)
        .await
        .unwrap();

    let found = hookrelay_store::api_keys::find_active_by_prefix(&db.pool, &key.prefix)
        .await
        .unwrap();

    assert!(found.is_none());
}

#[tokio::test]
async fn rejects_duplicate_prefix() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();

    hookrelay_store::api_keys::create(&db.pool, tenant.id, "hr_live_abc", "hash")
        .await
        .unwrap();
    let second =
        hookrelay_store::api_keys::create(&db.pool, tenant.id, "hr_live_abc", "hash").await;

    assert!(matches!(
        second,
        Err(hookrelay_store::error::StoreError::Conflict(_))
    ));
}
