mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use tokio::sync::Barrier;
use tokio::time::timeout;
use wareboxes_core::models::{AllocationStatus, ReservationStatus};

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
        "unexpected inventory boundary error: {error:?}"
    );
}

async fn assert_allocation_reconciliation(db: &db::Db, tenant_id: TenantId) {
    let mut tx = tenant_tx(db, tenant_id).await;
    let mismatches: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM inventory_balances balance
        WHERE balance.tenant_id = $1
          AND balance.qty_reserved IS DISTINCT FROM COALESCE((
              SELECT SUM(allocation.qty)
              FROM inventory_allocations allocation
              WHERE allocation.tenant_id = balance.tenant_id
                AND allocation.inventory_balance_id = balance.id
                AND allocation.deleted IS NULL
                AND allocation.status = 'allocated'
          ), 0)
        "#,
    )
    .bind(tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let commitment_mismatches: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inventory_hold_reconciliation")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(mismatches, 0);
    assert_eq!(commitment_mismatches, 0);
    assert!(repo::inventory::get_reconciliation_issues(db, tenant_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn soft_reservations_and_concrete_allocations_preserve_demand_and_stock() {
    let fixture = Fixture::new().await;
    let user = fixture.wms_user("inventory-allocation@test.local").await;
    let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
    let tenant_id = access.tenant_id;
    let owner_id = fixture.inventory_owner(tenant_id, "Allocation Owner").await;
    let facility_id = fixture.facility(tenant_id, "Allocation DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let other_facility_id = fixture.facility(tenant_id, "Other Allocation DC").await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, other_facility_id)
        .await;
    let item_id = fixture.item(tenant_id, "Allocation Item", "each").await;
    let order_id = fixture.order(tenant_id, "ALLOCATION-ORDER", owner_id).await;
    let order_item_id = fixture.order_item(tenant_id, order_id, item_id, 20).await;

    let balance_a = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                key: "ALLOCATION-A",
                item_id,
                qty: 6,
            },
        )
        .await
        .balance_id;
    let balance_b = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id,
                key: "ALLOCATION-B",
                item_id,
                qty: 10,
            },
        )
        .await
        .balance_id;
    let other_facility_balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: owner_id,
                facility_id: other_facility_id,
                key: "ALLOCATION-OTHER-FACILITY",
                item_id,
                qty: 10,
            },
        )
        .await
        .balance_id;

    let other_owner_id = fixture
        .inventory_owner(tenant_id, "Other Allocation Owner")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, other_owner_id, facility_id)
        .await;
    let other_owner_balance = fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: other_owner_id,
                facility_id,
                key: "ALLOCATION-OTHER-OWNER",
                item_id,
                qty: 10,
            },
        )
        .await
        .balance_id;

    let other_user = fixture
        .wms_user("inventory-allocation-other@test.local")
        .await;
    let other_access = default_tenant_for_user(&fixture.db, other_user.id)
        .await
        .unwrap();
    let other_tenant_owner = fixture
        .inventory_owner(other_access.tenant_id, "Cross Tenant Allocation Owner")
        .await;
    let other_tenant_facility = fixture
        .facility(other_access.tenant_id, "Cross Tenant Allocation DC")
        .await;
    fixture
        .assign_owner_to_facility(
            other_access.tenant_id,
            other_tenant_owner,
            other_tenant_facility,
        )
        .await;
    let other_tenant_item = fixture
        .item(
            other_access.tenant_id,
            "Cross Tenant Allocation Item",
            "each",
        )
        .await;
    let other_tenant_balance = fixture
        .received_balance(
            &other_access,
            ReceivedBalanceSetup {
                inventory_owner_id: other_tenant_owner,
                facility_id: other_tenant_facility,
                key: "ALLOCATION-CROSS-TENANT",
                item_id: other_tenant_item,
                qty: 10,
            },
        )
        .await
        .balance_id;

    let reservation_command = repo::inventory::CreateInventoryReservationCommand {
        order_id,
        order_item_id,
        facility_id,
        qty: 10,
        idempotency_key: "allocation-soft-reservation",
    };
    let reservation =
        repo::inventory::create_inventory_reservation(&fixture.db, &access, &reservation_command)
            .await
            .unwrap();
    assert_eq!(
        repo::inventory::create_inventory_reservation(&fixture.db, &access, &reservation_command)
            .await
            .unwrap(),
        reservation
    );
    let changed_retry = repo::inventory::create_inventory_reservation(
        &fixture.db,
        &access,
        &repo::inventory::CreateInventoryReservationCommand {
            qty: 9,
            ..reservation_command
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        changed_retry,
        AppError::Application(ApplicationError::IdempotencyKeyReused)
    ));
    assert!(repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap()
        .iter()
        .all(|balance| balance.qty_reserved == 0));
    let soft_reservation = repo::inventory::get_reservations(&fixture.db, tenant_id, false)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == reservation.reservation_id)
        .unwrap();
    assert_eq!(soft_reservation.status, ReservationStatus::Active);
    assert_eq!(soft_reservation.allocated_qty, 0);
    assert!(soft_reservation.allocations.is_empty());

    let allocation_command = repo::inventory::AllocateInventoryCommand {
        reservation_id: reservation.reservation_id,
        inventory_balance_id: balance_a,
        qty: 4,
        idempotency_key: "allocation-first",
    };
    let allocation = repo::inventory::allocate_inventory(&fixture.db, &access, &allocation_command)
        .await
        .unwrap();
    assert_eq!(
        repo::inventory::allocate_inventory(&fixture.db, &access, &allocation_command)
            .await
            .unwrap(),
        allocation
    );
    let peer = fixture.user("inventory-allocation-peer@test.local").await;
    let mut peer_membership_tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(peer.id)
        .execute(&mut *peer_membership_tx)
        .await
        .unwrap();
    peer_membership_tx.commit().await.unwrap();
    let peer_access = repo::tenants::access_for_user(&fixture.db, peer.id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    let cross_actor_replay =
        repo::inventory::allocate_inventory(&fixture.db, &peer_access, &allocation_command)
            .await
            .unwrap_err();
    assert!(matches!(
        cross_actor_replay,
        AppError::Application(ApplicationError::IdempotencyKeyReused)
    ));
    let balances = repo::inventory::get_balances(&fixture.db, tenant_id, false)
        .await
        .unwrap();
    assert_eq!(
        balances
            .iter()
            .find(|row| row.id == balance_a)
            .unwrap()
            .qty_reserved,
        4
    );

    let overstock_reservation = repo::inventory::create_inventory_reservation(
        &fixture.db,
        &access,
        &repo::inventory::CreateInventoryReservationCommand {
            order_id,
            order_item_id,
            facility_id,
            qty: 3,
            idempotency_key: "allocation-overstock-reservation",
        },
    )
    .await
    .unwrap();
    assert_boundary_rejection(
        repo::inventory::allocate_inventory(
            &fixture.db,
            &access,
            &repo::inventory::AllocateInventoryCommand {
                reservation_id: overstock_reservation.reservation_id,
                inventory_balance_id: balance_a,
                qty: 3,
                idempotency_key: "allocation-over-stock",
            },
        )
        .await
        .unwrap_err(),
    );
    assert_boundary_rejection(
        repo::inventory::allocate_inventory(
            &fixture.db,
            &access,
            &repo::inventory::AllocateInventoryCommand {
                reservation_id: reservation.reservation_id,
                inventory_balance_id: balance_b,
                qty: 7,
                idempotency_key: "allocation-over-demand",
            },
        )
        .await
        .unwrap_err(),
    );

    for (key, balance) in [
        ("allocation-cross-owner", other_owner_balance),
        ("allocation-cross-facility", other_facility_balance),
        ("allocation-cross-tenant", other_tenant_balance),
    ] {
        assert_boundary_rejection(
            repo::inventory::allocate_inventory(
                &fixture.db,
                &access,
                &repo::inventory::AllocateInventoryCommand {
                    reservation_id: reservation.reservation_id,
                    inventory_balance_id: balance,
                    qty: 1,
                    idempotency_key: key,
                },
            )
            .await
            .unwrap_err(),
        );
    }
    assert_boundary_rejection(
        repo::inventory::create_inventory_reservation(
            &fixture.db,
            &other_access,
            &repo::inventory::CreateInventoryReservationCommand {
                order_id,
                order_item_id,
                facility_id,
                qty: 1,
                idempotency_key: "allocation-cross-tenant-reservation",
            },
        )
        .await
        .unwrap_err(),
    );

    let concurrent_order = fixture
        .order(tenant_id, "ALLOCATION-CONCURRENT-ORDER", owner_id)
        .await;
    let concurrent_order_item = fixture
        .order_item(tenant_id, concurrent_order, item_id, 5)
        .await;
    let concurrent_reservation = repo::inventory::create_inventory_reservation(
        &fixture.db,
        &access,
        &repo::inventory::CreateInventoryReservationCommand {
            order_id: concurrent_order,
            order_item_id: concurrent_order_item,
            facility_id,
            qty: 5,
            idempotency_key: "allocation-concurrent-reservation",
        },
    )
    .await
    .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let mut attempts = Vec::new();
    for key in ["allocation-concurrent-a", "allocation-concurrent-b"] {
        let db = fixture.db.clone();
        let access = access.clone();
        let barrier = Arc::clone(&barrier);
        attempts.push(tokio::spawn(async move {
            barrier.wait().await;
            (
                key,
                repo::inventory::allocate_inventory(
                    &db,
                    &access,
                    &repo::inventory::AllocateInventoryCommand {
                        reservation_id: concurrent_reservation.reservation_id,
                        inventory_balance_id: balance_b,
                        qty: 4,
                        idempotency_key: key,
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
    .expect("concurrent allocations serialize");
    let mut accepted = Vec::new();
    let mut rejected = 0;
    for (key, result) in results {
        match result {
            Ok(result) => accepted.push((key, result)),
            Err(error) => {
                assert_boundary_rejection(error);
                rejected += 1;
            }
        }
    }
    assert_eq!(accepted.len(), 1);
    assert_eq!(rejected, 1);
    let (accepted_key, accepted_allocation) = &accepted[0];
    assert_eq!(
        repo::inventory::allocate_inventory(
            &fixture.db,
            &access,
            &repo::inventory::AllocateInventoryCommand {
                reservation_id: concurrent_reservation.reservation_id,
                inventory_balance_id: balance_b,
                qty: 4,
                idempotency_key: accepted_key,
            },
        )
        .await
        .unwrap(),
        *accepted_allocation
    );

    let cancel_allocation_command = repo::inventory::CancelInventoryAllocationCommand {
        allocation_id: allocation.allocation_id,
        idempotency_key: "allocation-first-cancel",
    };
    let cancelled_allocation = repo::inventory::cancel_inventory_allocation(
        &fixture.db,
        &access,
        &cancel_allocation_command,
    )
    .await
    .unwrap();
    assert_eq!(cancelled_allocation.released_qty, 4);
    assert_eq!(
        repo::inventory::cancel_inventory_allocation(
            &fixture.db,
            &access,
            &cancel_allocation_command
        )
        .await
        .unwrap(),
        cancelled_allocation
    );
    assert_eq!(
        repo::inventory::get_balances(&fixture.db, tenant_id, false)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.id == balance_a)
            .unwrap()
            .qty_reserved,
        0
    );

    let recovery_allocation = repo::inventory::allocate_inventory(
        &fixture.db,
        &access,
        &repo::inventory::AllocateInventoryCommand {
            reservation_id: reservation.reservation_id,
            inventory_balance_id: balance_a,
            qty: 5,
            idempotency_key: "allocation-recovery",
        },
    )
    .await
    .unwrap();
    let cancel_reservation_command = repo::inventory::CancelInventoryReservationCommand {
        reservation_id: reservation.reservation_id,
        idempotency_key: "allocation-reservation-cancel",
    };
    let cancelled_reservation = repo::inventory::cancel_inventory_reservation(
        &fixture.db,
        &access,
        &cancel_reservation_command,
    )
    .await
    .unwrap();
    assert_eq!(cancelled_reservation.released_qty, 5);
    assert_eq!(
        repo::inventory::cancel_inventory_reservation(
            &fixture.db,
            &access,
            &cancel_reservation_command
        )
        .await
        .unwrap(),
        cancelled_reservation
    );
    assert_boundary_rejection(
        repo::inventory::allocate_inventory(
            &fixture.db,
            &access,
            &repo::inventory::AllocateInventoryCommand {
                reservation_id: reservation.reservation_id,
                inventory_balance_id: balance_a,
                qty: 1,
                idempotency_key: "allocation-after-reservation-cancel",
            },
        )
        .await
        .unwrap_err(),
    );
    let reservation_after_cancel = repo::inventory::get_reservations(&fixture.db, tenant_id, true)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == reservation.reservation_id)
        .unwrap();
    assert_eq!(
        reservation_after_cancel.status,
        ReservationStatus::Cancelled
    );
    assert_eq!(reservation_after_cancel.allocated_qty, 0);
    let allocations = repo::inventory::get_allocations_in_scope(&fixture.db, &access, true)
        .await
        .unwrap();
    assert!(allocations
        .iter()
        .any(|row| row.id == recovery_allocation.allocation_id
            && row.status == AllocationStatus::Released));

    let concurrent_cancel = repo::inventory::cancel_inventory_allocation(
        &fixture.db,
        &access,
        &repo::inventory::CancelInventoryAllocationCommand {
            allocation_id: accepted_allocation.allocation_id,
            idempotency_key: "allocation-concurrent-release",
        },
    )
    .await
    .unwrap();
    assert_eq!(concurrent_cancel.released_qty, 4);
    assert_allocation_reconciliation(&fixture.db, tenant_id).await;
}
