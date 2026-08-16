//! Application contracts for parcel shipment execution.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wareboxes_domain::{
    CarrierCode, CarrierManifestId, CarrierServiceCode, CartonId, CartonTrackingAssignment,
    ConfigurationScope, ConfigurationVersionId, FacilityId, InventoryOwnerId, ManifestReference,
    OrderId, OrderLineId, OrderRevision, OrderStatus, PackSessionId, ShipmentCancellationDetails,
    ShipmentCancellationId, ShipmentDocumentId, ShipmentDocumentType, ShipmentId, ShipmentRevision,
    ShipmentScanValue, ShipmentStatus, ShipmentTrackingAssignmentId, ShortShipDemandQuantities,
    Timestamp, TrackingNumber, UserId,
};

pub const CREATE_SHIPMENT_OPERATION: &str = "shipping.shipment.create.v1";
pub const RECORD_MANUAL_MANIFEST_OPERATION: &str = "shipping.manifest.manual.record.v1";
pub const CONFIRM_SHIPMENT_DEPARTURE_OPERATION: &str = "shipping.shipment.departure.confirm.v1";
pub const CANCEL_SHIPMENT_OPERATION: &str = "shipping.shipment.cancel.v1";
pub const GENERATE_PACKING_SLIP_OPERATION: &str = "shipping.document.packing_slip.generate.v1";
pub const GENERATE_CARTON_LABEL_SET_OPERATION: &str =
    "shipping.document.carton_label_set.generate.v1";
pub const CANCEL_SHIPMENT_DOCUMENT_PRINT_OPERATION: &str = "shipping.document.print.cancel.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentPolicySource {
    ProductDefault,
    Configuration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPolicyExpectation {
    pub source: DocumentPolicySource,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub policy_hash: String,
}

impl DocumentPolicyExpectation {
    pub fn is_well_formed(&self) -> bool {
        let identity_is_valid = match self.source {
            DocumentPolicySource::ProductDefault => {
                self.configuration_id.is_none() && self.configuration_revision.is_none()
            }
            DocumentPolicySource::Configuration => {
                self.configuration_id.is_some()
                    && self
                        .configuration_revision
                        .is_some_and(|revision| revision > 0)
            }
        };
        identity_is_valid
            && self.policy_hash.len() == 64
            && self
                .policy_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPolicyReadModel {
    pub source: DocumentPolicySource,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub generate_packing_slip: bool,
    pub generate_carton_label: bool,
    pub require_tracking_barcode: bool,
    pub policy_hash: String,
}

impl DocumentPolicyReadModel {
    pub fn product_default() -> Self {
        let generate_packing_slip = true;
        let generate_carton_label = true;
        let require_tracking_barcode = false;
        Self {
            source: DocumentPolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            configuration_scope: None,
            generate_packing_slip,
            generate_carton_label,
            require_tracking_barcode,
            policy_hash: document_policy_hash(
                generate_packing_slip,
                generate_carton_label,
                require_tracking_barcode,
            ),
        }
    }

    pub fn expectation(&self) -> DocumentPolicyExpectation {
        DocumentPolicyExpectation {
            source: self.source,
            configuration_id: self.configuration_id,
            configuration_revision: self.configuration_revision,
            policy_hash: self.policy_hash.clone(),
        }
    }

    pub fn matches_expectation(&self, expected: &DocumentPolicyExpectation) -> bool {
        expected.is_well_formed() && self.expectation() == *expected
    }

    pub const fn permits(&self, document_type: ShipmentDocumentType) -> bool {
        match document_type {
            ShipmentDocumentType::PackingSlip => self.generate_packing_slip,
            ShipmentDocumentType::CartonLabelSet => self.generate_carton_label,
        }
    }
}

pub fn document_policy_hash(
    generate_packing_slip: bool,
    generate_carton_label: bool,
    require_tracking_barcode: bool,
) -> String {
    let canonical = format!(
        "document-policy-v1|{generate_packing_slip}|{generate_carton_label}|{require_tracking_barcode}"
    );
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

/// Generates one immutable packing slip at the observed shipment revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratePackingSlipCommand {
    pub shipment_id: ShipmentId,
    pub expected_revision: ShipmentRevision,
    pub expected_policy: DocumentPolicyExpectation,
}

/// Generates one immutable printable label set from a manifested shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerateCartonLabelSetCommand {
    pub shipment_id: ShipmentId,
    pub expected_revision: ShipmentRevision,
    pub expected_policy: DocumentPolicyExpectation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelShipmentDocumentPrintCommand {
    pub document_id: ShipmentDocumentId,
    pub command_id: wareboxes_domain::AutomationCommandId,
    pub expected_revision: u32,
}

/// Lists immutable documents belonging to one visible shipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipmentDocumentListQuery {
    pub shipment_id: ShipmentId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentDocumentListReadModel {
    pub policy: DocumentPolicyReadModel,
    pub documents: Vec<ShipmentDocumentReadModel>,
}

/// Reads one immutable document and its retained content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipmentDocumentContentQuery {
    pub document_id: ShipmentDocumentId,
}

/// One snapshotted order line represented by a shipment document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentDocumentLineReadModel {
    pub sequence: i64,
    pub order_line_id: OrderLineId,
    pub line_key: String,
    pub item_id: wareboxes_domain::CatalogItemId,
    pub item_description: String,
    pub uom: String,
    pub ordered_quantity: i64,
    pub accepted_short_quantity: i64,
    pub packed_quantity: i64,
}

/// Immutable metadata for one retained shipment document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentDocumentReadModel {
    pub document_id: ShipmentDocumentId,
    pub shipment_id: ShipmentId,
    pub order_id: OrderId,
    pub document_type: ShipmentDocumentType,
    pub manifest_id: Option<CarrierManifestId>,
    pub carrier_code: Option<CarrierCode>,
    pub service_code: Option<CarrierServiceCode>,
    pub manifest_reference: Option<ManifestReference>,
    pub file_name: String,
    pub media_type: String,
    pub content_length: i64,
    pub content_sha256: String,
    pub shipment_revision_at_generation: ShipmentRevision,
    pub carton_count: i64,
    pub line_count: i64,
    pub demand: ShortShipDemandQuantities,
    pub policy: DocumentPolicyReadModel,
    pub generated_by: UserId,
    pub generated_at: Timestamp,
}

/// Authenticated download payload for one immutable shipment document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShipmentDocumentContentReadModel {
    pub document: ShipmentDocumentReadModel,
    pub content: String,
}

/// Replay-stable packing-slip generation result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratePackingSlipResult {
    pub document: ShipmentDocumentReadModel,
}

/// Replay-stable carton-label-set generation result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerateCartonLabelSetResult {
    pub document: ShipmentDocumentReadModel,
}

/// Creates one shipment from a complete ready packing session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateShipmentCommand {
    pub order_id: OrderId,
    pub packing_session_id: PackSessionId,
    pub expected_revision: OrderRevision,
}

/// Reads a resumable shipment by its durable path-derived identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipmentQuery {
    pub shipment_id: ShipmentId,
}

/// Carrier tracking assigned to one immutable shipment carton.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentCartonTrackingReadModel {
    pub tracking_assignment_id: ShipmentTrackingAssignmentId,
    pub carton_id: CartonId,
    pub tracking_number: TrackingNumber,
}

/// Immutable manual carrier manifest captured for one shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualCarrierManifestReadModel {
    pub manifest_id: CarrierManifestId,
    pub carrier_code: CarrierCode,
    pub service_code: Option<CarrierServiceCode>,
    pub manifest_reference: ManifestReference,
    pub carton_tracking_assignments: Vec<ShipmentCartonTrackingReadModel>,
    pub manifested_by: UserId,
    pub manifested_at: Timestamp,
}

/// One snapshotted closed carton belonging to a shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentCartonReadModel {
    pub carton_id: CartonId,
    pub carton_barcode: ShipmentScanValue,
    pub sequence: i64,
    pub content_count: i64,
    pub packed_quantity: i64,
    pub weight_grams: Option<i64>,
    pub length_mm: Option<i64>,
    pub width_mm: Option<i64>,
    pub height_mm: Option<i64>,
    pub tracking_assignment_id: Option<ShipmentTrackingAssignmentId>,
    pub tracking_number: Option<TrackingNumber>,
    pub departed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentCancellationReadModel {
    pub cancellation_id: ShipmentCancellationId,
    pub previous_status: ShipmentStatus,
    pub details: ShipmentCancellationDetails,
    pub cancelled_by: UserId,
    pub cancelled_at: Timestamp,
}

/// Cumulative physical departure progress for one shipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentDepartureProgress {
    pub total_carton_count: i64,
    pub departed_carton_count: i64,
    pub remaining_carton_count: i64,
    pub total_quantity: i64,
    pub departed_quantity: i64,
    pub remaining_quantity: i64,
}

impl ShipmentDepartureProgress {
    pub const fn is_consistent(self) -> bool {
        self.total_carton_count > 0
            && self.departed_carton_count >= 0
            && self.remaining_carton_count >= 0
            && self.departed_carton_count + self.remaining_carton_count == self.total_carton_count
            && self.total_quantity > 0
            && self.departed_quantity >= 0
            && self.remaining_quantity >= 0
            && self.departed_quantity + self.remaining_quantity == self.total_quantity
    }
}

/// Resumable shipment state returned after create and read operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentReadModel {
    pub shipment_id: ShipmentId,
    pub attempt: i64,
    pub packing_session_id: PackSessionId,
    pub order_id: OrderId,
    pub order_key: String,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub status: ShipmentStatus,
    pub revision: ShipmentRevision,
    pub order_status: OrderStatus,
    pub order_revision: OrderRevision,
    pub demand: ShortShipDemandQuantities,
    pub departure_progress: ShipmentDepartureProgress,
    pub cartons: Vec<ShipmentCartonReadModel>,
    pub manifest: Option<ManualCarrierManifestReadModel>,
    pub cancellation: Option<ShipmentCancellationReadModel>,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub departed_by: Option<UserId>,
    pub departed_at: Option<Timestamp>,
}

impl ShipmentReadModel {
    /// Checks state-dependent facts that repository read models must preserve.
    pub fn is_consistent(&self) -> bool {
        if self.cartons.is_empty() {
            return false;
        }
        let carton_quantity = self.cartons.iter().try_fold(0_i64, |total, carton| {
            total.checked_add(carton.packed_quantity)
        });
        if carton_quantity != Some(self.demand.effective().get())
            || self.demand.effective().is_zero()
            || !self.departure_progress.is_consistent()
            || self.departure_progress.total_carton_count
                != i64::try_from(self.cartons.len()).unwrap_or(i64::MAX)
            || self.departure_progress.total_quantity != self.demand.effective().get()
            || self.departure_progress.departed_carton_count
                != i64::try_from(
                    self.cartons
                        .iter()
                        .filter(|carton| carton.departed_at.is_some())
                        .count(),
                )
                .unwrap_or(i64::MAX)
            || Some(self.departure_progress.departed_quantity)
                != self
                    .cartons
                    .iter()
                    .filter(|carton| carton.departed_at.is_some())
                    .try_fold(0_i64, |total, carton| {
                        total.checked_add(carton.packed_quantity)
                    })
        {
            return false;
        }
        match self.status {
            ShipmentStatus::AwaitingManifest => {
                matches!(self.order_status, OrderStatus::AwaitingShipment)
                    && self.manifest.is_none()
                    && self.cancellation.is_none()
                    && self.departed_by.is_none()
                    && self.departed_at.is_none()
                    && self.departure_progress.departed_carton_count == 0
                    && self.cartons.iter().all(|carton| {
                        carton.tracking_assignment_id.is_none() && carton.tracking_number.is_none()
                    })
            }
            ShipmentStatus::Manifested => {
                matches!(self.order_status, OrderStatus::AwaitingShipment)
                    && self.manifest_covers_cartons()
                    && self.cancellation.is_none()
                    && self.departed_by.is_none()
                    && self.departed_at.is_none()
                    && self.departure_progress.departed_carton_count == 0
            }
            ShipmentStatus::PartiallyDeparted => {
                matches!(self.order_status, OrderStatus::AwaitingShipment)
                    && self.manifest_covers_cartons()
                    && self.cancellation.is_none()
                    && self.departed_by.is_none()
                    && self.departed_at.is_none()
                    && self.departure_progress.departed_carton_count > 0
                    && self.departure_progress.remaining_carton_count > 0
            }
            ShipmentStatus::Departed => {
                matches!(self.order_status, OrderStatus::Shipped)
                    && self.manifest_covers_cartons()
                    && self.cancellation.is_none()
                    && self.departed_by.is_some()
                    && self.departed_at.is_some()
                    && self.departure_progress.remaining_carton_count == 0
            }
            ShipmentStatus::Cancelled => {
                self.cancellation.as_ref().is_some_and(|cancellation| {
                    match cancellation.previous_status {
                        ShipmentStatus::AwaitingManifest => {
                            self.revision.get() == 2
                                && self.manifest.is_none()
                                && self.cartons.iter().all(|carton| {
                                    carton.tracking_assignment_id.is_none()
                                        && carton.tracking_number.is_none()
                                })
                        }
                        ShipmentStatus::Manifested => {
                            self.revision.get() == 3 && self.manifest_covers_cartons()
                        }
                        _ => false,
                    }
                }) && self.departed_by.is_none()
                    && self.departed_at.is_none()
                    && self.departure_progress.departed_carton_count == 0
            }
        }
    }

    fn manifest_covers_cartons(&self) -> bool {
        let Some(manifest) = self.manifest.as_ref() else {
            return false;
        };
        manifest.carton_tracking_assignments.len() == self.cartons.len()
            && self.cartons.iter().all(|carton| {
                let (Some(assignment_id), Some(tracking_number)) = (
                    carton.tracking_assignment_id,
                    carton.tracking_number.as_ref(),
                ) else {
                    return false;
                };
                manifest
                    .carton_tracking_assignments
                    .iter()
                    .any(|assignment| {
                        assignment.tracking_assignment_id == assignment_id
                            && assignment.carton_id == carton.carton_id
                            && &assignment.tracking_number == tracking_number
                    })
            })
    }
}

/// Replay-stable shipment creation result, including the advanced order revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateShipmentResult {
    pub shipment: ShipmentReadModel,
    pub order_status: OrderStatus,
    pub order_revision: OrderRevision,
}

/// Records one manual carrier manifest and one tracking assignment per carton.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordManualManifestCommand {
    pub shipment_id: ShipmentId,
    pub carrier_code: CarrierCode,
    pub service_code: Option<CarrierServiceCode>,
    pub manifest_reference: ManifestReference,
    pub carton_tracking_assignments: Vec<CartonTrackingAssignment>,
    pub expected_revision: ShipmentRevision,
}

/// Replay-stable result of manifesting one shipment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordManualManifestResult {
    pub shipment_id: ShipmentId,
    pub order_id: OrderId,
    pub status: ShipmentStatus,
    pub revision: ShipmentRevision,
    pub manifest: ManualCarrierManifestReadModel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelShipmentCommand {
    pub shipment_id: ShipmentId,
    pub expected_shipment_revision: ShipmentRevision,
    pub expected_order_revision: OrderRevision,
    pub details: ShipmentCancellationDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelShipmentResult {
    pub shipment: ShipmentReadModel,
    pub packing_session_revision: OrderRevision,
}

/// Confirms physical departure for a nonempty subset of remaining shipment cartons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmShipmentDepartureCommand {
    pub shipment_id: ShipmentId,
    pub scanned_carton_barcodes: Vec<ShipmentScanValue>,
    pub expected_shipment_revision: ShipmentRevision,
    pub expected_order_revision: OrderRevision,
}

/// Replay-stable departure result for both shipment and order aggregates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmShipmentDepartureResult {
    pub shipment_id: ShipmentId,
    pub order_id: OrderId,
    pub shipment_status: ShipmentStatus,
    pub shipment_revision: ShipmentRevision,
    pub order_status: OrderStatus,
    pub order_revision: OrderRevision,
    pub scanned_carton_count: i64,
    pub departure_quantity: i64,
    pub cumulative_departed_quantity: i64,
    pub remaining_quantity: i64,
    pub remaining_carton_count: i64,
    pub demand: ShortShipDemandQuantities,
    pub departed_by: UserId,
    pub departed_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_domain::ShipmentCancellationReason;

    fn manifested_shipment(status: ShipmentStatus) -> ShipmentReadModel {
        let tracking_assignment_id = ShipmentTrackingAssignmentId::new(7).unwrap();
        let carton_id = CartonId::new(8).unwrap();
        let tracking_number = TrackingNumber::new("TRACK-8").unwrap();
        ShipmentReadModel {
            shipment_id: ShipmentId::new(1).unwrap(),
            attempt: 1,
            packing_session_id: PackSessionId::new(2).unwrap(),
            order_id: OrderId::new(3).unwrap(),
            order_key: "SO-3".into(),
            inventory_owner_id: InventoryOwnerId::new(4).unwrap(),
            facility_id: FacilityId::new(5).unwrap(),
            status,
            revision: ShipmentRevision::new(2).unwrap(),
            order_status: OrderStatus::AwaitingShipment,
            order_revision: OrderRevision::new(8).unwrap(),
            demand: ShortShipDemandQuantities::new(
                wareboxes_domain::PickQuantity::new(2).unwrap(),
                wareboxes_domain::ActualPickQuantity::ZERO,
            )
            .unwrap(),
            departure_progress: ShipmentDepartureProgress {
                total_carton_count: 1,
                departed_carton_count: 0,
                remaining_carton_count: 1,
                total_quantity: 2,
                departed_quantity: 0,
                remaining_quantity: 2,
            },
            cartons: vec![ShipmentCartonReadModel {
                carton_id,
                carton_barcode: ShipmentScanValue::new("CARTON-8").unwrap(),
                sequence: 1,
                content_count: 1,
                packed_quantity: 2,
                weight_grams: Some(1200),
                length_mm: Some(300),
                width_mm: Some(200),
                height_mm: Some(150),
                tracking_assignment_id: Some(tracking_assignment_id),
                tracking_number: Some(tracking_number.clone()),
                departed_at: None,
            }],
            manifest: Some(ManualCarrierManifestReadModel {
                manifest_id: CarrierManifestId::new(6).unwrap(),
                carrier_code: CarrierCode::new("UPS").unwrap(),
                service_code: Some(CarrierServiceCode::new("GROUND").unwrap()),
                manifest_reference: ManifestReference::new("MANIFEST-6").unwrap(),
                carton_tracking_assignments: vec![ShipmentCartonTrackingReadModel {
                    tracking_assignment_id,
                    carton_id,
                    tracking_number,
                }],
                manifested_by: UserId::new(9).unwrap(),
                manifested_at: "2026-08-08T20:00:00Z".parse().unwrap(),
            }),
            cancellation: None,
            created_by: UserId::new(9).unwrap(),
            created_at: "2026-08-08T19:00:00Z".parse().unwrap(),
            departed_by: None,
            departed_at: None,
        }
    }

    #[test]
    fn every_shipping_mutation_carries_an_optimistic_revision() {
        let order_revision = OrderRevision::new(3).unwrap();
        let shipment_revision = ShipmentRevision::new(4).unwrap();
        assert_eq!(
            CreateShipmentCommand {
                order_id: OrderId::new(2).unwrap(),
                packing_session_id: PackSessionId::new(1).unwrap(),
                expected_revision: order_revision,
            }
            .expected_revision,
            order_revision
        );
        assert_eq!(
            RecordManualManifestCommand {
                shipment_id: ShipmentId::new(2).unwrap(),
                carrier_code: CarrierCode::new("UPS").unwrap(),
                service_code: None,
                manifest_reference: ManifestReference::new("M-1").unwrap(),
                carton_tracking_assignments: vec![CartonTrackingAssignment::new(
                    CartonId::new(5).unwrap(),
                    TrackingNumber::new("T-5").unwrap(),
                )],
                expected_revision: shipment_revision,
            }
            .expected_revision,
            shipment_revision
        );
        assert_eq!(
            ConfirmShipmentDepartureCommand {
                shipment_id: ShipmentId::new(2).unwrap(),
                scanned_carton_barcodes: vec![ShipmentScanValue::new("C-5").unwrap()],
                expected_shipment_revision: shipment_revision,
                expected_order_revision: order_revision,
            }
            .expected_shipment_revision,
            shipment_revision
        );
        assert_eq!(
            ConfirmShipmentDepartureCommand {
                shipment_id: ShipmentId::new(2).unwrap(),
                scanned_carton_barcodes: vec![ShipmentScanValue::new("C-5").unwrap()],
                expected_shipment_revision: shipment_revision,
                expected_order_revision: order_revision,
            }
            .expected_order_revision,
            order_revision
        );
        let cancellation = CancelShipmentCommand {
            shipment_id: ShipmentId::new(2).unwrap(),
            expected_shipment_revision: shipment_revision,
            expected_order_revision: order_revision,
            details: ShipmentCancellationDetails::new(
                ShipmentCancellationReason::PackingCorrection,
                None,
            )
            .unwrap(),
        };
        assert_eq!(cancellation.expected_shipment_revision, shipment_revision);
        assert_eq!(cancellation.expected_order_revision, order_revision);
    }

    #[test]
    fn document_policy_default_and_hash_are_stable() {
        let policy = DocumentPolicyReadModel::product_default();
        assert_eq!(policy.source, DocumentPolicySource::ProductDefault);
        assert!(policy.generate_packing_slip);
        assert!(policy.generate_carton_label);
        assert!(!policy.require_tracking_barcode);
        assert_eq!(
            policy.policy_hash,
            "8fa715da98b8dc84175d61bdadddfd29318e7dfe43e36568bb052d0583c1df24"
        );
        assert!(policy.matches_expectation(&policy.expectation()));

        let mut stale = policy.expectation();
        stale.policy_hash = "0".repeat(64);
        assert!(!policy.matches_expectation(&stale));
    }

    #[test]
    fn shipment_read_model_requires_manifest_and_departure_facts_by_state() {
        let mut manifested = manifested_shipment(ShipmentStatus::Manifested);
        let mut second_carton = manifested.cartons[0].clone();
        second_carton.carton_id = CartonId::new(18).unwrap();
        second_carton.carton_barcode = ShipmentScanValue::new("CARTON-18").unwrap();
        second_carton.sequence = 2;
        second_carton.tracking_assignment_id = Some(ShipmentTrackingAssignmentId::new(17).unwrap());
        second_carton.tracking_number = Some(TrackingNumber::new("TRACK-18").unwrap());
        manifested.cartons.push(second_carton);
        manifested
            .manifest
            .as_mut()
            .unwrap()
            .carton_tracking_assignments
            .push(ShipmentCartonTrackingReadModel {
                tracking_assignment_id: ShipmentTrackingAssignmentId::new(17).unwrap(),
                carton_id: CartonId::new(18).unwrap(),
                tracking_number: TrackingNumber::new("TRACK-18").unwrap(),
            });
        manifested.demand = ShortShipDemandQuantities::new(
            wareboxes_domain::PickQuantity::new(4).unwrap(),
            wareboxes_domain::ActualPickQuantity::ZERO,
        )
        .unwrap();
        manifested.departure_progress.total_carton_count = 2;
        manifested.departure_progress.remaining_carton_count = 2;
        manifested.departure_progress.total_quantity = 4;
        manifested.departure_progress.remaining_quantity = 4;
        assert!(manifested.is_consistent());

        let mut partial = manifested;
        partial.status = ShipmentStatus::PartiallyDeparted;
        partial.revision = ShipmentRevision::new(3).unwrap();
        partial.order_revision = OrderRevision::new(9).unwrap();
        partial.departure_progress.departed_carton_count = 1;
        partial.departure_progress.remaining_carton_count = 1;
        partial.departure_progress.departed_quantity = 2;
        partial.departure_progress.remaining_quantity = 2;
        partial.cartons[0].departed_at = Some("2026-08-08T20:30:00Z".parse().unwrap());
        assert!(partial.is_consistent());

        let mut departed = partial;
        departed.status = ShipmentStatus::Departed;
        departed.order_status = OrderStatus::Shipped;
        departed.departed_by = Some(UserId::new(10).unwrap());
        departed.departed_at = Some("2026-08-08T21:00:00Z".parse().unwrap());
        departed.departure_progress.departed_carton_count = 2;
        departed.departure_progress.remaining_carton_count = 0;
        departed.departure_progress.departed_quantity = 4;
        departed.departure_progress.remaining_quantity = 0;
        departed.cartons[1].departed_at = departed.departed_at;
        assert!(departed.is_consistent());

        departed.cartons[0].tracking_number = None;
        assert!(!departed.is_consistent());
    }

    #[test]
    fn cancelled_attempt_requires_cancellation_evidence_but_not_a_live_order_state() {
        let mut cancelled = manifested_shipment(ShipmentStatus::Cancelled);
        cancelled.revision = ShipmentRevision::new(2).unwrap();
        cancelled.order_status = OrderStatus::Packing;
        cancelled.manifest = None;
        for carton in &mut cancelled.cartons {
            carton.tracking_assignment_id = None;
            carton.tracking_number = None;
        }
        cancelled.cancellation = Some(ShipmentCancellationReadModel {
            cancellation_id: ShipmentCancellationId::new(11).unwrap(),
            previous_status: ShipmentStatus::AwaitingManifest,
            details: ShipmentCancellationDetails::new(
                ShipmentCancellationReason::PackingCorrection,
                None,
            )
            .unwrap(),
            cancelled_by: UserId::new(9).unwrap(),
            cancelled_at: "2026-08-08T20:00:00Z".parse().unwrap(),
        });
        assert!(cancelled.is_consistent());
        cancelled.cancellation = None;
        assert!(!cancelled.is_consistent());
    }

    #[test]
    fn cancelled_manifested_attempt_retains_exact_tracking_history() {
        let mut cancelled = manifested_shipment(ShipmentStatus::Cancelled);
        cancelled.revision = ShipmentRevision::new(3).unwrap();
        cancelled.cancellation = Some(ShipmentCancellationReadModel {
            cancellation_id: ShipmentCancellationId::new(12).unwrap(),
            previous_status: ShipmentStatus::Manifested,
            details: ShipmentCancellationDetails::new(
                ShipmentCancellationReason::ShippingDataCorrection,
                None,
            )
            .unwrap(),
            cancelled_by: UserId::new(9).unwrap(),
            cancelled_at: "2026-08-08T20:05:00Z".parse().unwrap(),
        });
        assert!(cancelled.is_consistent());
        cancelled.cartons[0].tracking_number = None;
        assert!(!cancelled.is_consistent());
    }
}
