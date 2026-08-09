use serde::{Deserialize, Serialize};

use super::Revision;

/// Public lifecycle of one parcel shipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShipmentStatus {
    AwaitingManifest,
    Manifested,
    PartiallyDeparted,
    Departed,
}

/// Order state exposed by the parcel-shipment workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShipmentOrderStatus {
    AwaitingShipment,
    Shipped,
}

/// Creates one shipment from a ready packing session at the observed order revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateShipmentRequest {
    pub packing_session_id: i64,
    pub expected_revision: Revision,
}

/// Assigns one carrier tracking number to one shipment carton.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualCartonTrackingRequest {
    pub carton_id: i64,
    pub tracking_number: String,
}

/// Records a manual carrier manifest for every carton in one shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordManualManifestRequest {
    pub carrier_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_code: Option<String>,
    pub manifest_reference: String,
    pub carton_tracking_assignments: Vec<ManualCartonTrackingRequest>,
    pub expected_revision: Revision,
}

/// Confirms departure using a nonempty subset of remaining shipment carton barcodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmShipmentDepartureRequest {
    pub scanned_carton_barcodes: Vec<String>,
    pub expected_shipment_revision: Revision,
    pub expected_order_revision: Revision,
}

/// Generates the immutable packing slip for one shipment revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratePackingSlipRequest {
    pub expected_shipment_revision: Revision,
}

/// Generates the immutable carton-label set for one manifested shipment revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateCartonLabelSetRequest {
    pub expected_shipment_revision: Revision,
}

/// Shipment document kinds exposed by the public API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShipmentDocumentType {
    PackingSlip,
    CartonLabelSet,
}

/// Immutable metadata for one retained shipment document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShipmentDocumentResponse {
    pub document_id: i64,
    pub shipment_id: i64,
    pub order_id: i64,
    pub document_type: ShipmentDocumentType,
    pub manifest_id: Option<i64>,
    pub carrier_code: Option<String>,
    pub service_code: Option<String>,
    pub manifest_reference: Option<String>,
    pub file_name: String,
    pub media_type: String,
    pub content_length: i64,
    pub content_sha256: String,
    pub shipment_revision_at_generation: Revision,
    pub carton_count: i64,
    pub line_count: i64,
    pub demand: ShipmentDemandResponse,
    pub generated_by: i64,
    pub generated_at: String,
}

/// Replay-stable packing-slip generation result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratePackingSlipResponse {
    pub document: ShipmentDocumentResponse,
}

/// Replay-stable carton-label-set generation result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateCartonLabelSetResponse {
    pub document: ShipmentDocumentResponse,
}

/// All retained shipment documents, ordered by generation time and identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShipmentDocumentListResponse {
    pub documents: Vec<ShipmentDocumentResponse>,
}

/// One carrier tracking assignment persisted for a shipment carton.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShipmentCartonTrackingResponse {
    pub tracking_assignment_id: i64,
    pub carton_id: i64,
    pub tracking_number: String,
}

/// Immutable manual carrier manifest read model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualCarrierManifestResponse {
    pub manifest_id: i64,
    pub carrier_code: String,
    pub service_code: Option<String>,
    pub manifest_reference: String,
    pub carton_tracking_assignments: Vec<ShipmentCartonTrackingResponse>,
    pub manifested_by: i64,
    pub manifested_at: String,
}

/// One snapshotted closed carton belonging to a shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShipmentCartonResponse {
    pub carton_id: i64,
    pub carton_barcode: String,
    pub sequence: i64,
    pub content_count: i64,
    pub packed_quantity: i64,
    pub weight_grams: Option<i64>,
    pub length_mm: Option<i64>,
    pub width_mm: Option<i64>,
    pub height_mm: Option<i64>,
    pub tracking_assignment_id: Option<i64>,
    pub tracking_number: Option<String>,
    pub departed_at: Option<String>,
}

/// Cumulative carton and quantity progress for physical departure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShipmentDepartureProgressResponse {
    pub total_carton_count: i64,
    pub departed_carton_count: i64,
    pub remaining_carton_count: i64,
    pub total_quantity: i64,
    pub departed_quantity: i64,
    pub remaining_quantity: i64,
}

/// Ordered, physically shipped, and accepted-short quantities for one shipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShipmentDemandResponse {
    pub ordered_quantity: i64,
    pub shipped_quantity: i64,
    pub accepted_short_quantity: i64,
    pub accepted_substitute_quantity: i64,
}

/// Complete shipment read model used for create, resume, and operator display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShipmentResponse {
    pub shipment_id: i64,
    pub packing_session_id: i64,
    pub order_id: i64,
    pub order_key: String,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub status: ShipmentStatus,
    pub revision: Revision,
    pub order_status: ShipmentOrderStatus,
    pub order_revision: Revision,
    pub demand: ShipmentDemandResponse,
    pub departure_progress: ShipmentDepartureProgressResponse,
    pub cartons: Vec<ShipmentCartonResponse>,
    pub manifest: Option<ManualCarrierManifestResponse>,
    pub created_by: i64,
    pub created_at: String,
    pub departed_by: Option<i64>,
    pub departed_at: Option<String>,
}

/// Replay-stable result of creating one shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateShipmentResponse {
    pub shipment: ShipmentResponse,
    pub order_status: ShipmentOrderStatus,
    pub order_revision: Revision,
}

/// Replay-stable result of manually manifesting one shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordManualManifestResponse {
    pub shipment_id: i64,
    pub order_id: i64,
    pub status: ShipmentStatus,
    pub revision: Revision,
    pub manifest: ManualCarrierManifestResponse,
}

/// Replay-stable result of confirming shipment and order departure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmShipmentDepartureResponse {
    pub shipment_id: i64,
    pub order_id: i64,
    pub shipment_status: ShipmentStatus,
    pub shipment_revision: Revision,
    pub order_status: ShipmentOrderStatus,
    pub order_revision: Revision,
    pub scanned_carton_count: i64,
    pub departure_quantity: i64,
    pub cumulative_departed_quantity: i64,
    pub remaining_quantity: i64,
    pub remaining_carton_count: i64,
    pub demand: ShipmentDemandResponse,
    pub departed_by: i64,
    pub departed_at: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn shipping_mutation_requests_are_strict_revisioned_and_unscoped() {
        let create = serde_json::from_value::<CreateShipmentRequest>(json!({
            "packing_session_id": 12,
            "expected_revision": 8
        }))
        .unwrap();
        assert_eq!(create.packing_session_id, 12);
        assert_eq!(create.expected_revision.get(), 8);

        assert!(serde_json::from_value::<CreateShipmentRequest>(json!({
            "packing_session_id": 12,
            "expected_revision": 8,
            "order_id": 7
        }))
        .is_err());
        assert!(
            serde_json::from_value::<RecordManualManifestRequest>(json!({
                "carrier_code": "UPS",
                "manifest_reference": "M-1",
                "carton_tracking_assignments": [],
                "expected_revision": 0
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ConfirmShipmentDepartureRequest>(json!({
                "scanned_carton_barcodes": ["CARTON-1"],
                "expected_shipment_revision": 2,
                "expected_order_revision": 8,
                "force": true
            }))
            .is_err()
        );
        assert!(serde_json::from_value::<GeneratePackingSlipRequest>(json!({
            "expected_shipment_revision": 2,
            "shipment_id": 7
        }))
        .is_err());
        assert_eq!(
            serde_json::from_value::<GeneratePackingSlipRequest>(json!({
                "expected_shipment_revision": 2
            }))
            .unwrap()
            .expected_shipment_revision
            .get(),
            2
        );
        assert!(
            serde_json::from_value::<GenerateCartonLabelSetRequest>(json!({
                "expected_shipment_revision": 2,
                "format": "zpl"
            }))
            .is_err()
        );
    }

    #[test]
    fn manual_manifest_request_preserves_exact_carton_assignments() {
        let request = serde_json::from_value::<RecordManualManifestRequest>(json!({
            "carrier_code": "UPS",
            "service_code": "GROUND",
            "manifest_reference": "M-42",
            "carton_tracking_assignments": [
                {"carton_id": 8, "tracking_number": "TRACK-8"},
                {"carton_id": 9, "tracking_number": "TRACK-9"}
            ],
            "expected_revision": 1
        }))
        .unwrap();

        assert_eq!(request.carton_tracking_assignments.len(), 2);
        assert_eq!(request.carton_tracking_assignments[1].carton_id, 9);
        assert_eq!(request.expected_revision.get(), 1);
    }

    #[test]
    fn departed_response_separates_shipment_and_order_revisions() {
        let response = ConfirmShipmentDepartureResponse {
            shipment_id: 1,
            order_id: 2,
            shipment_status: ShipmentStatus::Departed,
            shipment_revision: Revision::new(3).unwrap(),
            order_status: ShipmentOrderStatus::Shipped,
            order_revision: Revision::new(10).unwrap(),
            scanned_carton_count: 2,
            departure_quantity: 5,
            cumulative_departed_quantity: 5,
            remaining_quantity: 0,
            remaining_carton_count: 0,
            demand: ShipmentDemandResponse {
                ordered_quantity: 7,
                shipped_quantity: 5,
                accepted_short_quantity: 2,
                accepted_substitute_quantity: 0,
            },
            departed_by: 4,
            departed_at: "2026-08-08T22:00:00Z".into(),
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "shipment_id": 1,
                "order_id": 2,
                "shipment_status": "departed",
                "shipment_revision": 3,
                "order_status": "shipped",
                "order_revision": 10,
                "scanned_carton_count": 2,
                "departure_quantity": 5,
                "cumulative_departed_quantity": 5,
                "remaining_quantity": 0,
                "remaining_carton_count": 0,
                "demand": {
                    "ordered_quantity": 7,
                    "shipped_quantity": 5,
                    "accepted_short_quantity": 2,
                    "accepted_substitute_quantity": 0
                },
                "departed_by": 4,
                "departed_at": "2026-08-08T22:00:00Z"
            })
        );
    }
}
