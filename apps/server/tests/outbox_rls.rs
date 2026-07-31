mod common;

use common::*;
use serde_json::json;
use wareboxes_api::repo::outbox::{self, NewOutboxEvent};
use wareboxes_domain::{FacilityId, InventoryOwnerId};

#[derive(Clone)]
struct EventRefs {
    tenant_id: TenantId,
    event_id: i64,
    event_key: String,
    ordering_key: String,
}

#[tokio::test]
async fn outbox_storage_and_workers_are_tenant_isolated() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("outbox-rls-a@test.com").await;
    let user_b = fixture.user("outbox-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let refs_a = event_refs(&fixture, tenant_a, user_a.id, "outbox-rls-a").await;
    let refs_b = event_refs(&fixture, tenant_b, user_b.id, "outbox-rls-b").await;
    let source_a = snapshot(&fixture.db, &refs_a).await;

    let unbound_counts: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM outbox_event_keys),
               (SELECT COUNT(*) FROM outbox_aggregate_sequences),
               (SELECT COUNT(*) FROM outbox_events)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0, 0));

    let mut unbound_connection = fixture.db.acquire().await.unwrap();
    assert_eq!(
        guessed_mutation_counts(&mut unbound_connection, &refs_a).await,
        [0, 0, 0, 0, 0, 0]
    );
    drop(unbound_connection);
    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(
        insert_event_key(&mut unbound_tx, tenant_a, "unbound-forged-key")
            .await
            .is_err()
    );
    unbound_tx.rollback().await.unwrap();
    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(
        insert_aggregate_sequence(&mut unbound_tx, tenant_a, "unbound-forged-ordering")
            .await
            .is_err()
    );
    unbound_tx.rollback().await.unwrap();
    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_event(
        &mut unbound_tx,
        tenant_a,
        "unbound-forged-event",
        "unbound-forged-ordering"
    )
    .await
    .is_err());
    unbound_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let visible_counts: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM outbox_event_keys),
               (SELECT COUNT(*) FROM outbox_aggregate_sequences),
               (SELECT COUNT(*) FROM outbox_events)
        "#,
    )
    .fetch_one(&mut *tenant_b_tx)
    .await
    .unwrap();
    assert_eq!(visible_counts, (1, 1, 1));
    assert_eq!(
        guessed_mutation_counts(&mut tenant_b_tx, &refs_a).await,
        [0, 0, 0, 0, 0, 0]
    );
    tenant_b_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_event_key(&mut tenant_b_tx, tenant_a, "forged-key")
        .await
        .is_err());
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(
        insert_aggregate_sequence(&mut tenant_b_tx, tenant_a, "forged-ordering")
            .await
            .is_err()
    );
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_event(
        &mut tenant_b_tx,
        tenant_a,
        "forged-event",
        "forged-ordering"
    )
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let cross_context_payload = json!({"forged": true});
    assert!(outbox::enqueue(
        &mut tenant_b_tx,
        &NewOutboxEvent {
            tenant_id: tenant_a,
            inventory_owner_id: None,
            facility_id: None,
            actor_user_id: None,
            event_key: "cross-context-event",
            aggregate_type: "outbox_rls_test",
            aggregate_id: "cross-context-event",
            ordering_key: "cross-context-event",
            aggregate_sequence: 1,
            event_type: "outbox.rls.cross_context",
            schema_version: 1,
            payload: &cross_context_payload,
            occurred_at: db::now_iso(),
        },
    )
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();
    assert_eq!(snapshot(&fixture.db, &refs_a).await, source_a);

    let claimed_a =
        outbox::claim_events(&fixture.db, tenant_a, "worker-a", "test-publisher", 10, 60)
            .await
            .unwrap();
    let claimed_b =
        outbox::claim_events(&fixture.db, tenant_b, "worker-b", "test-publisher", 10, 60)
            .await
            .unwrap();
    assert_eq!(
        claimed_a.iter().map(|event| event.id).collect::<Vec<_>>(),
        vec![refs_a.event_id]
    );
    assert_eq!(
        claimed_b.iter().map(|event| event.id).collect::<Vec<_>>(),
        vec![refs_b.event_id]
    );

    assert!(outbox::mark_failed(
        &fixture.db,
        &outbox::FailOutboxEvent {
            tenant_id: tenant_a,
            event_id: refs_a.event_id,
            worker_id: "worker-a",
            claim_version: claimed_a[0].claim_version,
            failure_class: outbox::DeliveryFailureClass::Permanent,
            error: "dead-letter isolation test",
            retry_after_seconds: 0,
            max_attempts: 1,
        },
    )
    .await
    .unwrap());
    assert!(
        !outbox::replay_dead_letter(&fixture.db, tenant_b, refs_a.event_id)
            .await
            .unwrap()
    );
    assert!(!outbox::discard_dead_letter(
        &fixture.db,
        tenant_b,
        refs_a.event_id,
        user_b.id,
        "wrong tenant",
    )
    .await
    .unwrap());
    assert!(
        outbox::replay_dead_letter(&fixture.db, tenant_a, refs_a.event_id)
            .await
            .unwrap()
    );

    let replayed_a =
        outbox::claim_events(&fixture.db, tenant_a, "replay-a", "test-publisher", 1, 60)
            .await
            .unwrap();
    assert_eq!(replayed_a[0].id, refs_a.event_id);
    assert!(outbox::mark_published(
        &fixture.db,
        tenant_a,
        refs_a.event_id,
        "replay-a",
        replayed_a[0].claim_version,
    )
    .await
    .unwrap());
    assert!(outbox::mark_published(
        &fixture.db,
        tenant_b,
        refs_b.event_id,
        "worker-b",
        claimed_b[0].claim_version,
    )
    .await
    .unwrap());

    assert_eq!(
        outbox::purge_published(&fixture.db, tenant_a, 0, 100)
            .await
            .unwrap(),
        1
    );
    assert!(outbox::get_events(&fixture.db, tenant_a, None, 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        outbox::get_events(&fixture.db, tenant_b, None, 10)
            .await
            .unwrap()
            .iter()
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        vec![refs_b.event_id]
    );
    let retained_a: (i64, i64) = {
        let mut tx = tenant_tx(&fixture.db, tenant_a).await;
        let counts = sqlx::query_as(
            r#"
            SELECT (SELECT COUNT(*) FROM outbox_event_keys),
                   (SELECT COUNT(*) FROM outbox_aggregate_sequences)
            "#,
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        counts
    };
    assert_eq!(retained_a, (1, 1));
}

async fn event_refs(fixture: &Fixture, tenant_id: TenantId, user_id: i64, key: &str) -> EventRefs {
    let inventory_owner_id = fixture.inventory_owner(tenant_id, key).await;
    let facility_id = fixture.facility(tenant_id, key).await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let event_key = format!("{key}-event");
    let ordering_key = format!("{key}-ordering");
    let payload = json!({"key": key});
    let mut tx = fixture.db.begin().await.unwrap();
    let event_id = outbox::enqueue(
        &mut tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(InventoryOwnerId::new(inventory_owner_id).unwrap()),
            facility_id: Some(FacilityId::new(facility_id).unwrap()),
            actor_user_id: Some(user_id),
            event_key: &event_key,
            aggregate_type: "outbox_rls_test",
            aggregate_id: key,
            ordering_key: &ordering_key,
            aggregate_sequence: 1,
            event_type: "outbox.rls.test",
            schema_version: 1,
            payload: &payload,
            occurred_at: db::now_iso(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    EventRefs {
        tenant_id,
        event_id,
        event_key,
        ordering_key,
    }
}

async fn guessed_mutation_counts(
    connection: &mut sqlx::PgConnection,
    refs: &EventRefs,
) -> [u64; 6] {
    let key_updates =
        sqlx::query("UPDATE outbox_event_keys SET created = created WHERE event_key = $1")
            .bind(&refs.event_key)
            .execute(&mut *connection)
            .await
            .unwrap()
            .rows_affected();
    let sequence_updates = sqlx::query(
        "UPDATE outbox_aggregate_sequences SET updated = updated WHERE ordering_key = $1",
    )
    .bind(&refs.ordering_key)
    .execute(&mut *connection)
    .await
    .unwrap()
    .rows_affected();
    let event_updates =
        sqlx::query("UPDATE outbox_events SET available_at = available_at WHERE id = $1")
            .bind(refs.event_id)
            .execute(&mut *connection)
            .await
            .unwrap()
            .rows_affected();
    let event_deletes = sqlx::query("DELETE FROM outbox_events WHERE id = $1")
        .bind(refs.event_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let sequence_deletes =
        sqlx::query("DELETE FROM outbox_aggregate_sequences WHERE ordering_key = $1")
            .bind(&refs.ordering_key)
            .execute(&mut *connection)
            .await
            .unwrap()
            .rows_affected();
    let key_deletes = sqlx::query("DELETE FROM outbox_event_keys WHERE event_key = $1")
        .bind(&refs.event_key)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    [
        key_updates,
        sequence_updates,
        event_updates,
        event_deletes,
        sequence_deletes,
        key_deletes,
    ]
}

async fn insert_event_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    event_key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO outbox_event_keys (tenant_id, event_key, created) VALUES ($1, $2, $3)")
        .bind(tenant_id.get())
        .bind(event_key)
        .bind(db::now_iso())
        .execute(&mut **tx)
        .await
        .map(|_| ())
}

async fn insert_aggregate_sequence(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    ordering_key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO outbox_aggregate_sequences
            (tenant_id, ordering_key, last_sequence, updated)
        VALUES ($1, $2, 1, $3)
        "#,
    )
    .bind(tenant_id.get())
    .bind(ordering_key)
    .bind(db::now_iso())
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

async fn insert_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    event_key: &str,
    ordering_key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO outbox_events
            (tenant_id, created, event_key, aggregate_type, aggregate_id,
             ordering_key, aggregate_sequence, event_type, schema_version,
             payload, occurred_at, available_at)
        VALUES ($1, $2, $3, 'outbox_rls_test', $3, $4, 1,
                'outbox.rls.forged', 1, '{}'::JSONB, $2, $2)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(event_key)
    .bind(ordering_key)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

async fn snapshot(db: &db::Db, refs: &EventRefs) -> String {
    let mut tx = tenant_tx(db, refs.tenant_id).await;
    let snapshot = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'event_key', (
                SELECT to_jsonb(key_row)
                FROM outbox_event_keys key_row
                WHERE event_key = $1
            ),
            'sequence', (
                SELECT to_jsonb(sequence_row)
                FROM outbox_aggregate_sequences sequence_row
                WHERE ordering_key = $2
            ),
            'event', (
                SELECT to_jsonb(event_row)
                FROM outbox_events event_row
                WHERE id = $3
            )
        )::TEXT
        "#,
    )
    .bind(&refs.event_key)
    .bind(&refs.ordering_key)
    .bind(refs.event_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    snapshot
}
