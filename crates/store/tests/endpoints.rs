mod support;

#[tokio::test]
async fn creates_and_lists_endpoints() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();

    let created = hookrelay_store::endpoints::create(
        &db.pool,
        hookrelay_store::endpoints::NewEndpoint {
            tenant_id: tenant.id,
            url: "https://example.com/hooks",
            description: Some("primary"),
            secret: b"0123456789abcdef0123456789abcdef",
            event_types: &["invoice.paid".to_owned()],
            ordering_mode: "endpoint",
        },
    )
    .await
    .unwrap();

    let listed = hookrelay_store::endpoints::list_for_tenant(&db.pool, tenant.id)
        .await
        .unwrap();

    assert_eq!(created.id, listed[0].id);
    assert_eq!(listed[0].event_types, ["invoice.paid"]);
    assert_eq!(listed[0].status, "active");
}

#[tokio::test]
async fn rotating_secret_keeps_previous_for_overlap() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let endpoint = support::create_endpoint(&db.pool, tenant.id).await;

    let rotated = hookrelay_store::endpoints::rotate_secret(
        &db.pool,
        tenant.id,
        endpoint.id,
        b"fedcba9876543210fedcba9876543210",
        chrono::Utc::now(),
    )
    .await
    .unwrap();

    assert_eq!(rotated.secret_current, b"fedcba9876543210fedcba9876543210");
    assert_eq!(
        rotated.secret_previous.as_deref(),
        Some(endpoint.secret_current.as_slice())
    );
}

#[tokio::test]
async fn tenant_isolation_hides_foreign_endpoints() {
    let db = support::test_db().await;
    let acme = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let other = hookrelay_store::tenants::create(&db.pool, "Other")
        .await
        .unwrap();
    let endpoint = support::create_endpoint(&db.pool, acme.id).await;

    let found = hookrelay_store::endpoints::find(&db.pool, other.id, endpoint.id)
        .await
        .unwrap();

    assert!(found.is_none());
}

#[tokio::test]
async fn rejects_unknown_status() {
    let db = support::test_db().await;
    let tenant = hookrelay_store::tenants::create(&db.pool, "Acme")
        .await
        .unwrap();
    let endpoint = support::create_endpoint(&db.pool, tenant.id).await;

    let result =
        hookrelay_store::endpoints::set_status(&db.pool, tenant.id, endpoint.id, "gone").await;

    assert!(matches!(
        result,
        Err(hookrelay_store::error::StoreError::Conflict(_))
    ));
}
