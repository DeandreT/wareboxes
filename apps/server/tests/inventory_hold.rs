mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use tokio::sync::Barrier;
use tokio::time::timeout;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryHoldReason, InventoryHoldStatus, TenantAccess};

fn command_context(access: &TenantAccess, key: &str) -> CommandContext {
    CommandContext {
        tenant_id: access.tenant_id,
        actor_id: access.user_id,
        request_id: format!("request-{key}"),
        idempotency_key: Some(key.to_owned()),
    }
}

fn assert_boundary_rejection(error: AppError) {
    assert!(
        matches!(
            error,
            AppError::Application(
                ApplicationError::Conflict(_)
                    | ApplicationError::Forbidden
                    | ApplicationError::NotFound(_)
            )
        ),
        "unexpected inventory hold boundary error: {error:?}"
    );
}

async fn balance_quantities(db: &db::Db, tenant_id: TenantId, balance_id: i64) -> (i64, i64, i64) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let quantities = sqlx::query_as(
        r#"
        SELECT qty_on_hand, qty_reserved, qty_held
        FROM inventory_balances
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    quantities
}

async fn journal_counts(db: &db::Db, tenant_id: TenantId) -> (i64, i64) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let counts = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(*) FROM inventory_transactions WHERE tenant_id = $1),
            (SELECT COUNT(*) FROM inventory_entries WHERE tenant_id = $1)
        "#,
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    counts
}

async fn assert_commitments_reconciled(
    db: &db::Db,
    access: &TenantAccess,
    inventory_balance_id: i64,
) {
    assert!(
        repo::inventory::get_inventory_hold_reconciliation_issues_in_scope(db, access)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repo::inventory::get_reconciliation_issues(db, access.tenant_id)
            .await
            .unwrap()
            .is_empty()
    );
    let mut tx = tenant_tx(db, access.tenant_id).await;
    let (projected_reserved, active_allocations, projected_held, active_holds): (
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        r#"
        SELECT balance.qty_reserved,
               COALESCE((
                   SELECT SUM(allocation.qty)
                   FROM inventory_allocations allocation
                   WHERE allocation.tenant_id = balance.tenant_id
                     AND allocation.inventory_balance_id = balance.id
                     AND allocation.deleted IS NULL
                     AND allocation.status = 'allocated'
               ), 0)::BIGINT,
               balance.qty_held,
               COALESCE((
                   SELECT SUM(hold.qty)
                   FROM inventory_holds hold
                   WHERE hold.tenant_id = balance.tenant_id
                     AND hold.inventory_balance_id = balance.id
                     AND hold.deleted IS NULL
                     AND hold.status = 'active'
               ), 0)::BIGINT
        FROM inventory_balances balance
        WHERE balance.tenant_id = $1 AND balance.id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(inventory_balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(projected_reserved, active_allocations);
    assert_eq!(projected_held, active_holds);
}

#[tokio::test]
async fn quantity_holds_cap_stock_are_replay_safe_and_release_fully() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("inventory-hold@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let tenant_id = access.tenant_id;
    let owner_id = fixture.inventory_owner(tenant_id, "Hold Owner").await;
    let facility_id = fixture.facility(tenant_id, "Hold DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture.item(tenant_id, "Hold Item", "each").await;
    let balance_a = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 10,
                key: "HOLD-A",
            },
        )
        .await;
    let balance_b = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 10,
                key: "HOLD-B",
            },
        )
        .await;
    let destination_id = fixture
        .location(tenant_id, facility_id, "HOLD-DESTINATION")
        .await;

    let order_a = fixture.order(tenant_id, "HOLD-ORDER-A", owner_id).await;
    fixture
        .allocated_reservation(
            tenant_id,
            user.id,
            order_a,
            balance_a.balance_id,
            3,
            "hold-allocation-a",
        )
        .await;
    let journal_before_hold = journal_counts(&fixture.db, tenant_id).await;
    let place_context = command_context(&access, "place-hold-a");
    let place_command = repo::inventory::PlaceInventoryHoldCommand {
        inventory_balance_id: balance_a.balance_id,
        qty: 7,
        reason: InventoryHoldReason::QualityInspection,
        note: Some("awaiting inspection"),
        reference_type: Some("receipt"),
        reference_id: Some(41),
    };
    let placed =
        repo::inventory::place_inventory_hold(&fixture.db, &access, &place_context, &place_command)
            .await
            .unwrap();
    assert_eq!(
        repo::inventory::place_inventory_hold(&fixture.db, &access, &place_context, &place_command)
            .await
            .unwrap(),
        placed
    );
    let changed_retry = repo::inventory::place_inventory_hold(
        &fixture.db,
        &access,
        &place_context,
        &repo::inventory::PlaceInventoryHoldCommand {
            qty: 6,
            ..place_command
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        changed_retry,
        AppError::Application(ApplicationError::IdempotencyKeyReused)
    ));
    assert_eq!(
        balance_quantities(&fixture.db, tenant_id, balance_a.balance_id).await,
        (10, 3, 7)
    );
    assert_boundary_rejection(
        repo::inventory::place_inventory_hold(
            &fixture.db,
            &access,
            &command_context(&access, "place-over-capacity"),
            &repo::inventory::PlaceInventoryHoldCommand {
                inventory_balance_id: balance_a.balance_id,
                qty: 1,
                reason: InventoryHoldReason::Regulatory,
                note: None,
                reference_type: None,
                reference_id: None,
            },
        )
        .await
        .unwrap_err(),
    );
    assert_boundary_rejection(
        repo::inventory::move_inventory(
            &fixture.db,
            tenant_id,
            user.id,
            balance_a.item_batch_id,
            balance_a.location_id,
            destination_id,
            1,
            None,
            None,
            None,
            None,
            "move-held-capacity",
        )
        .await
        .unwrap_err(),
    );

    let other_user = fixture.wms_user("inventory-hold-other@test.local").await;
    let other_access = default_tenant_for_user(&fixture.db, other_user.id)
        .await
        .unwrap();
    assert_boundary_rejection(
        repo::inventory::place_inventory_hold(
            &fixture.db,
            &other_access,
            &command_context(&other_access, "cross-tenant-hold"),
            &repo::inventory::PlaceInventoryHoldCommand {
                inventory_balance_id: balance_a.balance_id,
                qty: 1,
                reason: InventoryHoldReason::CustomerRequest,
                note: None,
                reference_type: None,
                reference_id: None,
            },
        )
        .await
        .unwrap_err(),
    );
    assert_boundary_rejection(
        repo::inventory::release_inventory_hold(
            &fixture.db,
            &other_access,
            &command_context(&other_access, "cross-tenant-release"),
            &repo::inventory::ReleaseInventoryHoldCommand {
                hold_id: placed.hold_id,
            },
        )
        .await
        .unwrap_err(),
    );

    let release_context = command_context(&access, "release-hold-a");
    let release_command = repo::inventory::ReleaseInventoryHoldCommand {
        hold_id: placed.hold_id,
    };
    let released = repo::inventory::release_inventory_hold(
        &fixture.db,
        &access,
        &release_context,
        &release_command,
    )
    .await
    .unwrap();
    assert_eq!(released.released_qty, 7);
    assert_eq!(
        repo::inventory::release_inventory_hold(
            &fixture.db,
            &access,
            &release_context,
            &release_command
        )
        .await
        .unwrap(),
        released
    );
    assert_boundary_rejection(
        repo::inventory::release_inventory_hold(
            &fixture.db,
            &access,
            &command_context(&access, "release-hold-a-with-new-key"),
            &release_command,
        )
        .await
        .unwrap_err(),
    );
    assert_eq!(
        journal_counts(&fixture.db, tenant_id).await,
        journal_before_hold
    );
    assert_eq!(
        balance_quantities(&fixture.db, tenant_id, balance_a.balance_id).await,
        (10, 3, 0)
    );
    let holds = repo::inventory::get_inventory_holds_in_scope(&fixture.db, &access, true)
        .await
        .unwrap();
    let released_hold = holds.iter().find(|hold| hold.id == placed.hold_id).unwrap();
    assert_eq!(released_hold.status, InventoryHoldStatus::Released);
    assert_eq!(released_hold.qty, 7);
    assert!(released_hold.deleted.is_some());

    repo::inventory::move_inventory(
        &fixture.db,
        tenant_id,
        user.id,
        balance_a.item_batch_id,
        balance_a.location_id,
        destination_id,
        7,
        None,
        None,
        None,
        None,
        "move-after-full-hold-release",
    )
    .await
    .unwrap();

    let place_b = repo::inventory::place_inventory_hold(
        &fixture.db,
        &access,
        &command_context(&access, "place-hold-b"),
        &repo::inventory::PlaceInventoryHoldCommand {
            inventory_balance_id: balance_b.balance_id,
            qty: 6,
            reason: InventoryHoldReason::DamageSuspected,
            note: Some("packaging damage"),
            reference_type: None,
            reference_id: None,
        },
    )
    .await
    .unwrap();
    let order_b = fixture.order(tenant_id, "HOLD-ORDER-B", owner_id).await;
    let reservation_b = fixture
        .reservation_for_balance(
            tenant_id,
            user.id,
            order_b,
            balance_b.balance_id,
            5,
            "hold-reservation-b",
        )
        .await;
    assert_boundary_rejection(
        repo::inventory::allocate_inventory(
            &fixture.db,
            &access,
            &repo::inventory::AllocateInventoryCommand {
                reservation_id: reservation_b,
                inventory_balance_id: balance_b.balance_id,
                qty: 5,
                idempotency_key: "allocate-over-held-capacity",
            },
        )
        .await
        .unwrap_err(),
    );
    repo::inventory::allocate_inventory(
        &fixture.db,
        &access,
        &repo::inventory::AllocateInventoryCommand {
            reservation_id: reservation_b,
            inventory_balance_id: balance_b.balance_id,
            qty: 4,
            idempotency_key: "allocate-exact-held-capacity",
        },
    )
    .await
    .unwrap();
    assert_eq!(
        balance_quantities(&fixture.db, tenant_id, balance_b.balance_id).await,
        (10, 4, 6)
    );
    repo::inventory::release_inventory_hold(
        &fixture.db,
        &access,
        &command_context(&access, "release-hold-b"),
        &repo::inventory::ReleaseInventoryHoldCommand {
            hold_id: place_b.hold_id,
        },
    )
    .await
    .unwrap();
    assert_commitments_reconciled(&fixture.db, &access, balance_a.balance_id).await;
    assert_commitments_reconciled(&fixture.db, &access, balance_b.balance_id).await;
}

#[tokio::test]
async fn concurrent_hold_and_allocation_share_one_balance_capacity() {
    let fixture = Fixture::new().await;
    let user = fixture
        .wms_user("inventory-hold-allocation-race@test.local")
        .await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Hold Allocation Race Owner")
        .await;
    let facility_id = fixture
        .facility(access.tenant_id, "Hold Allocation Race DC")
        .await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture
        .item(access.tenant_id, "Hold Allocation Race Item", "each")
        .await;
    let balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 10,
                key: "HOLD-ALLOCATION-RACE",
            },
        )
        .await;
    let order_id = fixture
        .order(access.tenant_id, "HOLD-ALLOCATION-RACE-ORDER", owner_id)
        .await;
    let reservation_id = fixture
        .reservation_for_balance(
            access.tenant_id,
            user.id,
            order_id,
            balance.balance_id,
            6,
            "hold-allocation-race-reservation",
        )
        .await;

    let barrier = Arc::new(Barrier::new(3));
    let hold_db = fixture.db.clone();
    let hold_access = access.clone();
    let hold_barrier = Arc::clone(&barrier);
    let hold_attempt = tokio::spawn(async move {
        hold_barrier.wait().await;
        repo::inventory::place_inventory_hold(
            &hold_db,
            &hold_access,
            &command_context(&hold_access, "hold-allocation-race-hold"),
            &repo::inventory::PlaceInventoryHoldCommand {
                inventory_balance_id: balance.balance_id,
                qty: 6,
                reason: InventoryHoldReason::InventoryDiscrepancy,
                note: None,
                reference_type: None,
                reference_id: None,
            },
        )
        .await
    });
    let allocation_db = fixture.db.clone();
    let allocation_access = access.clone();
    let allocation_barrier = Arc::clone(&barrier);
    let allocation_attempt = tokio::spawn(async move {
        allocation_barrier.wait().await;
        repo::inventory::allocate_inventory(
            &allocation_db,
            &allocation_access,
            &repo::inventory::AllocateInventoryCommand {
                reservation_id,
                inventory_balance_id: balance.balance_id,
                qty: 6,
                idempotency_key: "hold-allocation-race-allocation",
            },
        )
        .await
    });
    barrier.wait().await;
    let (hold_result, allocation_result) = timeout(Duration::from_secs(2), async {
        (
            hold_attempt.await.unwrap(),
            allocation_attempt.await.unwrap(),
        )
    })
    .await
    .expect("hold and allocation serialize");

    match (hold_result, allocation_result) {
        (Ok(_), Err(error)) | (Err(error), Ok(_)) => assert_boundary_rejection(error),
        outcomes => panic!("expected exactly one capacity winner: {outcomes:?}"),
    }
    let (on_hand, reserved, held) =
        balance_quantities(&fixture.db, access.tenant_id, balance.balance_id).await;
    assert_eq!(on_hand, 10);
    assert!(matches!((reserved, held), (6, 0) | (0, 6)));
    assert!(reserved + held <= on_hand);
    assert_commitments_reconciled(&fixture.db, &access, balance.balance_id).await;
}

#[tokio::test]
async fn concurrent_holds_cannot_overcommit_one_balance() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("inventory-hold-race@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Hold Race Owner")
        .await;
    let facility_id = fixture.facility(access.tenant_id, "Hold Race DC").await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let item_id = fixture
        .item(access.tenant_id, "Hold Race Item", "each")
        .await;
    let balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                item_id,
                qty: 10,
                key: "HOLD-RACE",
            },
        )
        .await;

    let barrier = Arc::new(Barrier::new(3));
    let mut attempts = Vec::new();
    for key in ["hold-race-a", "hold-race-b"] {
        let db = fixture.db.clone();
        let access = access.clone();
        let barrier = Arc::clone(&barrier);
        attempts.push(tokio::spawn(async move {
            barrier.wait().await;
            (
                key,
                repo::inventory::place_inventory_hold(
                    &db,
                    &access,
                    &command_context(&access, key),
                    &repo::inventory::PlaceInventoryHoldCommand {
                        inventory_balance_id: balance.balance_id,
                        qty: 6,
                        reason: InventoryHoldReason::Regulatory,
                        note: None,
                        reference_type: None,
                        reference_id: None,
                    },
                )
                .await,
            )
        }));
    }
    barrier.wait().await;
    let results = timeout(Duration::from_secs(2), async {
        let first = attempts.remove(0).await.unwrap();
        let second = attempts.remove(0).await.unwrap();
        [first, second]
    })
    .await
    .expect("competing holds serialize");
    let mut accepted = Vec::new();
    for (key, result) in results {
        match result {
            Ok(result) => accepted.push((key, result)),
            Err(error) => assert_boundary_rejection(error),
        }
    }
    assert_eq!(accepted.len(), 1);
    let (accepted_key, accepted_hold) = &accepted[0];
    assert_eq!(
        repo::inventory::place_inventory_hold(
            &fixture.db,
            &access,
            &command_context(&access, accepted_key),
            &repo::inventory::PlaceInventoryHoldCommand {
                inventory_balance_id: balance.balance_id,
                qty: 6,
                reason: InventoryHoldReason::Regulatory,
                note: None,
                reference_type: None,
                reference_id: None,
            },
        )
        .await
        .unwrap(),
        *accepted_hold
    );
    assert_eq!(
        balance_quantities(&fixture.db, access.tenant_id, balance.balance_id).await,
        (10, 0, 6)
    );
    let active_holds = repo::inventory::get_inventory_holds_in_scope(&fixture.db, &access, false)
        .await
        .unwrap();
    assert_eq!(active_holds.len(), 1);
    assert_eq!(active_holds[0].id, accepted_hold.hold_id);
    assert_commitments_reconciled(&fixture.db, &access, balance.balance_id).await;
}
