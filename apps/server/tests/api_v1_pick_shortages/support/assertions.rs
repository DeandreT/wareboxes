use sqlx::Row;
use wareboxes_api_contract::v1::{
    PickClaimResponse, PickContentConfirmationResponse, PickContentState, PickShortageStatus,
    ReallocatePickShortageResponse,
};

use super::*;
use crate::common::{admin_db_for, app_db_for, tenant_for_user, tenant_tx};

impl PickShortageFixture {
    pub(crate) async fn assert_reallocation_ledger(
        &self,
        shortage_id: i64,
        none: &ReallocatePickShortageResponse,
        partial: &ReallocatePickShortageResponse,
        full: &ReallocatePickShortageResponse,
    ) {
        let expected = [
            (none, 4, 0, 4, "not_allocated"),
            (partial, 4, 2, 2, "partially_allocated"),
            (full, 2, 2, 0, "fully_allocated"),
        ];
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        for (result, requested, allocated, remaining, outcome) in expected {
            let row = sqlx::query(
                r#"
                SELECT expected_shortage_revision, resulting_shortage_revision,
                       expected_order_revision, resulting_order_revision,
                       requested_qty, allocated_qty, remaining_qty,
                       allocation_count, outcome, strategy, policy_source,
                       policy_configuration_id, policy_configuration_revision,
                       policy_scope_level, policy_definition, policy_hash,
                       (SELECT event.payload->'allocation_policy'
                        FROM outbox_events event
                        WHERE event.tenant_id=run.tenant_id
                          AND event.event_type='outbound.pick.shortage_reallocated'
                          AND event.payload->>'reallocation_run_id'=run.id::TEXT)
                          AS event_policy,
                       (SELECT COUNT(*) FROM order_release_allocations snapshot
                        WHERE snapshot.tenant_id = run.tenant_id
                          AND snapshot.pick_shortage_reallocation_run_id = run.id)
                          AS snapshot_count,
                       (SELECT COUNT(*) FROM pick_tasks task
                        INNER JOIN order_release_allocations snapshot
                          ON snapshot.tenant_id = task.tenant_id
                         AND snapshot.allocation_id = task.source_allocation_id
                        WHERE task.tenant_id = run.tenant_id
                          AND snapshot.pick_shortage_reallocation_run_id = run.id)
                          AS task_count
                FROM pick_shortage_reallocation_runs run
                WHERE run.tenant_id = $1 AND run.id = $2
                "#,
            )
            .bind(self.access.tenant_id.get())
            .bind(result.reallocation_run_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            assert_eq!(
                row.get::<i64, _>("expected_shortage_revision"),
                result.shortage_revision.get() - 1
            );
            assert_eq!(
                row.get::<i64, _>("resulting_shortage_revision"),
                result.shortage_revision.get()
            );
            assert_eq!(
                row.get::<i64, _>("expected_order_revision"),
                result.order_revision.get() - 1
            );
            assert_eq!(
                row.get::<i64, _>("resulting_order_revision"),
                result.order_revision.get()
            );
            assert_eq!(row.get::<i64, _>("requested_qty"), requested);
            assert_eq!(row.get::<i64, _>("allocated_qty"), allocated);
            assert_eq!(row.get::<i64, _>("remaining_qty"), remaining);
            assert_eq!(
                row.get::<i64, _>("allocation_count"),
                i64::try_from(result.new_allocations.len()).unwrap()
            );
            assert_eq!(row.get::<String, _>("outcome"), outcome);
            assert_eq!(row.get::<String, _>("strategy"), "fifo");
            assert_eq!(row.get::<String, _>("policy_source"), "configuration");
            assert_eq!(
                row.get::<Option<i64>, _>("policy_configuration_id"),
                result.policy.configuration_id
            );
            assert_eq!(
                row.get::<Option<i64>, _>("policy_configuration_revision"),
                result
                    .policy
                    .configuration_revision
                    .map(|revision| revision.get())
            );
            assert_eq!(
                row.get::<Option<String>, _>("policy_scope_level")
                    .as_deref(),
                Some("owner_facility")
            );
            let definition = row.get::<serde_json::Value, _>("policy_definition");
            assert_eq!(definition["rotation"], "fifo");
            assert_eq!(
                row.get::<String, _>("policy_hash"),
                result.policy.policy_hash
            );
            assert_eq!(
                row.get::<serde_json::Value, _>("event_policy"),
                serde_json::to_value(&result.policy).unwrap()
            );
            assert_eq!(
                row.get::<i64, _>("snapshot_count"),
                i64::try_from(result.new_allocations.len()).unwrap()
            );
            assert_eq!(
                row.get::<i64, _>("task_count"),
                i64::try_from(result.new_tasks.len()).unwrap()
            );
        }

        let appended = sqlx::query(
            r#"
            SELECT (SELECT COUNT(*) FROM order_releases release
                    WHERE release.tenant_id = $1 AND release.order_id = $2) AS release_count,
                   (SELECT allocation_count FROM order_releases release
                    WHERE release.tenant_id = $1 AND release.order_id = $2) AS released_allocations,
                   (SELECT pick_task_count FROM order_releases release
                    WHERE release.tenant_id = $1 AND release.order_id = $2) AS released_tasks,
                   (SELECT COUNT(*) FROM order_release_allocations snapshot
                    WHERE snapshot.tenant_id = $1 AND snapshot.order_id = $2
                      AND snapshot.source_kind = 'initial') AS initial_snapshots,
                   (SELECT COUNT(*) FROM order_release_allocations snapshot
                    WHERE snapshot.tenant_id = $1 AND snapshot.order_id = $2
                      AND snapshot.source_kind = 'shortage_recovery'
                      AND snapshot.pick_shortage_id = $3) AS recovery_snapshots,
                   (SELECT COALESCE(SUM(snapshot.planned_qty), 0)::BIGINT
                    FROM order_release_allocations snapshot
                    WHERE snapshot.tenant_id = $1 AND snapshot.order_id = $2
                      AND snapshot.source_kind = 'shortage_recovery'
                      AND snapshot.pick_shortage_id = $3) AS recovery_qty,
                   (SELECT COUNT(*) FROM outbox_events event
                    WHERE event.tenant_id = $1
                      AND event.event_type = 'outbound.pick.shortage_reallocated'
                      AND event.ordering_key = 'order:' || $2::TEXT) AS event_count,
                   (SELECT COUNT(*) FROM pick_tasks task
                    WHERE task.tenant_id = $1 AND task.order_id = $2) AS total_tasks
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(self.order_id)
        .bind(shortage_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();

        assert_eq!(appended.get::<i64, _>("release_count"), 1);
        assert_eq!(appended.get::<i64, _>("released_allocations"), 1);
        assert_eq!(appended.get::<i64, _>("released_tasks"), 1);
        assert_eq!(appended.get::<i64, _>("initial_snapshots"), 1);
        assert_eq!(appended.get::<i64, _>("recovery_snapshots"), 2);
        assert_eq!(appended.get::<i64, _>("recovery_qty"), 4);
        assert_eq!(appended.get::<i64, _>("event_count"), 3);
        assert_eq!(appended.get::<i64, _>("total_tasks"), 3);

        let admin = admin_db_for(&self.fixture.db).await;
        let mutation = sqlx::query(
            r#"
            UPDATE order_release_allocations
            SET planned_qty = planned_qty
            WHERE tenant_id = $1 AND pick_shortage_id = $2
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(shortage_id)
        .execute(&admin)
        .await;
        assert!(
            mutation.is_err(),
            "recovery release snapshots are immutable"
        );
        admin.close().await;
    }

    pub(crate) async fn confirm_next(&self, shortage_id: i64, key: &str) -> ConfirmedRecovery {
        let claim = self
            .request(
                Method::POST,
                "/api/v1/picking-claims/next",
                Some(&format!("{key}-claim")),
                Some(json!({})),
            )
            .await;
        let claim = expect_status(claim, StatusCode::OK, "claim recovery pick").await;
        let claim = response_json::<Option<PickClaimResponse>>(claim)
            .await
            .expect("recovery allocation has pick work");
        let confirmation = self
            .request(
                Method::POST,
                &format!(
                    "/api/v1/picking-tasks/{}/contents/{}/confirmations",
                    claim.task_id, claim.content.content_id
                ),
                Some(key),
                Some(json!({
                    "source_location_barcode": claim.content.source_location_barcode,
                    "item_barcode": claim.content.item_barcodes[0],
                    "source_license_plate_barcode": claim.content.source_license_plate_barcode,
                    "destination_license_plate_barcode": self.destination_plate_barcode
                })),
            )
            .await;
        let confirmation =
            expect_status(confirmation, StatusCode::OK, "confirm recovery pick").await;
        let confirmation: PickContentConfirmationResponse = response_json(confirmation).await;
        assert_eq!(confirmation.content_state, PickContentState::Completed);
        assert!(confirmation.task_completed);

        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let shortage_status: String = sqlx::query_scalar(
            "SELECT status FROM pick_shortages WHERE tenant_id = $1 AND id = $2",
        )
        .bind(self.access.tenant_id.get())
        .bind(shortage_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        ConfirmedRecovery {
            order_status: confirmation.order_status,
            shortage_status: match shortage_status.as_str() {
                "awaiting_inventory" => PickShortageStatus::AwaitingInventory,
                "recovery_in_progress" => PickShortageStatus::RecoveryInProgress,
                "resolved" => PickShortageStatus::Resolved,
                value => panic!("unknown shortage status {value}"),
            },
        }
    }

    pub(crate) async fn assert_fully_recovered_and_packing_ready(
        &self,
        shortage_id: i64,
        planned_quantity: i64,
    ) {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let row = sqlx::query(
            r#"
            SELECT orders.status AS order_status, orders.revision AS order_revision,
                   shortage.status AS shortage_status, shortage.revision AS shortage_revision,
                   shortage.picked_qty, shortage.short_qty,
                   shortage.reallocated_qty, shortage.recovery_terminal_qty,
                   shortage.remaining_to_allocate_qty,
                   hold.status AS hold_status, hold.qty AS hold_qty,
                   source_balance.qty_on_hand AS original_source_on_hand,
                   source_balance.qty_reserved AS original_source_reserved,
                   source_balance.qty_held AS original_source_held,
                   reservation.status AS reservation_status,
                   reservation.qty AS reservation_qty,
                   (SELECT COUNT(*) FROM pick_tasks task
                    WHERE task.tenant_id = shortage.tenant_id
                      AND task.order_id = shortage.order_id
                      AND task.status = 'shorted') AS shorted_tasks,
                   (SELECT COUNT(*) FROM pick_tasks task
                    WHERE task.tenant_id = shortage.tenant_id
                      AND task.order_id = shortage.order_id
                      AND task.status = 'completed') AS completed_tasks,
                   (SELECT COUNT(*) FROM pick_task_contents content
                    WHERE content.tenant_id = shortage.tenant_id
                      AND content.order_id = shortage.order_id
                      AND content.state = 'shorted') AS shorted_contents,
                   (SELECT COUNT(*) FROM pick_task_contents content
                    WHERE content.tenant_id = shortage.tenant_id
                      AND content.order_id = shortage.order_id
                      AND content.state = 'completed') AS completed_contents,
                   (SELECT COUNT(*) FROM inventory_allocations allocation
                    WHERE allocation.tenant_id = shortage.tenant_id
                      AND allocation.reservation_id = shortage.reservation_id
                      AND allocation.status = 'allocated'
                      AND allocation.execution_stage = 'staged') AS staged_allocations,
                   (SELECT COALESCE(SUM(allocation.qty), 0)::BIGINT
                    FROM inventory_allocations allocation
                    WHERE allocation.tenant_id = shortage.tenant_id
                      AND allocation.reservation_id = shortage.reservation_id
                      AND allocation.status = 'allocated'
                      AND allocation.execution_stage = 'staged') AS staged_qty,
                   (SELECT COALESCE(SUM(balance.qty_on_hand), 0)::BIGINT
                    FROM inventory_balances balance
                    WHERE balance.tenant_id = shortage.tenant_id
                      AND balance.inventory_owner_id = shortage.inventory_owner_id
                      AND balance.facility_id = shortage.facility_id
                      AND balance.location_id = shortage.destination_location_id
                      AND balance.license_plate_id = shortage.destination_license_plate_id
                      AND balance.item_id = shortage.item_id) AS destination_on_hand,
                   (SELECT COALESCE(SUM(balance.qty_reserved), 0)::BIGINT
                    FROM inventory_balances balance
                    WHERE balance.tenant_id = shortage.tenant_id
                      AND balance.inventory_owner_id = shortage.inventory_owner_id
                      AND balance.facility_id = shortage.facility_id
                      AND balance.location_id = shortage.destination_location_id
                      AND balance.license_plate_id = shortage.destination_license_plate_id
                      AND balance.item_id = shortage.item_id) AS destination_reserved,
                   (SELECT COUNT(*) FROM work_tasks work
                    WHERE work.tenant_id = shortage.tenant_id
                      AND work.facility_id = shortage.facility_id
                      AND work.inventory_owner_id = shortage.inventory_owner_id
                      AND work.task_type LIKE 'cycle_count%') AS count_tasks
            FROM pick_shortages shortage
            INNER JOIN orders
              ON orders.tenant_id = shortage.tenant_id AND orders.id = shortage.order_id
            INNER JOIN inventory_holds hold
              ON hold.tenant_id = shortage.tenant_id AND hold.id = shortage.inventory_hold_id
            INNER JOIN inventory_balances source_balance
              ON source_balance.tenant_id = shortage.tenant_id
             AND source_balance.id = shortage.source_inventory_balance_id
            INNER JOIN inventory_reservations reservation
              ON reservation.tenant_id = shortage.tenant_id
             AND reservation.id = shortage.reservation_id
            WHERE shortage.tenant_id = $1 AND shortage.id = $2
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(shortage_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();

        assert_eq!(row.get::<String, _>("order_status"), "awaiting packing");
        assert_eq!(row.get::<i64, _>("order_revision"), 8);
        assert_eq!(row.get::<String, _>("shortage_status"), "resolved");
        assert_eq!(row.get::<i64, _>("shortage_revision"), 6);
        assert_eq!(row.get::<i64, _>("picked_qty"), 2);
        assert_eq!(row.get::<i64, _>("short_qty"), 4);
        assert_eq!(row.get::<i64, _>("reallocated_qty"), 4);
        assert_eq!(row.get::<i64, _>("recovery_terminal_qty"), 4);
        assert_eq!(row.get::<i64, _>("remaining_to_allocate_qty"), 0);
        assert_eq!(row.get::<String, _>("hold_status"), "active");
        assert_eq!(row.get::<i64, _>("hold_qty"), 4);
        assert_eq!(row.get::<i64, _>("original_source_on_hand"), 4);
        assert_eq!(row.get::<i64, _>("original_source_reserved"), 0);
        assert_eq!(row.get::<i64, _>("original_source_held"), 4);
        assert_eq!(row.get::<String, _>("reservation_status"), "active");
        assert_eq!(row.get::<i64, _>("reservation_qty"), planned_quantity);
        assert_eq!(row.get::<i64, _>("shorted_tasks"), 1);
        assert_eq!(row.get::<i64, _>("completed_tasks"), 2);
        assert_eq!(row.get::<i64, _>("shorted_contents"), 1);
        assert_eq!(row.get::<i64, _>("completed_contents"), 2);
        assert_eq!(row.get::<i64, _>("staged_allocations"), 3);
        assert_eq!(row.get::<i64, _>("staged_qty"), planned_quantity);
        assert_eq!(row.get::<i64, _>("destination_on_hand"), planned_quantity);
        assert_eq!(row.get::<i64, _>("destination_reserved"), planned_quantity);
        assert_eq!(row.get::<i64, _>("count_tasks"), 0);
    }

    pub(crate) async fn successful_reallocation_key(&self, shortage_id: i64) -> String {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let key = sqlx::query_scalar(
            r#"
            SELECT idempotency_key
            FROM command_idempotency_records
            WHERE tenant_id = $1
              AND operation = 'picking.shortage.reallocate.v1'
              AND (result_json->>'shortage_id')::BIGINT = $2
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(shortage_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        key
    }

    pub(crate) async fn assert_one_reallocation(&self, shortage_id: i64, quantity: i64) {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let row = sqlx::query(
            r#"
            SELECT (SELECT COUNT(*) FROM pick_shortage_reallocation_runs run
                    WHERE run.tenant_id = $1 AND run.pick_shortage_id = $2) AS run_count,
                   (SELECT COUNT(*) FROM command_idempotency_records command
                    WHERE command.tenant_id = $1
                      AND command.operation = 'picking.shortage.reallocate.v1'
                      AND (command.result_json->>'shortage_id')::BIGINT = $2) AS command_count,
                   (SELECT COUNT(*) FROM outbox_events event
                    WHERE event.tenant_id = $1
                      AND event.event_type = 'outbound.pick.shortage_reallocated'
                      AND event.ordering_key = 'order:' || $3::TEXT) AS event_count,
                   (SELECT COUNT(*) FROM order_release_allocations snapshot
                    WHERE snapshot.tenant_id = $1 AND snapshot.pick_shortage_id = $2
                      AND snapshot.source_kind = 'shortage_recovery') AS snapshot_count,
                   (SELECT COALESCE(SUM(snapshot.planned_qty), 0)::BIGINT
                    FROM order_release_allocations snapshot
                    WHERE snapshot.tenant_id = $1 AND snapshot.pick_shortage_id = $2
                      AND snapshot.source_kind = 'shortage_recovery') AS snapshot_qty,
                   (SELECT COUNT(*) FROM pick_tasks task
                    INNER JOIN order_release_allocations snapshot
                      ON snapshot.tenant_id = task.tenant_id
                     AND snapshot.allocation_id = task.source_allocation_id
                    WHERE task.tenant_id = $1 AND snapshot.pick_shortage_id = $2
                      AND snapshot.source_kind = 'shortage_recovery') AS task_count
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(shortage_id)
        .bind(self.order_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(row.get::<i64, _>("run_count"), 1);
        assert_eq!(row.get::<i64, _>("command_count"), 1);
        assert_eq!(row.get::<i64, _>("event_count"), 1);
        assert_eq!(row.get::<i64, _>("snapshot_count"), 1);
        assert_eq!(row.get::<i64, _>("snapshot_qty"), quantity);
        assert_eq!(row.get::<i64, _>("task_count"), 1);
    }

    pub(crate) async fn assert_cancellation_has_zero_effects(&self) {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let row = sqlx::query(
            r#"
            SELECT orders.status, orders.revision,
                   shortage.status AS shortage_status,
                   shortage.revision AS shortage_revision,
                   reservation.status AS reservation_status,
                   (SELECT COUNT(*) FROM order_cancellations cancellation
                    WHERE cancellation.tenant_id = orders.tenant_id
                      AND cancellation.order_id = orders.id) AS cancellation_count,
                   (SELECT COUNT(*) FROM pick_shortage_reallocation_runs run
                    WHERE run.tenant_id = orders.tenant_id
                      AND run.order_id = orders.id) AS run_count
            FROM orders
            INNER JOIN pick_shortages shortage
              ON shortage.tenant_id = orders.tenant_id AND shortage.order_id = orders.id
            INNER JOIN inventory_reservations reservation
              ON reservation.tenant_id = shortage.tenant_id
             AND reservation.id = shortage.reservation_id
            WHERE orders.tenant_id = $1 AND orders.id = $2
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(self.order_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(row.get::<String, _>("status"), "processing");
        assert_eq!(row.get::<i64, _>("revision"), 4);
        assert_eq!(
            row.get::<String, _>("shortage_status"),
            "awaiting_inventory"
        );
        assert_eq!(row.get::<i64, _>("shortage_revision"), 1);
        assert_eq!(row.get::<String, _>("reservation_status"), "active");
        assert_eq!(row.get::<i64, _>("cancellation_count"), 0);
        assert_eq!(row.get::<i64, _>("run_count"), 0);
    }

    pub(crate) async fn assert_shortage_tables_governed(&self) {
        let admin = admin_db_for(&self.fixture.db).await;
        for (table, can_update) in [
            ("pick_shortages", true),
            ("pick_shortage_reallocation_runs", false),
        ] {
            let privileges: (bool, bool, bool, bool) = sqlx::query_as(
                r#"
                SELECT has_table_privilege('wareboxes_app', $1, 'SELECT'),
                       has_table_privilege('wareboxes_app', $1, 'INSERT'),
                       has_table_privilege('wareboxes_app', $1, 'UPDATE'),
                       has_table_privilege('wareboxes_app', $1, 'DELETE')
                "#,
            )
            .bind(table)
            .fetch_one(&admin)
            .await
            .unwrap();
            assert_eq!(privileges, (true, true, can_update, false), "{table}");
            let rls: (bool, bool) = sqlx::query_as(
                "SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE oid = $1::regclass",
            )
            .bind(table)
            .fetch_one(&admin)
            .await
            .unwrap();
            assert_eq!(rls, (true, true), "{table}");
        }
        for sequence in [
            "pick_shortages_id_seq",
            "pick_shortage_reallocation_runs_id_seq",
        ] {
            let privileges: (bool, bool, bool) = sqlx::query_as(
                r#"
                SELECT has_sequence_privilege('wareboxes_app', $1, 'USAGE'),
                       has_sequence_privilege('wareboxes_app', $1, 'SELECT'),
                       has_sequence_privilege('wareboxes_app', $1, 'UPDATE')
                "#,
            )
            .bind(sequence)
            .fetch_one(&admin)
            .await
            .unwrap();
            assert_eq!(privileges, (true, false, false), "{sequence}");
        }
        admin.close().await;
    }

    pub(crate) async fn assert_shortage_rows_immutable(&self, shortage_id: i64, run_id: i64) {
        let admin = admin_db_for(&self.fixture.db).await;
        for (operation, statement, id) in [
            (
                "forged shortage recovery projection",
                "UPDATE pick_shortages SET revision = revision + 1, modified_at = clock_timestamp(), reallocated_qty = 1, remaining_to_allocate_qty = remaining_to_allocate_qty - 1, status = 'recovery_in_progress' WHERE tenant_id = $1 AND id = $2",
                shortage_id,
            ),
            (
                "shortage facts update",
                "UPDATE pick_shortages SET reason_code = 'damaged_inventory' WHERE tenant_id = $1 AND id = $2",
                shortage_id,
            ),
            (
                "shortage delete",
                "DELETE FROM pick_shortages WHERE tenant_id = $1 AND id = $2",
                shortage_id,
            ),
            (
                "reallocation run update",
                "UPDATE pick_shortage_reallocation_runs SET outcome = outcome WHERE tenant_id = $1 AND id = $2",
                run_id,
            ),
            (
                "reallocation run delete",
                "DELETE FROM pick_shortage_reallocation_runs WHERE tenant_id = $1 AND id = $2",
                run_id,
            ),
        ] {
            let result = sqlx::query(statement)
                .bind(self.access.tenant_id.get())
                .bind(id)
                .execute(&admin)
                .await;
            assert!(result.is_err(), "{operation} must be rejected");
        }
        admin.close().await;
    }

    pub(crate) async fn assert_cross_tenant_rls(&self, shortage_id: i64) {
        let outsider = self
            .fixture
            .wms_user("shortage-rls-outsider@test.local")
            .await;
        let outsider_tenant = tenant_for_user(&self.fixture.db, outsider.id).await;
        let app_db = app_db_for(&self.fixture.db).await;
        let missing_context: (i64, i64) = sqlx::query_as(
            r#"
            SELECT (SELECT COUNT(*) FROM pick_shortages WHERE tenant_id = $1),
                   (SELECT COUNT(*) FROM pick_shortage_reallocation_runs WHERE tenant_id = $1)
            "#,
        )
        .bind(self.access.tenant_id.get())
        .fetch_one(&app_db)
        .await
        .unwrap();
        assert_eq!(missing_context, (0, 0));
        let mut tx = tenant_tx(&app_db, outsider_tenant).await;
        let concealed: (i64, i64) = sqlx::query_as(
            r#"
            SELECT (SELECT COUNT(*) FROM pick_shortages
                    WHERE tenant_id = $1 AND id = $2),
                   (SELECT COUNT(*) FROM pick_shortage_reallocation_runs
                    WHERE tenant_id = $1 AND pick_shortage_id = $2)
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(shortage_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        app_db.close().await;
        assert_eq!(concealed, (0, 0));
    }
}
