use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::{ConfigurationScope, Revision};

pub const PRODUCT_DEFAULT_DOCUMENT_POLICY_HASH: &str =
    "8fa715da98b8dc84175d61bdadddfd29318e7dfe43e36568bb052d0583c1df24";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentPolicySource {
    ProductDefault,
    Configuration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocumentPolicyExpectation {
    pub source: DocumentPolicySource,
    pub configuration_id: Option<i64>,
    pub configuration_revision: Option<i64>,
    pub policy_hash: String,
}

impl DocumentPolicyExpectation {
    pub fn product_default() -> Self {
        Self {
            source: DocumentPolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            policy_hash: PRODUCT_DEFAULT_DOCUMENT_POLICY_HASH.to_owned(),
        }
    }

    fn validate(&self) -> Result<(), &'static str> {
        let identity_is_valid = match self.source {
            DocumentPolicySource::ProductDefault => {
                self.configuration_id.is_none() && self.configuration_revision.is_none()
            }
            DocumentPolicySource::Configuration => {
                self.configuration_id.is_some_and(|id| id > 0)
                    && self
                        .configuration_revision
                        .is_some_and(|revision| revision > 0)
            }
        };
        if !identity_is_valid {
            return Err("document policy identity is invalid");
        }
        if self.policy_hash.len() != 64
            || !self
                .policy_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("document policy hash must be lowercase SHA-256 hex");
        }
        Ok(())
    }
}

impl Default for DocumentPolicyExpectation {
    fn default() -> Self {
        Self::product_default()
    }
}

impl<'de> Deserialize<'de> for DocumentPolicyExpectation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            source: DocumentPolicySource,
            configuration_id: Option<i64>,
            configuration_revision: Option<i64>,
            policy_hash: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        let value = Self {
            source: raw.source,
            configuration_id: raw.configuration_id,
            configuration_revision: raw.configuration_revision,
            policy_hash: raw.policy_hash,
        };
        value.validate().map_err(D::Error::custom)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentPolicyResponse {
    pub source: DocumentPolicySource,
    pub configuration_id: Option<i64>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub generate_packing_slip: bool,
    pub generate_carton_label: bool,
    pub require_tracking_barcode: bool,
    pub policy_hash: String,
}

impl DocumentPolicyResponse {
    pub fn expectation(&self) -> DocumentPolicyExpectation {
        DocumentPolicyExpectation {
            source: self.source,
            configuration_id: self.configuration_id,
            configuration_revision: self.configuration_revision,
            policy_hash: self.policy_hash.clone(),
        }
    }
}

/// Public lifecycle of one parcel shipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShipmentStatus {
    AwaitingManifest,
    Manifested,
    PartiallyDeparted,
    Departed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShipmentCancellationReason {
    PackingCorrection,
    ShippingDataCorrection,
    DuplicateShipment,
    OperatorError,
    Other,
}

/// Order state exposed by the parcel-shipment workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShipmentOrderStatus {
    Packing,
    AwaitingShipment,
    Shipped,
    Cancelled,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelShipmentRequest {
    pub expected_shipment_revision: Revision,
    pub expected_order_revision: Revision,
    pub reason: ShipmentCancellationReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl<'de> Deserialize<'de> for CancelShipmentRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            expected_shipment_revision: Revision,
            expected_order_revision: Revision,
            reason: ShipmentCancellationReason,
            #[serde(default)]
            note: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.note.as_ref().is_some_and(|note| {
            note.is_empty()
                || note.trim() != note
                || note.chars().count() > 500
                || note.chars().any(char::is_control)
        }) {
            return Err(D::Error::custom("shipment cancellation note is invalid"));
        }
        if raw.reason == ShipmentCancellationReason::Other && raw.note.is_none() {
            return Err(D::Error::custom(
                "shipment cancellation reason Other requires a note",
            ));
        }
        Ok(Self {
            expected_shipment_revision: raw.expected_shipment_revision,
            expected_order_revision: raw.expected_order_revision,
            reason: raw.reason,
            note: raw.note,
        })
    }
}

/// Generates the immutable packing slip for one shipment revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratePackingSlipRequest {
    pub expected_shipment_revision: Revision,
    #[serde(default)]
    pub expected_policy: DocumentPolicyExpectation,
}

/// Generates the immutable carton-label set for one manifested shipment revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateCartonLabelSetRequest {
    pub expected_shipment_revision: Revision,
    #[serde(default)]
    pub expected_policy: DocumentPolicyExpectation,
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
    pub policy: DocumentPolicyResponse,
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
    pub policy: DocumentPolicyResponse,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShipmentCancellationResponse {
    pub cancellation_id: i64,
    pub previous_status: ShipmentStatus,
    pub reason: ShipmentCancellationReason,
    pub note: Option<String>,
    pub cancelled_by: i64,
    pub cancelled_at: String,
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
    pub attempt: i64,
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
    pub cancellation: Option<ShipmentCancellationResponse>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelShipmentResponse {
    pub shipment: ShipmentResponse,
    pub packing_session_revision: Revision,
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
        let cancel = serde_json::from_value::<CancelShipmentRequest>(json!({
            "expected_shipment_revision": 1,
            "expected_order_revision": 8,
            "reason": "packing_correction",
            "note": "Reclose carton"
        }))
        .unwrap();
        assert_eq!(cancel.reason, ShipmentCancellationReason::PackingCorrection);
        assert_eq!(cancel.expected_order_revision.get(), 8);
        assert!(serde_json::from_value::<CancelShipmentRequest>(json!({
            "expected_shipment_revision": 1,
            "expected_order_revision": 8,
            "reason": "other"
        }))
        .is_err());
        assert!(serde_json::from_value::<CancelShipmentRequest>(json!({
            "expected_shipment_revision": 1,
            "expected_order_revision": 8,
            "reason": "operator_error",
            "force": true
        }))
        .is_err());
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
        assert!(serde_json::from_value::<DocumentPolicyExpectation>(json!({
            "source": "configuration",
            "configuration_id": null,
            "configuration_revision": 4,
            "policy_hash": "a".repeat(64)
        }))
        .is_err());
        assert!(serde_json::from_value::<DocumentPolicyExpectation>(json!({
            "source": "product_default",
            "configuration_id": null,
            "configuration_revision": null,
            "policy_hash": "A".repeat(64)
        }))
        .is_err());
        assert_eq!(
            serde_json::from_value::<GeneratePackingSlipRequest>(json!({
                "expected_shipment_revision": 2
            }))
            .unwrap()
            .expected_policy,
            DocumentPolicyExpectation::product_default()
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
