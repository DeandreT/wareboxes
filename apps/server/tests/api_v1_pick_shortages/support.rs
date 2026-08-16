use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{
    AllocationExecutionStage, OrderAllocationOutcome, PickClaimResponse, PickOrderStatus,
    PickShortageStatus, ReallocatePickShortageResponse, ReportPickShortageResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::TenantId;

use super::common::*;

#[path = "support/assertions.rs"]
mod assertions;

#[path = "support/scenarios.rs"]
mod scenarios;

#[path = "support/policy.rs"]
mod policy;

pub(super) struct PickShortageFixture {
    pub(super) fixture: Fixture,
    pub(super) app: axum::Router,
    pub(super) access: TenantAccess,
    pub(super) token: String,
    pub(super) inventory_owner_id: i64,
    pub(super) facility_id: i64,
    pub(super) order_id: i64,
    pub(super) item_id: i64,
    pub(super) item_barcode: String,
    pub(super) destination_location_id: i64,
    pub(super) destination_plate_id: i64,
    pub(super) destination_plate_barcode: String,
    pub(super) source_balance: ReceivedBalance,
    pub(super) claim: PickClaimResponse,
}

pub(super) struct AllocationProgress {
    pub(super) newly_allocated: i64,
    pub(super) recovery_allocated: i64,
    pub(super) recovery_picked: i64,
    pub(super) remaining: i64,
    pub(super) task_count: usize,
    pub(super) status: PickShortageStatus,
}

pub(super) struct ConfirmedRecovery {
    pub(super) order_status: PickOrderStatus,
    pub(super) shortage_status: PickShortageStatus,
}

pub(super) struct RecoveryProgressExpectation {
    pub(super) shortage_id: i64,
    pub(super) shortage_revision: i64,
    pub(super) shortage_status: PickShortageStatus,
    pub(super) order_revision: i64,
    pub(super) trigger_task_id: i64,
    pub(super) trigger_source_allocation_id: i64,
    pub(super) terminal_quantity: i64,
    pub(super) preceding_event_type: &'static str,
}

pub(super) struct CrossTenantUser {
    pub(super) tenant_id: TenantId,
    pub(super) token: String,
}

impl PickShortageFixture {
    pub(super) async fn new(key: &str, quantity: i64) -> Self {
        let fixture = Fixture::new().await;
        let operator = fixture.wms_user(&format!("{key}@test.local")).await;
        let access = default_tenant_for_user(&fixture.db, operator.id)
            .await
            .unwrap();
        grant_permissions(&fixture.db, access.tenant_id, operator.id, key, &["orders"]).await;
        let inventory_owner_id = fixture
            .inventory_owner(access.tenant_id, &format!("{key} owner"))
            .await;
        let facility_id = fixture
            .facility(access.tenant_id, &format!("{key} facility"))
            .await;
        fixture
            .assign_owner_to_facility(access.tenant_id, inventory_owner_id, facility_id)
            .await;
        let destination_location_id = staging_location(
            &fixture,
            access.tenant_id,
            facility_id,
            &format!("{key}-STAGE"),
        )
        .await;
        let destination_plate_barcode = format!("{key}-TOTE");
        let destination_plate_id = plate_at(
            &fixture,
            access.tenant_id,
            inventory_owner_id,
            facility_id,
            destination_location_id,
            &destination_plate_barcode,
        )
        .await;
        let item_id = fixture
            .item(access.tenant_id, &format!("{key} item"), "each")
            .await;
        let item_barcode = format!("{key}-ITEM");
        wareboxes_api::repo::items::add_barcode(
            &fixture.db,
            access.tenant_id,
            item_id,
            &item_barcode,
            "code128",
            None,
        )
        .await
        .unwrap();
        let order_id = fixture
            .order_header(
                access.tenant_id,
                &format!("{key}-ORDER"),
                inventory_owner_id,
            )
            .await;
        fixture
            .order_item(access.tenant_id, order_id, item_id, quantity)
            .await;
        let source_key = format!("{key}-SOURCE");
        let source_balance = fixture
            .received_balance(
                &access,
                ReceivedBalanceSetup {
                    inventory_owner_id,
                    facility_id,
                    item_id,
                    qty: quantity,
                    key: &source_key,
                },
            )
            .await;
        let token = auth::create_session(&fixture.db, operator.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        let allocation = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &format!("/api/v1/orders/{order_id}/allocation-runs"),
            Some(&format!("{key}-allocate")),
            Some(json!({
                "facility_id": facility_id,
                "expected_revision": 1,
                "expected_policy": {"source": "product_default", "policy_hash": "6090a99a06ea2e049d7321d5cf2b8f462c6d6e6e2ca527ae87657a7a5fd9d156"}
            })),
        )
        .await;
        expect_status(allocation, StatusCode::OK, "fixture allocation").await;
        let release = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            &format!("/api/v1/orders/{order_id}/releases"),
            Some(&format!("{key}-release")),
            Some(json!({
                "facility_id": facility_id,
                "destination_location_id": destination_location_id,
                "expected_revision": 2
            })),
        )
        .await;
        expect_status(release, StatusCode::OK, "fixture release").await;
        let claim = send(
            &app,
            &token,
            access.tenant_id,
            Method::POST,
            "/api/v1/picking-claims/next",
            Some(&format!("{key}-claim")),
            Some(json!({})),
        )
        .await;
        let claim = expect_status(claim, StatusCode::OK, "fixture pick claim").await;
        let claim = response_json::<Option<PickClaimResponse>>(claim)
            .await
            .expect("fixture has released pick work");
        Self {
            fixture,
            app,
            access,
            token,
            inventory_owner_id,
            facility_id,
            order_id,
            item_id,
            item_barcode,
            destination_location_id,
            destination_plate_id,
            destination_plate_barcode,
            source_balance,
            claim,
        }
    }

    pub(super) fn short_pick_path(&self) -> String {
        format!(
            "/api/v1/picking-tasks/{}/contents/{}/short-picks",
            self.claim.task_id, self.claim.content.content_id
        )
    }

    pub(super) fn confirmation_path(&self) -> String {
        format!(
            "/api/v1/picking-tasks/{}/contents/{}/confirmations",
            self.claim.task_id, self.claim.content.content_id
        )
    }

    pub(super) fn no_pick_body(&self, reason: &str, note: Option<&str>) -> Value {
        json!({
            "source_location_barcode": self.claim.content.source_location_barcode,
            "source_license_plate_barcode": self.claim.content.source_license_plate_barcode,
            "observed_item_barcode": Value::Null,
            "observed_lot": Value::Null,
            "observed_serial": Value::Null,
            "outcome": { "kind": "no_pick" },
            "details": { "reason": reason, "note": note }
        })
    }

    pub(super) fn partial_body(
        &self,
        picked_quantity: i64,
        reason: &str,
        note: Option<&str>,
    ) -> Value {
        json!({
            "source_location_barcode": self.claim.content.source_location_barcode,
            "source_license_plate_barcode": self.claim.content.source_license_plate_barcode,
            "observed_item_barcode": self.item_barcode,
            "observed_lot": self.claim.content.lot,
            "observed_serial": self.claim.content.serial,
            "outcome": {
                "kind": "partial",
                "picked_quantity": picked_quantity,
                "destination_license_plate_barcode": self.destination_plate_barcode
            },
            "details": { "reason": reason, "note": note }
        })
    }

    pub(super) fn confirmation_body(&self) -> Value {
        json!({
            "source_location_barcode": self.claim.content.source_location_barcode,
            "item_barcode": self.item_barcode,
            "source_license_plate_barcode": self.claim.content.source_license_plate_barcode,
            "destination_license_plate_barcode": self.destination_plate_barcode
        })
    }

    pub(super) fn invalid_report_bodies(&self) -> Vec<(&'static str, Value)> {
        let mut partial_without_item = self.partial_body(1, "insufficient_quantity", None);
        partial_without_item["observed_item_barcode"] = Value::Null;
        let wrong_inventory_without_evidence = self.no_pick_body("wrong_inventory", None);
        let lot_mismatch_without_evidence = self.no_pick_body("lot_or_serial_mismatch", None);
        vec![
            (
                "short-negative-quantity",
                self.partial_body(-1, "insufficient_quantity", None),
            ),
            (
                "short-full-quantity",
                self.partial_body(
                    self.claim.content.planned_quantity,
                    "insufficient_quantity",
                    None,
                ),
            ),
            ("short-other-without-note", self.no_pick_body("other", None)),
            (
                "short-wrong-source",
                merge_json(
                    self.no_pick_body("inventory_missing", None),
                    json!({"source_location_barcode": "NOT-THE-SOURCE"}),
                ),
            ),
            (
                "short-wrong-source-license-plate",
                merge_json(
                    self.no_pick_body("inventory_missing", None),
                    json!({"source_license_plate_barcode": "NOT-THE-SOURCE-PLATE"}),
                ),
            ),
            (
                "short-no-pick-with-destination",
                merge_json(
                    self.no_pick_body("inventory_missing", None),
                    json!({"outcome": {
                        "kind": "no_pick",
                        "destination_license_plate_barcode": self.destination_plate_barcode
                    }}),
                ),
            ),
            ("short-partial-without-item", partial_without_item),
            (
                "short-wrong-inventory-without-evidence",
                wrong_inventory_without_evidence,
            ),
            (
                "short-lot-mismatch-without-evidence",
                lot_mismatch_without_evidence,
            ),
            (
                "short-wrong-inventory-matches-directed-item",
                merge_json(
                    self.no_pick_body("wrong_inventory", None),
                    json!({"observed_item_barcode": self.item_barcode}),
                ),
            ),
        ]
    }

    pub(super) async fn request(
        &self,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        send(
            &self.app,
            &self.token,
            self.access.tenant_id,
            method,
            path,
            key,
            body,
        )
        .await
    }

    pub(super) async fn report(&self, key: Option<&str>, body: Value) -> axum::response::Response {
        self.request(Method::POST, &self.short_pick_path(), key, Some(body))
            .await
    }

    pub(super) async fn reallocate(
        &self,
        shortage_id: i64,
        key: Option<&str>,
        body: Value,
    ) -> axum::response::Response {
        self.request(
            Method::POST,
            &format!("/api/v1/pick-shortages/{shortage_id}/reallocations"),
            key,
            Some(body),
        )
        .await
    }

    pub(super) async fn add_recovery_balance(&self, quantity: i64, key: &str) -> ReceivedBalance {
        self.fixture
            .received_balance(
                &self.access,
                ReceivedBalanceSetup {
                    inventory_owner_id: self.inventory_owner_id,
                    facility_id: self.facility_id,
                    item_id: self.item_id,
                    qty: quantity,
                    key,
                },
            )
            .await
    }

    pub(super) async fn wrong_destination_plate(&self, barcode: &str) -> String {
        plate_at(
            &self.fixture,
            self.access.tenant_id,
            self.inventory_owner_id,
            self.facility_id,
            self.source_balance.location_id,
            barcode,
        )
        .await;
        barcode.to_string()
    }

    pub(super) async fn assert_untouched_pick(&self, quantity: i64) {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let row = sqlx::query(
            r#"
            SELECT orders.status AS order_status, orders.revision AS order_revision,
                   task.status AS task_status, content.state AS content_state,
                   allocation.status AS allocation_status,
                   allocation.execution_stage,
                   allocation.deleted IS NOT NULL AS allocation_deleted,
                   balance.qty_on_hand, balance.qty_reserved, balance.qty_held,
                   reservation.status AS reservation_status, reservation.qty AS reservation_qty,
                   (SELECT COUNT(*) FROM inventory_holds hold
                    WHERE hold.tenant_id = task.tenant_id
                      AND hold.inventory_balance_id = content.source_inventory_balance_id)
                      AS hold_count,
                   (SELECT COUNT(*) FROM pick_confirmations confirmation
                    WHERE confirmation.tenant_id = task.tenant_id
                      AND confirmation.order_id = task.order_id) AS confirmation_count
            FROM pick_tasks task
            INNER JOIN pick_task_contents content
              ON content.tenant_id = task.tenant_id AND content.task_id = task.id
            INNER JOIN orders
              ON orders.tenant_id = task.tenant_id AND orders.id = task.order_id
            INNER JOIN inventory_allocations allocation
              ON allocation.tenant_id = content.tenant_id
             AND allocation.id = content.source_allocation_id
            INNER JOIN inventory_balances balance
              ON balance.tenant_id = content.tenant_id
             AND balance.id = content.source_inventory_balance_id
            INNER JOIN inventory_reservations reservation
              ON reservation.tenant_id = content.tenant_id
             AND reservation.id = content.reservation_id
            WHERE task.tenant_id = $1 AND task.id = $2 AND content.id = $3
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(self.claim.task_id)
        .bind(self.claim.content.content_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();

        assert_eq!(row.get::<String, _>("order_status"), "processing");
        assert_eq!(row.get::<i64, _>("order_revision"), 3);
        assert_eq!(row.get::<String, _>("task_status"), "in_progress");
        assert_eq!(row.get::<String, _>("content_state"), "pending");
        assert_eq!(row.get::<String, _>("allocation_status"), "allocated");
        assert_eq!(row.get::<String, _>("execution_stage"), "pick_source");
        assert!(!row.get::<bool, _>("allocation_deleted"));
        assert_eq!(row.get::<i64, _>("qty_on_hand"), quantity);
        assert_eq!(row.get::<i64, _>("qty_reserved"), quantity);
        assert_eq!(row.get::<i64, _>("qty_held"), 0);
        assert_eq!(row.get::<String, _>("reservation_status"), "active");
        assert_eq!(row.get::<i64, _>("reservation_qty"), quantity);
        assert_eq!(row.get::<i64, _>("hold_count"), 0);
        assert_eq!(row.get::<i64, _>("confirmation_count"), 0);
    }

    pub(super) async fn assert_reported_state(
        &self,
        report: &ReportPickShortageResponse,
        picked: i64,
        short: i64,
    ) {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let row = sqlx::query(
            r#"
            SELECT orders.status AS order_status, orders.revision AS order_revision,
                   task.status AS task_status, content.state AS content_state,
                   source.status AS source_status, source.execution_stage AS source_stage,
                   source.deleted IS NOT NULL AS source_deleted,
                   source_balance.qty_on_hand AS source_on_hand,
                   source_balance.qty_reserved AS source_reserved,
                   source_balance.qty_held AS source_held,
                   reservation.status AS reservation_status,
                   reservation.qty AS reservation_qty,
                   hold.status AS hold_status, hold.qty AS hold_qty,
                   hold.reason_code, hold.reference_type, hold.reference_id,
                   shortage.revision AS shortage_revision,
                   shortage.status AS shortage_status,
                   shortage.report_previous_order_revision,
                   shortage.report_resulting_order_revision,
                   shortage.reallocated_qty, shortage.recovery_terminal_qty,
                   shortage.remaining_to_allocate_qty,
                   (SELECT COUNT(*) FROM pick_confirmations confirmation
                    WHERE confirmation.tenant_id = shortage.tenant_id
                      AND confirmation.order_id = shortage.order_id) AS confirmation_count,
                   (SELECT COUNT(*) FROM command_idempotency_records command
                    WHERE command.tenant_id = shortage.tenant_id
                      AND command.operation = 'picking.shortage.report.v1'
                      AND (command.result_json->>'shortage_id')::BIGINT = shortage.id)
                      AS command_count,
                   (SELECT command.inventory_transaction_id
                    FROM command_idempotency_records command
                    WHERE command.tenant_id = shortage.tenant_id
                      AND command.operation = 'picking.shortage.report.v1'
                      AND (command.result_json->>'shortage_id')::BIGINT = shortage.id)
                      AS command_transaction_id,
                   (SELECT COUNT(*) FROM outbox_events event
                    WHERE event.tenant_id = shortage.tenant_id
                      AND event.event_type = 'outbound.pick.shortage_reported'
                      AND event.ordering_key = 'order:' || shortage.order_id::TEXT)
                      AS event_count,
                   (SELECT COUNT(*) FROM order_activity activity
                    WHERE activity.tenant_id = shortage.tenant_id
                      AND activity.order_id = shortage.order_id
                      AND activity.action LIKE 'reported pick shortage on task %')
                      AS activity_count,
                   (SELECT COUNT(*) FROM work_tasks work
                    WHERE work.tenant_id = shortage.tenant_id
                      AND work.facility_id = shortage.facility_id
                      AND work.inventory_owner_id = shortage.inventory_owner_id
                      AND work.task_type LIKE 'cycle_count%') AS count_task_count
            FROM pick_shortages shortage
            INNER JOIN orders
              ON orders.tenant_id = shortage.tenant_id AND orders.id = shortage.order_id
            INNER JOIN pick_tasks task
              ON task.tenant_id = shortage.tenant_id AND task.id = shortage.task_id
            INNER JOIN pick_task_contents content
              ON content.tenant_id = shortage.tenant_id
             AND content.id = shortage.pick_task_content_id
            INNER JOIN inventory_allocations source
              ON source.tenant_id = shortage.tenant_id
             AND source.id = shortage.source_inventory_allocation_id
            INNER JOIN inventory_balances source_balance
              ON source_balance.tenant_id = shortage.tenant_id
             AND source_balance.id = shortage.source_inventory_balance_id
            INNER JOIN inventory_reservations reservation
              ON reservation.tenant_id = shortage.tenant_id
             AND reservation.id = shortage.reservation_id
            INNER JOIN inventory_holds hold
              ON hold.tenant_id = shortage.tenant_id
             AND hold.id = shortage.inventory_hold_id
            WHERE shortage.tenant_id = $1 AND shortage.id = $2
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(report.shortage_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        let event = sqlx::query(
            r#"
            SELECT event_key, aggregate_type, aggregate_id, ordering_key,
                   schema_version, payload, inventory_owner_id, facility_id, actor_user_id
            FROM outbox_events
            WHERE tenant_id = $1
              AND event_type = 'outbound.pick.shortage_reported'
              AND ordering_key = 'order:' || $2::TEXT
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(self.order_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();

        assert_eq!(row.get::<String, _>("order_status"), "processing");
        assert_eq!(row.get::<i64, _>("order_revision"), 4);
        assert_eq!(row.get::<String, _>("task_status"), "shorted");
        assert_eq!(row.get::<String, _>("content_state"), "shorted");
        assert_eq!(row.get::<String, _>("source_status"), "shorted");
        assert_eq!(row.get::<String, _>("source_stage"), "pick_source");
        assert!(row.get::<bool, _>("source_deleted"));
        assert_eq!(row.get::<i64, _>("source_on_hand"), short);
        assert_eq!(row.get::<i64, _>("source_reserved"), 0);
        assert_eq!(row.get::<i64, _>("source_held"), short);
        assert_eq!(row.get::<String, _>("reservation_status"), "active");
        assert_eq!(
            row.get::<i64, _>("reservation_qty"),
            report.quantities.planned
        );
        assert_eq!(row.get::<String, _>("hold_status"), "active");
        assert_eq!(row.get::<i64, _>("hold_qty"), short);
        assert_eq!(row.get::<String, _>("reason_code"), "inventory_discrepancy");
        assert_eq!(
            row.get::<Option<String>, _>("reference_type").as_deref(),
            Some("pick_shortage_source")
        );
        assert_eq!(
            row.get::<Option<i64>, _>("reference_id"),
            Some(self.claim.content.content_id)
        );
        assert_eq!(row.get::<i64, _>("shortage_revision"), 1);
        assert_eq!(
            row.get::<String, _>("shortage_status"),
            "awaiting_inventory"
        );
        assert_eq!(row.get::<i64, _>("report_previous_order_revision"), 3);
        assert_eq!(row.get::<i64, _>("report_resulting_order_revision"), 4);
        assert_eq!(row.get::<i64, _>("reallocated_qty"), 0);
        assert_eq!(row.get::<i64, _>("recovery_terminal_qty"), 0);
        assert_eq!(row.get::<i64, _>("remaining_to_allocate_qty"), short);
        assert_eq!(
            row.get::<i64, _>("confirmation_count"),
            i64::from(picked > 0)
        );
        assert_eq!(row.get::<i64, _>("command_count"), 1);
        assert_eq!(
            row.get::<Option<i64>, _>("command_transaction_id"),
            report
                .movement
                .as_ref()
                .map(|movement| movement.inventory_transaction_id)
        );
        assert_eq!(row.get::<i64, _>("event_count"), 1);
        assert_eq!(row.get::<i64, _>("activity_count"), 1);
        assert_eq!(row.get::<i64, _>("count_task_count"), 0);
        assert_eq!(
            event.get::<String, _>("event_key"),
            format!("pick-shortage:{}", report.shortage_id)
        );
        assert_eq!(event.get::<String, _>("aggregate_type"), "pick_shortage");
        assert_eq!(
            event.get::<String, _>("aggregate_id"),
            report.shortage_id.to_string()
        );
        assert_eq!(
            event.get::<String, _>("ordering_key"),
            format!("order:{}", self.order_id)
        );
        assert_eq!(event.get::<i32, _>("schema_version"), 1);
        assert_eq!(
            event.get::<Option<i64>, _>("inventory_owner_id"),
            Some(self.inventory_owner_id)
        );
        assert_eq!(
            event.get::<Option<i64>, _>("facility_id"),
            Some(self.facility_id)
        );
        assert_eq!(
            event.get::<Option<i64>, _>("actor_user_id"),
            Some(self.access.user_id.get())
        );
        assert_eq!(
            event.get::<Value, _>("payload"),
            json!({
                "pick_shortage_id": report.shortage_id,
                "pick_task_id": report.task_id,
                "pick_content_id": report.content_id,
                "order_id": report.order_id,
                "planned_quantity": report.quantities.planned,
                "picked_quantity": report.quantities.picked,
                "short_quantity": report.quantities.short,
                "reason": report.details.reason,
                "inventory_hold_id": report.hold.hold_id,
                "inventory_transaction_id": report
                    .movement
                    .as_ref()
                    .map(|movement| movement.inventory_transaction_id),
                "order_revision": report.order_revision,
            })
        );
    }

    pub(super) async fn assert_one_conserved_move(
        &self,
        report: &ReportPickShortageResponse,
        quantity: i64,
    ) {
        let movement = report.movement.as_ref().expect("partial movement");
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let row = sqlx::query(
            r#"
            SELECT confirmation.picked_qty,
                   source.status AS source_status,
                   source.execution_stage AS source_stage,
                   destination.status AS destination_status,
                   destination.execution_stage AS destination_stage,
                   destination.qty AS destination_qty,
                   source_balance.qty_on_hand AS source_on_hand,
                   source_balance.qty_reserved AS source_reserved,
                   destination_balance.qty_on_hand AS destination_on_hand,
                   destination_balance.qty_reserved AS destination_reserved,
                   (SELECT COUNT(*) FROM inventory_entries entry
                    WHERE entry.tenant_id = confirmation.tenant_id
                      AND entry.transaction_id = confirmation.inventory_transaction_id)
                      AS entry_count,
                   (SELECT COALESCE(SUM(entry.quantity_delta), 0)::BIGINT
                    FROM inventory_entries entry
                    WHERE entry.tenant_id = confirmation.tenant_id
                      AND entry.transaction_id = confirmation.inventory_transaction_id)
                      AS journal_net,
                   (SELECT COALESCE(SUM(ABS(entry.quantity_delta)), 0)::BIGINT
                    FROM inventory_entries entry
                    WHERE entry.tenant_id = confirmation.tenant_id
                      AND entry.transaction_id = confirmation.inventory_transaction_id)
                      AS journal_volume
            FROM pick_confirmations confirmation
            INNER JOIN inventory_allocations source
              ON source.tenant_id = confirmation.tenant_id
             AND source.id = confirmation.source_inventory_allocation_id
            INNER JOIN inventory_allocations destination
              ON destination.tenant_id = confirmation.tenant_id
             AND destination.id = confirmation.destination_inventory_allocation_id
            INNER JOIN inventory_balances source_balance
              ON source_balance.tenant_id = confirmation.tenant_id
             AND source_balance.id = confirmation.source_inventory_balance_id
            INNER JOIN inventory_balances destination_balance
              ON destination_balance.tenant_id = confirmation.tenant_id
             AND destination_balance.id = confirmation.destination_inventory_balance_id
            WHERE confirmation.tenant_id = $1
              AND confirmation.inventory_transaction_id = $2
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(movement.inventory_transaction_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();

        assert_eq!(row.get::<i64, _>("picked_qty"), quantity);
        assert_eq!(row.get::<String, _>("source_status"), "shorted");
        assert_eq!(row.get::<String, _>("source_stage"), "pick_source");
        assert_eq!(row.get::<String, _>("destination_status"), "allocated");
        assert_eq!(row.get::<String, _>("destination_stage"), "staged");
        assert_eq!(row.get::<i64, _>("destination_qty"), quantity);
        assert_eq!(row.get::<i64, _>("source_on_hand"), report.quantities.short);
        assert_eq!(row.get::<i64, _>("source_reserved"), 0);
        assert_eq!(row.get::<i64, _>("destination_on_hand"), quantity);
        assert_eq!(row.get::<i64, _>("destination_reserved"), quantity);
        assert_eq!(row.get::<i64, _>("entry_count"), 2);
        assert_eq!(row.get::<i64, _>("journal_net"), 0);
        assert_eq!(row.get::<i64, _>("journal_volume"), quantity * 2);
    }

    pub(super) async fn shortage_count(&self) -> i64 {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let count = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pick_shortages WHERE tenant_id = $1 AND order_id = $2",
        )
        .bind(self.access.tenant_id.get())
        .bind(self.order_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        count
    }

    pub(super) async fn assert_shortage_count(&self, expected: i64) {
        assert_eq!(self.shortage_count().await, expected);
    }

    pub(super) async fn assert_confirmation_quantities(&self, expected: &[i64]) {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let rows: Vec<i64> = sqlx::query_scalar(
            r#"
            SELECT picked_qty
            FROM pick_confirmations
            WHERE tenant_id = $1 AND order_id = $2
            ORDER BY id
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(self.order_id)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(rows, expected);
    }

    pub(super) async fn assert_reallocation_count(&self, shortage_id: i64, expected: i64) {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pick_shortage_reallocation_runs WHERE tenant_id = $1 AND pick_shortage_id = $2",
        )
        .bind(self.access.tenant_id.get())
        .bind(shortage_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(count, expected);
    }

    pub(super) async fn grant_supervisor(&self) {
        grant_permissions(
            &self.fixture.db,
            self.access.tenant_id,
            self.access.user_id.get(),
            &format!("shortage-supervisor-{}", self.access.user_id.get()),
            &["wms_supervisor"],
        )
        .await;
    }

    pub(super) async fn revoke_scope(&self) {
        self.set_scope(Vec::new(), Vec::new()).await;
    }

    pub(super) async fn set_scope(&self, facility_ids: Vec<i64>, inventory_owner_ids: Vec<i64>) {
        assert!(wareboxes_api::repo::tenants::update_user_access_scope(
            &self.fixture.db,
            self.access.tenant_id,
            &UpdateUserAccessScope {
                user_id: self.access.user_id.get(),
                all_facilities: false,
                facility_ids,
                all_inventory_owners: false,
                inventory_owner_ids,
            },
        )
        .await
        .unwrap());
    }

    pub(super) async fn cross_tenant_user(&self, email: &str) -> CrossTenantUser {
        let user = self.fixture.wms_user(email).await;
        let tenant_id = tenant_for_user(&self.fixture.db, user.id).await;
        let token = auth::create_session(&self.fixture.db, user.id)
            .await
            .unwrap();
        CrossTenantUser { tenant_id, token }
    }

    pub(super) async fn operator_only_token(&self, email: &str) -> String {
        let user = self.fixture.user(email).await;
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
            .bind(self.access.tenant_id.get())
            .bind(user.id)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let permission = wareboxes_persistence_postgres::permissions::find_by_name(
            &self.fixture.db,
            self.access.tenant_id,
            "wms",
        )
        .await
        .unwrap()
        .expect("fixture tenant has wms permission");
        let role = wareboxes_persistence_postgres::roles::add_role(
            &self.fixture.db,
            self.access.tenant_id,
            &format!("{email}-operator"),
            Some("RF picking operator without supervisor recovery access"),
        )
        .await
        .unwrap();
        assert!(wareboxes_persistence_postgres::roles::add_role_permission(
            &self.fixture.db,
            self.access.tenant_id,
            role,
            permission.id,
        )
        .await
        .unwrap());
        assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
            &self.fixture.db,
            self.access.tenant_id,
            user.id,
            role,
        )
        .await
        .unwrap());
        assert!(wareboxes_api::repo::tenants::update_user_access_scope(
            &self.fixture.db,
            self.access.tenant_id,
            &UpdateUserAccessScope {
                user_id: user.id,
                all_facilities: false,
                facility_ids: vec![self.facility_id],
                all_inventory_owners: false,
                inventory_owner_ids: vec![self.inventory_owner_id],
            },
        )
        .await
        .unwrap());
        auth::create_session(&self.fixture.db, user.id)
            .await
            .unwrap()
    }
}

pub(super) async fn send(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    idempotency_key: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = idempotency_key {
        request = request
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .header(REQUEST_ID_HEADER, format!("request-{key}"));
    }
    let body = if let Some(body) = body {
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

pub(super) async fn response_json<T: DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

pub(super) async fn response_json_value(response: axum::response::Response) -> Value {
    response_json(response).await
}

pub(super) async fn expect_status(
    response: axum::response::Response,
    expected: StatusCode,
    operation: &str,
) -> axum::response::Response {
    if response.status() != expected {
        let actual = response.status();
        let body = response_json_value(response).await;
        panic!("{operation}: expected {expected}, got {actual}: {body}");
    }
    response
}

async fn grant_permissions(
    db: &wareboxes_persistence_postgres::db::Db,
    tenant_id: TenantId,
    user_id: i64,
    role_name: &str,
    permission_names: &[&str],
) {
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        &format!("{role_name}-shortage-supervisor"),
        Some("Shortage reallocation supervisor"),
    )
    .await
    .unwrap();
    for permission_name in permission_names {
        let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
            db,
            tenant_id,
            permission_name,
        )
        .await
        .unwrap()
        {
            Some(permission) => permission.id,
            None => wareboxes_persistence_postgres::permissions::add_permission(
                db,
                tenant_id,
                permission_name,
                Some(match *permission_name {
                    "orders" => "Fulfillment orders",
                    "wms_supervisor" => "WMS supervisor exception recovery",
                    value => value,
                }),
            )
            .await
            .unwrap(),
        };
        assert!(wareboxes_persistence_postgres::roles::add_role_permission(
            db, tenant_id, role, permission,
        )
        .await
        .unwrap());
    }
    assert!(
        wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role,)
            .await
            .unwrap()
    );
}

async fn staging_location(
    fixture: &Fixture,
    tenant_id: TenantId,
    facility_id: i64,
    barcode: &str,
) -> i64 {
    wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some(barcode),
        Some(barcode),
        "staging",
        true,
        false,
        false,
    )
    .await
    .unwrap()
}

async fn plate_at(
    fixture: &Fixture,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    barcode: &str,
) -> i64 {
    let plate_id = wareboxes_api::repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        facility_id,
        Some(barcode),
    )
    .await
    .unwrap();
    let admin = admin_db_for(&fixture.db).await;
    sqlx::query("UPDATE license_plates SET location_id = $1 WHERE tenant_id = $2 AND id = $3")
        .bind(location_id)
        .bind(tenant_id.get())
        .bind(plate_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    plate_id
}

pub(super) fn reallocation_body(shortage_revision: i64, order_revision: i64) -> Value {
    json!({
        "expected_shortage_revision": shortage_revision,
        "expected_order_revision": order_revision
    })
}

pub(super) fn assert_report_contract(
    report: &ReportPickShortageResponse,
    fixture: &PickShortageFixture,
    picked: i64,
    short: i64,
    status: PickShortageStatus,
) {
    assert!(report.shortage_id > 0);
    assert_eq!(report.shortage_revision.get(), 1);
    assert_eq!(report.order_id, fixture.order_id);
    assert_eq!(report.task_id, fixture.claim.task_id);
    assert_eq!(report.content_id, fixture.claim.content.content_id);
    assert_eq!(
        report.quantities.planned,
        fixture.claim.content.planned_quantity
    );
    assert_eq!(report.quantities.picked, picked);
    assert_eq!(report.quantities.short, short);
    assert_eq!(report.shortage_status, status);
    assert_eq!(report.order_revision.get(), 4);
    assert_eq!(report.reported_by, fixture.access.user_id.get());
    assert!(report.hold.hold_id > 0);
    assert_eq!(
        report.hold.inventory_balance_id,
        fixture.source_balance.balance_id
    );
    assert_eq!(report.hold.held_quantity, short);
    assert_eq!(report.reallocated_quantity, 0);
    assert_eq!(report.recovery_terminal_quantity, 0);
    assert_eq!(report.remaining_to_allocate_quantity, short);
}

pub(super) fn assert_partial_movement_contract(
    report: &ReportPickShortageResponse,
    fixture: &PickShortageFixture,
    quantity: i64,
) {
    let movement = report
        .movement
        .as_ref()
        .unwrap_or_else(|| panic!("partial short is missing movement: {report:?}"));
    assert_eq!(movement.picked_quantity, quantity);
    assert_eq!(
        movement.destination_license_plate_id,
        fixture.destination_plate_id
    );
    assert_eq!(
        movement.source_inventory_balance_id,
        fixture.source_balance.balance_id
    );
    assert_eq!(
        movement.source_location_id,
        fixture.source_balance.location_id
    );
    assert_eq!(
        movement.destination_location_id,
        fixture.destination_location_id
    );
    assert_eq!(movement.destination_stage, AllocationExecutionStage::Staged);
    assert!(movement.inventory_transaction_id > 0);
}

pub(super) fn assert_reallocation_contract(
    result: &ReallocatePickShortageResponse,
    shortage_id: i64,
    outcome: OrderAllocationOutcome,
    progress: AllocationProgress,
) {
    assert!(result.reallocation_run_id > 0);
    assert_eq!(result.shortage_id, shortage_id);
    assert_eq!(result.outcome, outcome);
    assert_eq!(result.newly_allocated_quantity, progress.newly_allocated);
    assert_eq!(result.reallocated_quantity, progress.recovery_allocated);
    assert_eq!(result.recovery_terminal_quantity, progress.recovery_picked);
    assert_eq!(result.remaining_to_allocate_quantity, progress.remaining);
    assert_eq!(result.shortage_status, progress.status);
    assert_eq!(result.new_tasks.len(), progress.task_count);
    assert_eq!(result.new_allocations.len(), progress.task_count);
    assert_eq!(
        result
            .new_allocations
            .iter()
            .map(|allocation| allocation.quantity)
            .sum::<i64>(),
        progress.newly_allocated
    );
    assert!(result
        .new_allocations
        .iter()
        .all(|allocation| allocation.execution_stage == AllocationExecutionStage::PickSource));
    for task in &result.new_tasks {
        let allocation = result
            .new_allocations
            .iter()
            .find(|allocation| allocation.allocation_id == task.source_allocation_id)
            .expect("each recovery task references a returned allocation");
        assert_eq!(
            task.source_inventory_balance_id,
            allocation.inventory_balance_id
        );
        assert_eq!(task.source_location_id, allocation.location_id);
        assert_eq!(task.planned_quantity, allocation.quantity);
    }
}

fn merge_json(mut base: Value, overlay: Value) -> Value {
    {
        let base = base.as_object_mut().expect("base JSON object");
        let overlay = overlay.as_object().expect("overlay JSON object");
        for (key, value) in overlay {
            base.insert(key.clone(), value.clone());
        }
    }
    base
}
