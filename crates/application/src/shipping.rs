//! Application contracts for parcel shipment execution.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CarrierCode, CarrierManifestId, CarrierServiceCode, CartonId, CartonTrackingAssignment,
    FacilityId, InventoryOwnerId, ManifestReference, OrderId, OrderRevision, OrderStatus,
    PackSessionId, ShipmentId, ShipmentRevision, ShipmentScanValue, ShipmentStatus,
    ShipmentTrackingAssignmentId, ShortShipDemandQuantities, Timestamp, TrackingNumber, UserId,
};

pub const CREATE_SHIPMENT_OPERATION: &str = "shipping.shipment.create.v1";
pub const RECORD_MANUAL_MANIFEST_OPERATION: &str = "shipping.manifest.manual.record.v1";
pub const CONFIRM_SHIPMENT_DEPARTURE_OPERATION: &str = "shipping.shipment.departure.confirm.v1";

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
}

/// Resumable shipment state returned after create and read operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentReadModel {
    pub shipment_id: ShipmentId,
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
    pub cartons: Vec<ShipmentCartonReadModel>,
    pub manifest: Option<ManualCarrierManifestReadModel>,
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
        {
            return false;
        }
        match self.status {
            ShipmentStatus::AwaitingManifest => {
                matches!(self.order_status, OrderStatus::AwaitingShipment)
                    && self.manifest.is_none()
                    && self.departed_by.is_none()
                    && self.departed_at.is_none()
                    && self.cartons.iter().all(|carton| {
                        carton.tracking_assignment_id.is_none() && carton.tracking_number.is_none()
                    })
            }
            ShipmentStatus::Manifested => {
                matches!(self.order_status, OrderStatus::AwaitingShipment)
                    && self.manifest_covers_cartons()
                    && self.departed_by.is_none()
                    && self.departed_at.is_none()
            }
            ShipmentStatus::Departed => {
                matches!(self.order_status, OrderStatus::Shipped)
                    && self.manifest_covers_cartons()
                    && self.departed_by.is_some()
                    && self.departed_at.is_some()
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

/// Confirms physical departure by scanning the exact shipment carton set.
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
    pub demand: ShortShipDemandQuantities,
    pub departed_by: UserId,
    pub departed_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifested_shipment(status: ShipmentStatus) -> ShipmentReadModel {
        let tracking_assignment_id = ShipmentTrackingAssignmentId::new(7).unwrap();
        let carton_id = CartonId::new(8).unwrap();
        let tracking_number = TrackingNumber::new("TRACK-8").unwrap();
        ShipmentReadModel {
            shipment_id: ShipmentId::new(1).unwrap(),
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
    }

    #[test]
    fn shipment_read_model_requires_manifest_and_departure_facts_by_state() {
        let manifested = manifested_shipment(ShipmentStatus::Manifested);
        assert!(manifested.is_consistent());

        let mut departed = manifested;
        departed.status = ShipmentStatus::Departed;
        departed.order_status = OrderStatus::Shipped;
        departed.departed_by = Some(UserId::new(10).unwrap());
        departed.departed_at = Some("2026-08-08T21:00:00Z".parse().unwrap());
        assert!(departed.is_consistent());

        departed.cartons[0].tracking_number = None;
        assert!(!departed.is_consistent());
    }
}
