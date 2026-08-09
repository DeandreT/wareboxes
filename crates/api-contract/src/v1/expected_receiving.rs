use serde::{Deserialize, Serialize};

use super::InventoryBalanceStatus;

/// Inbound load states visible to an expected-receiving scanner session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedReceivingLoadStatus {
    Arrived,
    Receiving,
    Received,
}

/// Directed receiving location the operator must scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedReceivingLocation {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

/// One expected load line presented to a receiving operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedReceiptLine {
    pub load_line_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub item_barcodes: Vec<String>,
    pub expected_quantity: i64,
    pub received_quantity: i64,
    pub rejected_quantity: i64,
    pub missing_quantity: i64,
    pub remaining_quantity: i64,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
}

/// Scanner-oriented view of one inbound load's expected receiving work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedReceivingSessionResponse {
    pub load_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub reference_number: Option<String>,
    pub status: ExpectedReceivingLoadStatus,
    pub receiving_location: ExpectedReceivingLocation,
    pub lines: Vec<ExpectedReceiptLine>,
}

/// Operator disposition recorded by one expected-receipt command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedReceiptDisposition {
    Received,
    Quarantined,
    Rejected,
    Missing,
}

/// Typed reason for receiving a physically present expected unit into quarantine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedReceiptQuarantineReason {
    Damaged,
    QualityInspection,
    CountDiscrepancy,
    WrongItem,
    Other,
}

/// Typed reason for rejecting or marking expected inventory missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedReceiptExceptionReason {
    Damaged,
    QualityRejected,
    ShortShipment,
    CountDiscrepancy,
    WrongItem,
    Other,
}

/// Resolves one positive quantity against an expected receipt line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfirmExpectedReceiptRequest {
    Received {
        item_barcode: String,
        receiving_location_barcode: String,
        quantity: i64,
        license_plate_barcode: Option<String>,
        lot: Option<String>,
        serial: Option<String>,
        expiration: Option<String>,
    },
    Quarantined {
        item_barcode: String,
        receiving_location_barcode: String,
        quantity: i64,
        license_plate_barcode: Option<String>,
        lot: Option<String>,
        serial: Option<String>,
        expiration: Option<String>,
        reason: ExpectedReceiptQuarantineReason,
        note: Option<String>,
    },
    Rejected {
        item_barcode: String,
        quantity: i64,
        reason: ExpectedReceiptExceptionReason,
        note: Option<String>,
    },
    Missing {
        quantity: i64,
        reason: ExpectedReceiptExceptionReason,
        note: Option<String>,
    },
}

/// Cumulative resolution state for one expected receipt line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedReceiptLineStatus {
    Pending,
    Partial,
    Received,
    Rejected,
    Missing,
}

/// Result of atomically resolving one expected-receipt quantity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedReceiptConfirmationResponse {
    pub load_id: i64,
    pub load_line_id: i64,
    pub disposition: ExpectedReceiptDisposition,
    pub quantity: i64,
    pub inventory_transaction_id: Option<i64>,
    pub inventory_balance_id: Option<i64>,
    pub item_batch_id: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub inventory_hold_id: Option<i64>,
    pub inventory_status: Option<InventoryBalanceStatus>,
    pub line_status: ExpectedReceiptLineStatus,
    pub load_status: ExpectedReceivingLoadStatus,
    pub cumulative_received_quantity: i64,
    pub cumulative_rejected_quantity: i64,
    pub cumulative_missing_quantity: i64,
    pub remaining_quantity: i64,
    pub receive_completed: bool,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn session() -> ExpectedReceivingSessionResponse {
        ExpectedReceivingSessionResponse {
            load_id: 11,
            inventory_owner_id: 22,
            facility_id: 33,
            reference_number: Some("ASN-1001".into()),
            status: ExpectedReceivingLoadStatus::Receiving,
            receiving_location: ExpectedReceivingLocation {
                location_id: 44,
                barcode: "DOCK-04".into(),
                name: Some("Inbound Dock 4".into()),
            },
            lines: vec![ExpectedReceiptLine {
                load_line_id: 55,
                item_id: 66,
                item_description: Some("Case-picked item".into()),
                uom: "case".into(),
                item_barcodes: vec!["0012345678905".into(), "CASE-66".into()],
                expected_quantity: 12,
                received_quantity: 4,
                rejected_quantity: 1,
                missing_quantity: 0,
                remaining_quantity: 7,
                lot: Some("LOT-07".into()),
                serial: None,
                expiration: Some("2027-07-26T00:00:00+00:00".into()),
            }],
        }
    }

    #[test]
    fn expected_receiving_session_has_an_exact_scanner_contract() {
        let response = session();
        let value = serde_json::to_value(&response).unwrap();

        assert_eq!(
            value,
            json!({
                "load_id": 11,
                "inventory_owner_id": 22,
                "facility_id": 33,
                "reference_number": "ASN-1001",
                "status": "receiving",
                "receiving_location": {
                    "location_id": 44,
                    "barcode": "DOCK-04",
                    "name": "Inbound Dock 4"
                },
                "lines": [{
                    "load_line_id": 55,
                    "item_id": 66,
                    "item_description": "Case-picked item",
                    "uom": "case",
                    "item_barcodes": ["0012345678905", "CASE-66"],
                    "expected_quantity": 12,
                    "received_quantity": 4,
                    "rejected_quantity": 1,
                    "missing_quantity": 0,
                    "remaining_quantity": 7,
                    "lot": "LOT-07",
                    "serial": null,
                    "expiration": "2027-07-26T00:00:00+00:00"
                }]
            })
        );
        assert_eq!(
            serde_json::from_value::<ExpectedReceivingSessionResponse>(value).unwrap(),
            response
        );
    }

    #[test]
    fn expected_receiving_contract_rejects_unknown_fields_at_every_level() {
        let mut value = serde_json::to_value(session()).unwrap();
        value["tenant_id"] = json!(99);
        assert!(serde_json::from_value::<ExpectedReceivingSessionResponse>(value).is_err());

        assert!(serde_json::from_value::<ExpectedReceivingLocation>(json!({
            "location_id": 44,
            "barcode": "DOCK-04",
            "name": null,
            "active": true
        }))
        .is_err());

        let mut line = serde_json::to_value(&session().lines[0]).unwrap();
        line["deleted"] = json!(null);
        assert!(serde_json::from_value::<ExpectedReceiptLine>(line).is_err());
    }

    #[test]
    fn expected_receiving_session_excludes_internal_metadata() {
        let value = serde_json::to_value(session()).unwrap();

        for field in [
            "tenant_id",
            "created",
            "modified",
            "deleted",
            "task_id",
            "assigned_user_id",
            "required_permission",
            "lease_expires_at",
        ] {
            assert!(
                value.get(field).is_none(),
                "unexpected session field {field}"
            );
            assert!(
                value["receiving_location"].get(field).is_none(),
                "unexpected location field {field}"
            );
            assert!(
                value["lines"][0].get(field).is_none(),
                "unexpected line field {field}"
            );
        }
    }

    #[test]
    fn expected_receiving_load_status_supports_completion_reconciliation() {
        assert_eq!(
            serde_json::to_string(&ExpectedReceivingLoadStatus::Arrived).unwrap(),
            r#""arrived""#
        );
        assert_eq!(
            serde_json::to_string(&ExpectedReceivingLoadStatus::Received).unwrap(),
            r#""received""#
        );
        assert!(serde_json::from_str::<ExpectedReceivingLoadStatus>(r#""scheduled""#).is_err());
    }

    #[test]
    fn received_confirmation_request_has_an_exact_contract() {
        let request = ConfirmExpectedReceiptRequest::Received {
            item_barcode: "0012345678905".into(),
            receiving_location_barcode: "DOCK-04".into(),
            quantity: 4,
            license_plate_barcode: Some("LP-1004".into()),
            lot: Some("LOT-07".into()),
            serial: None,
            expiration: Some("2027-07-26T00:00:00+00:00".into()),
        };
        let value = serde_json::to_value(&request).unwrap();

        assert_eq!(
            value,
            json!({
                "disposition": "received",
                "item_barcode": "0012345678905",
                "receiving_location_barcode": "DOCK-04",
                "quantity": 4,
                "license_plate_barcode": "LP-1004",
                "lot": "LOT-07",
                "serial": null,
                "expiration": "2027-07-26T00:00:00+00:00"
            })
        );
        assert_eq!(
            serde_json::from_value::<ConfirmExpectedReceiptRequest>(value).unwrap(),
            request
        );
    }

    #[test]
    fn rejected_confirmation_request_has_an_exact_contract() {
        let request = ConfirmExpectedReceiptRequest::Rejected {
            item_barcode: "CASE-66".into(),
            quantity: 2,
            reason: ExpectedReceiptExceptionReason::QualityRejected,
            note: Some("Seal was broken".into()),
        };
        let value = serde_json::to_value(&request).unwrap();

        assert_eq!(
            value,
            json!({
                "disposition": "rejected",
                "item_barcode": "CASE-66",
                "quantity": 2,
                "reason": "quality_rejected",
                "note": "Seal was broken"
            })
        );
        assert_eq!(
            serde_json::from_value::<ConfirmExpectedReceiptRequest>(value).unwrap(),
            request
        );
    }

    #[test]
    fn quarantined_confirmation_request_has_an_exact_contract() {
        let request = ConfirmExpectedReceiptRequest::Quarantined {
            item_barcode: "CASE-66".into(),
            receiving_location_barcode: "DOCK-04".into(),
            quantity: 2,
            license_plate_barcode: Some("LP-QA-66".into()),
            lot: Some("LOT-07".into()),
            serial: None,
            expiration: Some("2027-07-26T00:00:00+00:00".into()),
            reason: ExpectedReceiptQuarantineReason::Damaged,
            note: Some("Outer case was crushed".into()),
        };
        let value = serde_json::to_value(&request).unwrap();

        assert_eq!(
            value,
            json!({
                "disposition": "quarantined",
                "item_barcode": "CASE-66",
                "receiving_location_barcode": "DOCK-04",
                "quantity": 2,
                "license_plate_barcode": "LP-QA-66",
                "lot": "LOT-07",
                "serial": null,
                "expiration": "2027-07-26T00:00:00+00:00",
                "reason": "damaged",
                "note": "Outer case was crushed"
            })
        );
        assert_eq!(
            serde_json::from_value::<ConfirmExpectedReceiptRequest>(value).unwrap(),
            request
        );
    }

    #[test]
    fn missing_confirmation_request_has_an_exact_contract() {
        let request = ConfirmExpectedReceiptRequest::Missing {
            quantity: 3,
            reason: ExpectedReceiptExceptionReason::ShortShipment,
            note: None,
        };
        let value = serde_json::to_value(&request).unwrap();

        assert_eq!(
            value,
            json!({
                "disposition": "missing",
                "quantity": 3,
                "reason": "short_shipment",
                "note": null
            })
        );
        assert_eq!(
            serde_json::from_value::<ConfirmExpectedReceiptRequest>(value).unwrap(),
            request
        );
    }

    #[test]
    fn confirmation_request_rejects_unknown_and_impossible_stock_fields() {
        assert!(
            serde_json::from_value::<ConfirmExpectedReceiptRequest>(json!({
                "disposition": "received",
                "item_barcode": "CASE-66",
                "receiving_location_barcode": "DOCK-04",
                "quantity": 1,
                "license_plate_barcode": null,
                "lot": null,
                "serial": null,
                "expiration": null,
                "tenant_id": 99
            }))
            .is_err()
        );

        assert!(
            serde_json::from_value::<ConfirmExpectedReceiptRequest>(json!({
                "disposition": "rejected",
                "item_barcode": "CASE-66",
                "quantity": 1,
                "reason": "damaged",
                "note": null,
                "receiving_location_barcode": "DOCK-04",
                "license_plate_barcode": "LP-1004"
            }))
            .is_err()
        );

        assert!(
            serde_json::from_value::<ConfirmExpectedReceiptRequest>(json!({
                "disposition": "missing",
                "quantity": 1,
                "reason": "short_shipment",
                "note": null,
                "item_barcode": "CASE-66",
                "lot": "LOT-07"
            }))
            .is_err()
        );
    }

    #[test]
    fn expected_receipt_confirmation_response_has_an_exact_public_contract() {
        let response = ExpectedReceiptConfirmationResponse {
            load_id: 11,
            load_line_id: 55,
            disposition: ExpectedReceiptDisposition::Received,
            quantity: 4,
            inventory_transaction_id: Some(77),
            inventory_balance_id: Some(88),
            item_batch_id: Some(99),
            license_plate_id: Some(111),
            inventory_hold_id: None,
            inventory_status: Some(InventoryBalanceStatus::Available),
            line_status: ExpectedReceiptLineStatus::Partial,
            load_status: ExpectedReceivingLoadStatus::Receiving,
            cumulative_received_quantity: 4,
            cumulative_rejected_quantity: 1,
            cumulative_missing_quantity: 0,
            remaining_quantity: 7,
            receive_completed: false,
        };
        let value = serde_json::to_value(&response).unwrap();

        assert_eq!(
            value,
            json!({
                "load_id": 11,
                "load_line_id": 55,
                "disposition": "received",
                "quantity": 4,
                "inventory_transaction_id": 77,
                "inventory_balance_id": 88,
                "item_batch_id": 99,
                "license_plate_id": 111,
                "inventory_hold_id": null,
                "inventory_status": "available",
                "line_status": "partial",
                "load_status": "receiving",
                "cumulative_received_quantity": 4,
                "cumulative_rejected_quantity": 1,
                "cumulative_missing_quantity": 0,
                "remaining_quantity": 7,
                "receive_completed": false
            })
        );
        for field in [
            "tenant_id",
            "created",
            "deleted",
            "actor_user_id",
            "request_hash",
            "idempotency_key",
        ] {
            assert!(
                value.get(field).is_none(),
                "unexpected confirmation field {field}"
            );
        }
    }
}
