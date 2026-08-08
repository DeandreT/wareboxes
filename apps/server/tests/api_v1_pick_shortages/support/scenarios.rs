use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use sqlx::Row;
use wareboxes_api_contract::v1::{
    PickClaimResponse, PickContentConfirmationResponse, ReportPickShortageResponse,
};

use super::{
    admin_db_for, expect_status, response_json, tenant_tx, PickShortageFixture,
    ReceivedBalanceSetup, RecoveryProgressExpectation,
};

impl PickShortageFixture {
    pub(crate) async fn claim_next(&self, key: &str) -> PickClaimResponse {
        let response = self
            .request(
                Method::POST,
                "/api/v1/picking-claims/next",
                Some(key),
                Some(json!({})),
            )
            .await;
        let response = expect_status(response, StatusCode::OK, "claim next pick").await;
        response_json::<Option<PickClaimResponse>>(response)
            .await
            .expect("fixture has executable pick work")
    }

    pub(crate) fn no_pick_body_for_claim(claim: &PickClaimResponse) -> Value {
        json!({
            "source_location_barcode": claim.content.source_location_barcode,
            "source_license_plate_barcode": claim.content.source_license_plate_barcode,
            "observed_item_barcode": Value::Null,
            "observed_lot": Value::Null,
            "observed_serial": Value::Null,
            "outcome": { "kind": "no_pick" },
            "details": { "reason": "inventory_missing", "note": Value::Null }
        })
    }

    pub(crate) async fn report_claim(
        &self,
        claim: &PickClaimResponse,
        key: &str,
        body: Value,
    ) -> axum::response::Response {
        self.request(
            Method::POST,
            &format!(
                "/api/v1/picking-tasks/{}/contents/{}/short-picks",
                claim.task_id, claim.content.content_id
            ),
            Some(key),
            Some(body),
        )
        .await
    }

    pub(crate) fn confirmation_body_for_claim(&self, claim: &PickClaimResponse) -> Value {
        json!({
            "source_location_barcode": claim.content.source_location_barcode,
            "item_barcode": claim.content.item_barcodes[0],
            "source_license_plate_barcode": claim.content.source_license_plate_barcode,
            "destination_license_plate_barcode": self.destination_plate_barcode
        })
    }

    pub(crate) async fn confirm_claim(
        &self,
        claim: &PickClaimResponse,
        key: &str,
        body: Value,
    ) -> axum::response::Response {
        self.request(
            Method::POST,
            &format!(
                "/api/v1/picking-tasks/{}/contents/{}/confirmations",
                claim.task_id, claim.content.content_id
            ),
            Some(key),
            Some(body),
        )
        .await
    }

    pub(crate) async fn create_additional_shortage(
        &self,
        key: &str,
        quantity: i64,
    ) -> ReportPickShortageResponse {
        let order_id = self
            .fixture
            .order_header(
                self.access.tenant_id,
                &format!("{key}-ORDER"),
                self.inventory_owner_id,
            )
            .await;
        self.fixture
            .order_item(self.access.tenant_id, order_id, self.item_id, quantity)
            .await;
        self.fixture
            .received_balance(
                &self.access,
                ReceivedBalanceSetup {
                    inventory_owner_id: self.inventory_owner_id,
                    facility_id: self.facility_id,
                    item_id: self.item_id,
                    qty: quantity,
                    key: &format!("{key}-SOURCE"),
                },
            )
            .await;

        let allocation = self
            .request(
                Method::POST,
                &format!("/api/v1/orders/{order_id}/allocation-runs"),
                Some(&format!("{key}-allocate")),
                Some(json!({
                    "facility_id": self.facility_id,
                    "expected_revision": 1,
                    "strategy": "fefo"
                })),
            )
            .await;
        expect_status(
            allocation,
            StatusCode::OK,
            "allocate additional shortage order",
        )
        .await;
        let release = self
            .request(
                Method::POST,
                &format!("/api/v1/orders/{order_id}/releases"),
                Some(&format!("{key}-release")),
                Some(json!({
                    "facility_id": self.facility_id,
                    "destination_location_id": self.destination_location_id,
                    "expected_revision": 2
                })),
            )
            .await;
        expect_status(release, StatusCode::OK, "release additional shortage order").await;
        let claim = self.claim_next(&format!("{key}-claim")).await;
        assert_eq!(claim.order_id, order_id);
        let body = Self::no_pick_body_for_claim(&claim);
        let report = self
            .report_claim(&claim, &format!("{key}-report"), body)
            .await;
        let report = expect_status(report, StatusCode::OK, "report additional shortage").await;
        response_json(report).await
    }

    pub(crate) async fn make_destination_a_packing_station(&self) {
        let admin = admin_db_for(&self.fixture.db).await;
        let updated = sqlx::query(
            "UPDATE locations SET type = 'packing' WHERE tenant_id = $1 AND facility_id = $2 AND id = $3",
        )
        .bind(self.access.tenant_id.get())
        .bind(self.facility_id)
        .bind(self.destination_location_id)
        .execute(&admin)
        .await
        .expect("convert fixture destination to packing station");
        assert_eq!(updated.rows_affected(), 1);
        admin.close().await;
    }

    pub(crate) async fn open_packing_session(
        &self,
        order_revision: i64,
        key: &str,
    ) -> axum::response::Response {
        self.request(
            Method::POST,
            &format!("/api/v1/orders/{}/packing-sessions", self.order_id),
            Some(key),
            Some(json!({
                "facility_id": self.facility_id,
                "station_location_id": self.destination_location_id,
                "expected_revision": order_revision
            })),
        )
        .await
    }

    pub(crate) async fn assert_recovery_progress_event(
        &self,
        expected: RecoveryProgressExpectation,
    ) {
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        let rows = sqlx::query(
            r#"
            SELECT event.event_key, event.aggregate_type, event.aggregate_id,
                   event.ordering_key, event.aggregate_sequence, event.schema_version,
                   event.payload, event.inventory_owner_id, event.facility_id,
                   event.actor_user_id,
                   (SELECT previous.event_type
                    FROM outbox_events previous
                    WHERE previous.tenant_id = event.tenant_id
                      AND previous.ordering_key = event.ordering_key
                      AND previous.aggregate_sequence = event.aggregate_sequence - 1)
                       AS preceding_event_type
            FROM outbox_events event
            WHERE event.tenant_id = $1
              AND event.event_type = 'outbound.pick.shortage_recovery_progressed'
              AND event.aggregate_id = $2
            ORDER BY event.aggregate_sequence
            "#,
        )
        .bind(self.access.tenant_id.get())
        .bind(expected.shortage_id.to_string())
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();

        assert_eq!(rows.len(), 1, "recovery progress event must be replay safe");
        let event = &rows[0];
        assert_eq!(
            event.get::<String, _>("event_key"),
            format!(
                "pick-shortage:{}:recovery:{}",
                expected.shortage_id, expected.shortage_revision
            )
        );
        assert_eq!(event.get::<String, _>("aggregate_type"), "pick_shortage");
        assert_eq!(
            event.get::<String, _>("aggregate_id"),
            expected.shortage_id.to_string()
        );
        assert_eq!(
            event.get::<String, _>("ordering_key"),
            format!("order:{}", self.order_id)
        );
        assert!(event.get::<i64, _>("aggregate_sequence") > 1);
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
            event
                .get::<Option<String>, _>("preceding_event_type")
                .as_deref(),
            Some(expected.preceding_event_type)
        );
        assert_eq!(
            event.get::<Value, _>("payload"),
            json!({
                "pick_shortage_id": expected.shortage_id,
                "shortage_revision": expected.shortage_revision,
                "shortage_status": expected.shortage_status,
                "order_id": self.order_id,
                "order_revision": expected.order_revision,
                "reallocated_quantity": expected.terminal_quantity,
                "recovery_terminal_quantity": expected.terminal_quantity,
                "remaining_to_allocate_quantity": 0,
                "trigger_pick_task_id": expected.trigger_task_id,
                "trigger_source_inventory_allocation_id": expected.trigger_source_allocation_id,
                "terminal_quantity": expected.terminal_quantity,
            })
        );
    }

    pub(crate) async fn parse_confirmation(
        response: axum::response::Response,
        context: &str,
    ) -> PickContentConfirmationResponse {
        let response = expect_status(response, StatusCode::OK, context).await;
        response_json(response).await
    }
}
