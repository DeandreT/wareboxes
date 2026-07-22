mod common;

use std::collections::BTreeSet;

use common::*;
use serde_json::json;
use wareboxes_domain::{FacilityId, InventoryOwnerId};
use wareboxes_server::repo::outbox::{self, NewOutboxEvent};

async fn enqueue_test_event(
    db: &db::Db,
    tenant_id: TenantId,
    user_id: i64,
    owner_id: i64,
    facility_id: i64,
    event: (&str, &str, i64),
) -> i64 {
    let (event_key, ordering_key, aggregate_sequence) = event;
    let mut tx = db.begin().await.unwrap();
    let payload = json!({"event_key": event_key});
    let aggregate_id = event_key.to_string();
    let id = outbox::enqueue(
        &mut tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(InventoryOwnerId::new(owner_id).unwrap()),
            facility_id: Some(FacilityId::new(facility_id).unwrap()),
            actor_user_id: Some(user_id),
            event_key,
            aggregate_type: "outbox_test",
            aggregate_id: &aggregate_id,
            ordering_key,
            aggregate_sequence,
            event_type: "outbox.test.created",
            schema_version: 1,
            payload: &payload,
            occurred_at: db::now_iso(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

#[tokio::test]
async fn domain_events_are_atomic_immutable_and_replay_safe() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("outbox-producer@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let owner_id = fixture.inventory_owner(tenant_id, "Outbox Owner").await;
    let facility_id = fixture.facility(tenant_id, "Outbox DC").await;
    let location_id = fixture
        .location(tenant_id, facility_id, "OUTBOX-RECEIVING")
        .await;
    let item_id = fixture.item(tenant_id, "Outbox Item", "each").await;
    let batch_id = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        owner_id,
        item_id,
        None,
        Some("OUTBOX-LOT"),
        None,
        None,
    )
    .await
    .unwrap();

    let transaction_id = repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        user.id,
        batch_id,
        location_id,
        12,
        None,
        None,
        None,
        None,
        Some("outbox-receipt"),
    )
    .await
    .unwrap();
    let replayed_transaction_id = repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        user.id,
        batch_id,
        location_id,
        12,
        None,
        None,
        None,
        None,
        Some("outbox-receipt"),
    )
    .await
    .unwrap();
    assert_eq!(replayed_transaction_id, transaction_id);

    let balance_id = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap()[0]
        .id;
    let order_id = fixture.order(tenant_id, "OUTBOX-ORDER", owner_id).await;
    let reservation_id = repo::inventory::reserve_inventory(
        &fixture.db,
        &repo::inventory::ReserveInventoryCommand {
            tenant_id,
            actor_user_id: user.id,
            order_id,
            order_item_id: None,
            inventory_balance_id: balance_id,
            qty: 4,
            idempotency_key: "outbox-reservation",
        },
    )
    .await
    .unwrap();
    assert_eq!(
        repo::inventory::reserve_inventory(
            &fixture.db,
            &repo::inventory::ReserveInventoryCommand {
                tenant_id,
                actor_user_id: user.id,
                order_id,
                order_item_id: None,
                inventory_balance_id: balance_id,
                qty: 4,
                idempotency_key: "outbox-reservation",
            },
        )
        .await
        .unwrap(),
        reservation_id
    );
    assert!(repo::inventory::cancel_reservation(
        &fixture.db,
        &repo::inventory::CancelReservationCommand {
            tenant_id,
            actor_user_id: user.id,
            reservation_id,
            idempotency_key: "outbox-cancellation",
        },
    )
    .await
    .unwrap());
    assert!(repo::inventory::cancel_reservation(
        &fixture.db,
        &repo::inventory::CancelReservationCommand {
            tenant_id,
            actor_user_id: user.id,
            reservation_id,
            idempotency_key: "outbox-cancellation",
        },
    )
    .await
    .unwrap());

    let events = outbox::get_events(&fixture.db, tenant_id, None, 100)
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "inventory.reservation.cancelled",
            "inventory.reservation.created",
            "inventory.transaction.recorded",
        ])
    );
    assert!(events.iter().all(|event| {
        event.tenant_id == tenant_id
            && event.inventory_owner_id.map(InventoryOwnerId::get) == Some(owner_id)
            && event.facility_id.map(FacilityId::get) == Some(facility_id)
            && event.actor_user_id == Some(user.id)
            && event.schema_version == 1
            && event.payload.is_object()
    }));
    let transaction_event = events
        .iter()
        .find(|event| event.event_type == "inventory.transaction.recorded")
        .unwrap();
    assert_eq!(
        transaction_event.payload["inventory_transaction_id"],
        transaction_id
    );
    let first_page = outbox::get_events(&fixture.db, tenant_id, None, 2)
        .await
        .unwrap();
    let second_page = outbox::get_events(
        &fixture.db,
        tenant_id,
        Some(first_page.last().unwrap().id),
        2,
    )
    .await
    .unwrap();
    assert_eq!(first_page.len(), 2);
    assert_eq!(second_page.len(), 1);

    let other_user = fixture.user("outbox-other-tenant@test.com").await;
    let other_tenant = tenant_for_user(&fixture.db, other_user.id).await;
    let (ordering_a, ordering_b) = tokio::join!(
        outbox::claim_events(&fixture.db, "ordering-worker-a", 10, 60),
        outbox::claim_events(&fixture.db, "ordering-worker-b", 10, 60),
    );
    let ordering_a = ordering_a.unwrap();
    let ordering_b = ordering_b.unwrap();
    let first_claims = ordering_a.iter().chain(&ordering_b).collect::<Vec<_>>();
    assert_eq!(first_claims.len(), 2);
    assert!(first_claims
        .iter()
        .all(|event| event.event_type != "inventory.reservation.cancelled"));
    let guarded_event = first_claims[0];
    assert!(!outbox::mark_published(
        &fixture.db,
        other_tenant,
        guarded_event.id,
        guarded_event.claimed_by.as_deref().unwrap(),
        guarded_event.claim_version,
    )
    .await
    .unwrap());
    assert!(!outbox::mark_failed(
        &fixture.db,
        &outbox::FailOutboxEvent {
            tenant_id: other_tenant,
            event_id: guarded_event.id,
            worker_id: guarded_event.claimed_by.as_deref().unwrap(),
            claim_version: guarded_event.claim_version,
            error: "wrong tenant",
            retry_after_seconds: 0,
            max_attempts: 3,
        },
    )
    .await
    .unwrap());
    for event in &ordering_a {
        assert!(outbox::mark_published(
            &fixture.db,
            tenant_id,
            event.id,
            "ordering-worker-a",
            event.claim_version,
        )
        .await
        .unwrap());
    }
    for event in &ordering_b {
        assert!(outbox::mark_published(
            &fixture.db,
            tenant_id,
            event.id,
            "ordering-worker-b",
            event.claim_version,
        )
        .await
        .unwrap());
    }
    let second_claim = outbox::claim_events(&fixture.db, "ordering-worker-c", 10, 60)
        .await
        .unwrap();
    assert_eq!(second_claim.len(), 1);
    assert_eq!(
        second_claim[0].event_type,
        "inventory.reservation.cancelled"
    );
    assert!(outbox::mark_published(
        &fixture.db,
        tenant_id,
        second_claim[0].id,
        "ordering-worker-c",
        second_claim[0].claim_version,
    )
    .await
    .unwrap());

    let mut rolled_back = fixture.db.begin().await.unwrap();
    let rollback_payload = json!({"rolled_back": true});
    outbox::enqueue(
        &mut rolled_back,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(InventoryOwnerId::new(owner_id).unwrap()),
            facility_id: Some(FacilityId::new(facility_id).unwrap()),
            actor_user_id: Some(user.id),
            event_key: "rolled-back-event",
            aggregate_type: "outbox_test",
            aggregate_id: "rolled-back-event",
            ordering_key: "rolled-back-event",
            aggregate_sequence: 1,
            event_type: "outbox.test.rolled_back",
            schema_version: 1,
            payload: &rollback_payload,
            occurred_at: db::now_iso(),
        },
    )
    .await
    .unwrap();
    rolled_back.rollback().await.unwrap();
    assert_eq!(
        outbox::get_events(&fixture.db, tenant_id, None, 100)
            .await
            .unwrap()
            .len(),
        3
    );

    assert!(
        sqlx::query("UPDATE outbox_events SET event_type = 'tampered' WHERE id = $1")
            .bind(events[0].id)
            .execute(&fixture.db)
            .await
            .is_err()
    );

    let other_owner = fixture
        .inventory_owner(other_tenant, "Other Tenant Owner")
        .await;
    let other_facility = fixture.facility(other_tenant, "Other Tenant DC").await;
    enqueue_test_event(
        &fixture.db,
        other_tenant,
        other_user.id,
        other_owner,
        other_facility,
        (
            &transaction_event.event_key,
            &transaction_event.event_key,
            1,
        ),
    )
    .await;
    assert_eq!(
        outbox::get_events(&fixture.db, tenant_id, None, 100)
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        outbox::get_events(&fixture.db, other_tenant, None, 100)
            .await
            .unwrap()
            .len(),
        1
    );

    let mut invalid_dimensions = fixture.db.begin().await.unwrap();
    let invalid_payload = json!({"invalid": true});
    assert!(outbox::enqueue(
        &mut invalid_dimensions,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(InventoryOwnerId::new(other_owner).unwrap()),
            facility_id: Some(FacilityId::new(other_facility).unwrap()),
            actor_user_id: Some(other_user.id),
            event_key: "cross-tenant-dimensions",
            aggregate_type: "outbox_test",
            aggregate_id: "cross-tenant-dimensions",
            ordering_key: "cross-tenant-dimensions",
            aggregate_sequence: 1,
            event_type: "outbox.test.invalid",
            schema_version: 1,
            payload: &invalid_payload,
            occurred_at: db::now_iso(),
        },
    )
    .await
    .is_err());
    invalid_dimensions.rollback().await.unwrap();
}

#[tokio::test]
async fn workers_claim_retry_and_recover_outbox_events_once_per_lease() {
    let fixture = Fixture::new().await;
    let user = fixture.user("outbox-worker@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let owner_id = fixture.inventory_owner(tenant_id, "Worker Owner").await;
    let facility_id = fixture.facility(tenant_id, "Worker DC").await;

    for event_number in 1..=6 {
        let event_key = format!("worker-event-{event_number}");
        enqueue_test_event(
            &fixture.db,
            tenant_id,
            user.id,
            owner_id,
            facility_id,
            (&event_key, &event_key, 1),
        )
        .await;
    }

    let (worker_a, worker_b) = tokio::join!(
        outbox::claim_events(&fixture.db, "worker-a", 3, 60),
        outbox::claim_events(&fixture.db, "worker-b", 3, 60),
    );
    let worker_a = worker_a.unwrap();
    let worker_b = worker_b.unwrap();
    assert_eq!(worker_a.len(), 3);
    assert_eq!(worker_b.len(), 3);
    let worker_a_ids = worker_a
        .iter()
        .map(|event| event.id)
        .collect::<BTreeSet<_>>();
    let worker_b_ids = worker_b
        .iter()
        .map(|event| event.id)
        .collect::<BTreeSet<_>>();
    assert!(worker_a_ids.is_disjoint(&worker_b_ids));

    let retry_event = &worker_a[0];
    assert!(!outbox::mark_failed(
        &fixture.db,
        &outbox::FailOutboxEvent {
            tenant_id,
            event_id: retry_event.id,
            worker_id: "worker-b",
            claim_version: retry_event.claim_version,
            error: "wrong worker",
            retry_after_seconds: 0,
            max_attempts: 3,
        },
    )
    .await
    .unwrap());
    assert!(outbox::mark_failed(
        &fixture.db,
        &outbox::FailOutboxEvent {
            tenant_id,
            event_id: retry_event.id,
            worker_id: "worker-a",
            claim_version: retry_event.claim_version,
            error: "temporary delivery failure",
            retry_after_seconds: 0,
            max_attempts: 3,
        },
    )
    .await
    .unwrap());

    for event in worker_a.iter().skip(1) {
        assert!(outbox::mark_published(
            &fixture.db,
            tenant_id,
            event.id,
            "worker-a",
            event.claim_version,
        )
        .await
        .unwrap());
    }
    for event in &worker_b {
        assert!(outbox::mark_published(
            &fixture.db,
            tenant_id,
            event.id,
            "worker-b",
            event.claim_version,
        )
        .await
        .unwrap());
    }

    let retried = outbox::claim_events(&fixture.db, "worker-c", 1, 60)
        .await
        .unwrap();
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].id, retry_event.id);
    assert_eq!(retried[0].attempts, 2);
    assert_eq!(
        retried[0].last_error.as_deref(),
        Some("temporary delivery failure")
    );
    assert!(!outbox::mark_published(
        &fixture.db,
        tenant_id,
        retried[0].id,
        "worker-a",
        retried[0].claim_version,
    )
    .await
    .unwrap());
    assert!(outbox::mark_published(
        &fixture.db,
        tenant_id,
        retried[0].id,
        "worker-c",
        retried[0].claim_version,
    )
    .await
    .unwrap());

    let stale_event_id = enqueue_test_event(
        &fixture.db,
        tenant_id,
        user.id,
        owner_id,
        facility_id,
        ("stale-worker-event", "stale-worker-event", 1),
    )
    .await;
    let stale_claim = outbox::claim_events(&fixture.db, "same-worker", 1, 60)
        .await
        .unwrap();
    assert_eq!(stale_claim[0].id, stale_event_id);
    assert!(outbox::claim_events(&fixture.db, "same-worker", 1, 1)
        .await
        .unwrap()
        .is_empty());
    sqlx::query(
        "UPDATE outbox_events SET lease_expires_at = clock_timestamp() - INTERVAL '1 second' WHERE id = $1",
    )
    .bind(stale_event_id)
    .execute(&fixture.db)
    .await
    .unwrap();
    let recovered = outbox::claim_events(&fixture.db, "same-worker", 1, 300)
        .await
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, stale_event_id);
    assert_eq!(recovered[0].attempts, 2);
    assert!(recovered[0].claim_version > stale_claim[0].claim_version);
    assert!(!outbox::mark_published(
        &fixture.db,
        tenant_id,
        stale_event_id,
        "same-worker",
        stale_claim[0].claim_version,
    )
    .await
    .unwrap());
    assert!(outbox::mark_published(
        &fixture.db,
        tenant_id,
        stale_event_id,
        "same-worker",
        recovered[0].claim_version,
    )
    .await
    .unwrap());

    let poison_event_id = enqueue_test_event(
        &fixture.db,
        tenant_id,
        user.id,
        owner_id,
        facility_id,
        ("poison-worker-event", "poison-worker-event", 1),
    )
    .await;
    let poison_claim = outbox::claim_events(&fixture.db, "poison-worker", 1, 60)
        .await
        .unwrap();
    assert_eq!(poison_claim[0].id, poison_event_id);
    assert!(outbox::mark_failed(
        &fixture.db,
        &outbox::FailOutboxEvent {
            tenant_id,
            event_id: poison_event_id,
            worker_id: "poison-worker",
            claim_version: poison_claim[0].claim_version,
            error: "permanent delivery failure",
            retry_after_seconds: 0,
            max_attempts: 1,
        },
    )
    .await
    .unwrap());
    assert!(outbox::claim_events(&fixture.db, "idle-worker", 10, 60)
        .await
        .unwrap()
        .is_empty());
    let poison_event = outbox::get_events(&fixture.db, tenant_id, Some(stale_event_id), 10)
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.id == poison_event_id)
        .unwrap();
    assert!(poison_event.dead_lettered_at.is_some());
    assert_eq!(poison_event.attempts, 1);
    assert!(
        outbox::replay_dead_letter(&fixture.db, tenant_id, poison_event_id)
            .await
            .unwrap()
    );
    let replayed_poison = outbox::claim_events(&fixture.db, "replay-worker", 1, 60)
        .await
        .unwrap();
    assert_eq!(replayed_poison[0].id, poison_event_id);
    assert_eq!(replayed_poison[0].attempts, 1);
    assert_eq!(replayed_poison[0].replay_count, 1);
    assert!(outbox::mark_published(
        &fixture.db,
        tenant_id,
        poison_event_id,
        "replay-worker",
        replayed_poison[0].claim_version,
    )
    .await
    .unwrap());

    let blocked_first_id = enqueue_test_event(
        &fixture.db,
        tenant_id,
        user.id,
        owner_id,
        facility_id,
        ("discard-sequence-1", "discard-ordering-key", 1),
    )
    .await;
    let blocked_second_id = enqueue_test_event(
        &fixture.db,
        tenant_id,
        user.id,
        owner_id,
        facility_id,
        ("discard-sequence-2", "discard-ordering-key", 2),
    )
    .await;
    let blocked_first = outbox::claim_events(&fixture.db, "discard-worker", 10, 60)
        .await
        .unwrap();
    assert_eq!(blocked_first.len(), 1);
    assert_eq!(blocked_first[0].id, blocked_first_id);
    assert!(outbox::mark_failed(
        &fixture.db,
        &outbox::FailOutboxEvent {
            tenant_id,
            event_id: blocked_first_id,
            worker_id: "discard-worker",
            claim_version: blocked_first[0].claim_version,
            error: "invalid external destination",
            retry_after_seconds: 0,
            max_attempts: 1,
        },
    )
    .await
    .unwrap());
    assert!(outbox::claim_events(&fixture.db, "blocked-worker", 10, 60)
        .await
        .unwrap()
        .is_empty());
    assert!(outbox::discard_dead_letter(
        &fixture.db,
        tenant_id,
        blocked_first_id,
        user.id,
        "destination was permanently decommissioned",
    )
    .await
    .unwrap());
    let unblocked = outbox::claim_events(&fixture.db, "unblocked-worker", 10, 60)
        .await
        .unwrap();
    assert_eq!(unblocked.len(), 1);
    assert_eq!(unblocked[0].id, blocked_second_id);
    assert!(outbox::mark_published(
        &fixture.db,
        tenant_id,
        blocked_second_id,
        "unblocked-worker",
        unblocked[0].claim_version,
    )
    .await
    .unwrap());

    let events = outbox::get_events(&fixture.db, tenant_id, None, 100)
        .await
        .unwrap();
    assert_eq!(events.len(), 10);
    assert!(events
        .iter()
        .all(|event| event.published_at.is_some() || event.discarded_at.is_some()));
    let discarded = events
        .iter()
        .find(|event| event.id == blocked_first_id)
        .unwrap();
    assert_eq!(discarded.discarded_by_user_id, Some(user.id));
    assert_eq!(
        discarded.discard_reason.as_deref(),
        Some("destination was permanently decommissioned")
    );
    assert!(outbox::claim_events(&fixture.db, "idle-worker", 10, 60)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        outbox::purge_published(&fixture.db, 0, 100).await.unwrap(),
        10
    );
    assert!(outbox::get_events(&fixture.db, tenant_id, None, 100)
        .await
        .unwrap()
        .is_empty());

    let retained_keys: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM outbox_event_keys WHERE tenant_id = $1")
            .bind(tenant_id.get())
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    assert_eq!(retained_keys, 10);

    let duplicate_payload = json!({"duplicate": true});
    let mut duplicate_key_tx = fixture.db.begin().await.unwrap();
    assert!(outbox::enqueue(
        &mut duplicate_key_tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(InventoryOwnerId::new(owner_id).unwrap()),
            facility_id: Some(FacilityId::new(facility_id).unwrap()),
            actor_user_id: Some(user.id),
            event_key: "worker-event-1",
            aggregate_type: "outbox_test",
            aggregate_id: "worker-event-1",
            ordering_key: "new-ordering-key",
            aggregate_sequence: 1,
            event_type: "outbox.test.duplicate",
            schema_version: 1,
            payload: &duplicate_payload,
            occurred_at: db::now_iso(),
        },
    )
    .await
    .is_err());
    duplicate_key_tx.rollback().await.unwrap();

    let mut old_sequence_tx = fixture.db.begin().await.unwrap();
    assert!(outbox::enqueue(
        &mut old_sequence_tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(InventoryOwnerId::new(owner_id).unwrap()),
            facility_id: Some(FacilityId::new(facility_id).unwrap()),
            actor_user_id: Some(user.id),
            event_key: "old-sequence-reinsert",
            aggregate_type: "outbox_test",
            aggregate_id: "old-sequence-reinsert",
            ordering_key: "discard-ordering-key",
            aggregate_sequence: 1,
            event_type: "outbox.test.old_sequence",
            schema_version: 1,
            payload: &duplicate_payload,
            occurred_at: db::now_iso(),
        },
    )
    .await
    .is_err());
    old_sequence_tx.rollback().await.unwrap();
}
