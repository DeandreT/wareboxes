use serde::{Deserialize, Serialize};

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
}
