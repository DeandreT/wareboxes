mod common;

use common::*;
use wareboxes_application::integration::{IntegrationInboxReadScope, NewIntegrationInboxReceipt};
use wareboxes_domain::{FacilityId, InventoryOwnerId};
use wareboxes_persistence_postgres::{integration_inbox, PersistenceError};

async fn inbox_scope(
    fixture: &Fixture,
    tenant_id: TenantId,
    label: &str,
) -> (InventoryOwnerId, FacilityId) {
    let owner_id = fixture.inventory_owner(tenant_id, label).await;
    let facility_id = fixture.facility(tenant_id, label).await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO inventory_owner_facilities
            (tenant_id, created, inventory_owner_id, facility_id)
        VALUES ($1, clock_timestamp(), $2, $3)
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id)
    .bind(facility_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (
        InventoryOwnerId::new(owner_id).unwrap(),
        FacilityId::new(facility_id).unwrap(),
    )
}

#[tokio::test]
async fn raw_receipts_are_exact_and_identical_retries_return_the_original() {
    let fixture = Fixture::new().await;
    let user = fixture.user("inbox-raw@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let (inventory_owner_id, facility_id) = inbox_scope(&fixture, tenant_id, "Inbox Raw").await;
    let raw_payload = [0, 159, 146, 150, 255, b'{', b'}'];

    let first = integration_inbox::receive(
        &fixture.db,
        &NewIntegrationInboxReceipt {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            owner_mapping: None,
            source_key: "partner-sftp",
            deduplication_key: "message-100",
            content_type: "application/octet-stream",
            raw_payload: &raw_payload,
            request_id: Some("request-first"),
        },
    )
    .await
    .unwrap();
    assert!(!first.replayed);
    assert_eq!(first.receipt.raw_payload, raw_payload);
    assert_eq!(first.receipt.payload_sha256.len(), 32);

    let replay = integration_inbox::receive(
        &fixture.db,
        &NewIntegrationInboxReceipt {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            owner_mapping: None,
            source_key: "partner-sftp",
            deduplication_key: "message-100",
            content_type: "application/octet-stream",
            raw_payload: &raw_payload,
            request_id: Some("request-retry"),
        },
    )
    .await
    .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt, first.receipt);
    assert_eq!(replay.receipt.request_id.as_deref(), Some("request-first"));

    let read = integration_inbox::get(
        &fixture.db,
        IntegrationInboxReadScope {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
        },
        first.receipt.id,
    )
    .await
    .unwrap();
    assert_eq!(read, Some(first.receipt));

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM integration_inbox_receipts),
               (SELECT COUNT(*) FROM integration_inbox_keys)
        "#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(counts, (1, 1));

    let oversized = vec![0; 16 * 1024 * 1024 + 1];
    let oversized_result = integration_inbox::receive(
        &fixture.db,
        &NewIntegrationInboxReceipt {
            tenant_id,
            inventory_owner_id: None,
            facility_id: None,
            owner_mapping: None,
            source_key: "partner-sftp",
            deduplication_key: "oversized-message",
            content_type: "application/octet-stream",
            raw_payload: &oversized,
            request_id: None,
        },
    )
    .await;
    assert!(matches!(
        oversized_result,
        Err(PersistenceError::InvalidInput(_))
    ));
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let oversized_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM integration_inbox_receipts WHERE deduplication_key = 'oversized-message'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(oversized_rows, 0);
}

#[tokio::test]
async fn concurrent_receipt_deduplication_has_one_canonical_winner() {
    let fixture = Fixture::new().await;
    let user = fixture.user("inbox-race@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let payload = br#"{"external_order":"race-1"}"#;
    let first = NewIntegrationInboxReceipt {
        tenant_id,
        inventory_owner_id: None,
        facility_id: None,
        owner_mapping: None,
        source_key: "orders-api",
        deduplication_key: "race-1",
        content_type: "application/json",
        raw_payload: payload,
        request_id: Some("race-request-a"),
    };
    let second = NewIntegrationInboxReceipt {
        request_id: Some("race-request-b"),
        ..first
    };

    let (first_result, second_result) = tokio::join!(
        integration_inbox::receive(&fixture.db, &first),
        integration_inbox::receive(&fixture.db, &second)
    );
    let first_result = first_result.unwrap();
    let second_result = second_result.unwrap();
    assert_eq!(first_result.receipt.id, second_result.receipt.id);
    assert_ne!(first_result.replayed, second_result.replayed);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM integration_inbox_receipts),
               (SELECT COUNT(*) FROM integration_inbox_keys)
        "#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(counts, (1, 1));
}

#[tokio::test]
async fn deduplication_key_reuse_with_changed_content_or_scope_is_rejected() {
    let fixture = Fixture::new().await;
    let user = fixture.user("inbox-conflict@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let (inventory_owner_id, facility_id) =
        inbox_scope(&fixture, tenant_id, "Inbox Conflict").await;
    let original_payload = b"original bytes";

    let original = integration_inbox::receive(
        &fixture.db,
        &NewIntegrationInboxReceipt {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            owner_mapping: None,
            source_key: "edi-gateway",
            deduplication_key: "edi-control-1",
            content_type: "application/edi-x12",
            raw_payload: original_payload,
            request_id: None,
        },
    )
    .await
    .unwrap();

    let changed_payload = integration_inbox::receive(
        &fixture.db,
        &NewIntegrationInboxReceipt {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            owner_mapping: None,
            source_key: "edi-gateway",
            deduplication_key: "edi-control-1",
            content_type: "application/edi-x12",
            raw_payload: b"changed bytes",
            request_id: None,
        },
    )
    .await;
    assert!(matches!(
        changed_payload,
        Err(PersistenceError::Conflict(_))
    ));

    let changed_scope = integration_inbox::receive(
        &fixture.db,
        &NewIntegrationInboxReceipt {
            tenant_id,
            inventory_owner_id: None,
            facility_id: None,
            owner_mapping: None,
            source_key: "edi-gateway",
            deduplication_key: "edi-control-1",
            content_type: "application/edi-x12",
            raw_payload: original_payload,
            request_id: None,
        },
    )
    .await;
    assert!(matches!(changed_scope, Err(PersistenceError::Conflict(_))));

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let stored_payload: Vec<u8> =
        sqlx::query_scalar("SELECT raw_payload FROM integration_inbox_receipts WHERE id = $1")
            .bind(original.receipt.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(stored_payload, original_payload);
}

#[tokio::test]
async fn inbox_tables_fail_closed_and_enforce_scoped_immutable_envelopes() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("inbox-rls-a@test.com").await;
    let user_b = fixture.user("inbox-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let (owner_a, facility_a) = inbox_scope(&fixture, tenant_a, "Inbox Tenant A").await;
    let owner_b = InventoryOwnerId::new(
        fixture
            .inventory_owner(tenant_b, "Inbox Tenant B Owner")
            .await,
    )
    .unwrap();

    db::validate_runtime_role(&fixture.db).await.unwrap();
    let received = integration_inbox::receive(
        &fixture.db,
        &NewIntegrationInboxReceipt {
            tenant_id: tenant_a,
            inventory_owner_id: Some(owner_a),
            facility_id: Some(facility_a),
            owner_mapping: None,
            source_key: "tenant-a-source",
            deduplication_key: "tenant-a-message",
            content_type: "text/plain",
            raw_payload: b"tenant a",
            request_id: None,
        },
    )
    .await
    .unwrap();

    assert!(integration_inbox::get(
        &fixture.db,
        IntegrationInboxReadScope {
            tenant_id: tenant_a,
            inventory_owner_id: None,
            facility_id: None,
        },
        received.receipt.id,
    )
    .await
    .unwrap()
    .is_none());
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let guessed_receipts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM integration_inbox_receipts WHERE id = $1")
            .bind(received.receipt.id)
            .fetch_one(&mut *tenant_b_tx)
            .await
            .unwrap();
    tenant_b_tx.rollback().await.unwrap();
    assert_eq!(guessed_receipts, 0);
    let unbound_counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM integration_inbox_receipts),
               (SELECT COUNT(*) FROM integration_inbox_keys)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0));

    let wrong_tenant_scope = integration_inbox::receive(
        &fixture.db,
        &NewIntegrationInboxReceipt {
            tenant_id: tenant_a,
            inventory_owner_id: Some(owner_b),
            facility_id: None,
            owner_mapping: None,
            source_key: "tenant-a-source",
            deduplication_key: "wrong-owner",
            content_type: "text/plain",
            raw_payload: b"wrong owner",
            request_id: None,
        },
    )
    .await;
    assert!(wrong_tenant_scope.is_err());

    let runtime_privileges: (bool, bool, bool, bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege(
                   current_user, 'integration_inbox_receipts', 'SELECT'
               ),
               has_table_privilege(
                   current_user, 'integration_inbox_receipts', 'INSERT'
               ),
               has_table_privilege(
                   current_user, 'integration_inbox_receipts', 'UPDATE'
               ),
               has_table_privilege(
                   current_user, 'integration_inbox_receipts', 'DELETE'
               ),
               has_table_privilege(
                   current_user, 'integration_inbox_receipts', 'TRUNCATE'
               ),
               has_sequence_privilege(
                   current_user, 'integration_inbox_receipts_id_seq', 'USAGE'
               ),
               has_sequence_privilege(
                   current_user, 'integration_inbox_receipts_id_seq', 'SELECT'
               )
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(
        runtime_privileges,
        (true, true, false, false, false, true, false)
    );

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    assert!(
        sqlx::query("UPDATE integration_inbox_receipts SET raw_payload = $1 WHERE id = $2")
            .bind(b"changed".as_slice())
            .bind(received.receipt.id)
            .execute(&mut *tenant_a_tx)
            .await
            .is_err()
    );
    tenant_a_tx.rollback().await.unwrap();

    let mut invalid_hash_tx = tenant_tx(&fixture.db, tenant_a).await;
    let invalid_hash = sqlx::query(
        r#"
        INSERT INTO integration_inbox_receipts
            (tenant_id, received_at, source_key, deduplication_key, content_type,
             raw_payload, payload_sha256)
        VALUES ($1, clock_timestamp(), 'constraint-source', 'constraint-key',
                'text/plain', '\x00'::BYTEA, '\x01'::BYTEA)
        "#,
    )
    .bind(tenant_a.get())
    .execute(&mut *invalid_hash_tx)
    .await;
    invalid_hash_tx.rollback().await.unwrap();
    assert!(invalid_hash.is_err());

    let mut forged_key_tx = tenant_tx(&fixture.db, tenant_a).await;
    let forged_key = sqlx::query(
        r#"
        INSERT INTO integration_inbox_keys
            (tenant_id, source_key, deduplication_key, created_at, receipt_id,
             inventory_owner_id, facility_id, content_type, payload_sha256)
        VALUES ($1, 'forged-source', 'forged-key', clock_timestamp(), $2,
                $3, $4, 'text/plain', $5)
        "#,
    )
    .bind(tenant_a.get())
    .bind(received.receipt.id)
    .bind(owner_a.get())
    .bind(facility_a.get())
    .bind(&received.receipt.payload_sha256)
    .execute(&mut *forged_key_tx)
    .await;
    forged_key_tx.rollback().await.unwrap();
    assert!(forged_key.is_err());

    let admin_db = admin_db_for(&fixture.db).await;
    let mut admin_tx = admin_db.begin().await.unwrap();
    db::bind_tenant_context(&mut admin_tx, tenant_a)
        .await
        .unwrap();
    assert!(
        sqlx::query("UPDATE integration_inbox_receipts SET raw_payload = $1 WHERE id = $2")
            .bind(b"admin changed".as_slice())
            .bind(received.receipt.id)
            .execute(&mut *admin_tx)
            .await
            .is_err()
    );
    admin_tx.rollback().await.unwrap();

    let mut admin_tx = admin_db.begin().await.unwrap();
    db::bind_tenant_context(&mut admin_tx, tenant_a)
        .await
        .unwrap();
    assert!(
        sqlx::query("DELETE FROM integration_inbox_keys WHERE receipt_id = $1")
            .bind(received.receipt.id)
            .execute(&mut *admin_tx)
            .await
            .is_err()
    );
    admin_tx.rollback().await.unwrap();

    let mut archive_tx = admin_db.begin().await.unwrap();
    db::bind_tenant_context(&mut archive_tx, tenant_a)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query("DELETE FROM integration_inbox_receipts WHERE id = $1")
            .bind(received.receipt.id)
            .execute(&mut *archive_tx)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    archive_tx.commit().await.unwrap();

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let retained_keys: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM integration_inbox_keys WHERE receipt_id = $1")
            .bind(received.receipt.id)
            .fetch_one(&mut *tenant_a_tx)
            .await
            .unwrap();
    tenant_a_tx.rollback().await.unwrap();
    assert_eq!(retained_keys, 1);

    let archived_replay = integration_inbox::receive(
        &fixture.db,
        &NewIntegrationInboxReceipt {
            tenant_id: tenant_a,
            inventory_owner_id: Some(owner_a),
            facility_id: Some(facility_a),
            owner_mapping: None,
            source_key: "tenant-a-source",
            deduplication_key: "tenant-a-message",
            content_type: "text/plain",
            raw_payload: b"tenant a",
            request_id: Some("archived-retry"),
        },
    )
    .await;
    assert!(matches!(
        archived_replay,
        Err(PersistenceError::Conflict(_))
    ));

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let archived_counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM integration_inbox_receipts
                WHERE source_key = 'tenant-a-source'
                  AND deduplication_key = 'tenant-a-message'),
               (SELECT COUNT(*) FROM integration_inbox_keys
                WHERE source_key = 'tenant-a-source'
                  AND deduplication_key = 'tenant-a-message')
        "#,
    )
    .fetch_one(&mut *tenant_a_tx)
    .await
    .unwrap();
    tenant_a_tx.rollback().await.unwrap();
    assert_eq!(archived_counts, (0, 1));
    admin_db.close().await;
}
