//! Application contracts for allocation-backed pack-station workflows.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CartonContentId, CartonContentRemovalId, CartonId, CartonMeasurements, FacilityId,
    InventoryAllocationId, InventoryBalanceId, InventoryOwnerId, ItemBatchId, LicensePlateId,
    LocationId, OrderId, OrderLineId, OrderRevision, OrderStatus, PackContentRemovalDetails,
    PackQuantity, PackScanValue, PackSessionId, PackSessionStatus, PackingProgress, Timestamp,
    UserId,
};

pub const OPEN_PACK_SESSION_OPERATION: &str = "packing.session.open.v1";
pub const CREATE_CARTON_OPERATION: &str = "packing.carton.create.v1";
pub const PACK_PICKED_ALLOCATION_OPERATION: &str = "packing.content.confirm.v1";
pub const REMOVE_PACKED_CONTENT_OPERATION: &str = "packing.content.remove.v1";
pub const CLOSE_CARTON_OPERATION: &str = "packing.carton.close.v1";
pub const VOID_CARTON_OPERATION: &str = "packing.carton.void.v1";

/// Starts packing one picked order at a scoped physical station.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenPackSessionCommand {
    pub order_id: OrderId,
    pub facility_id: FacilityId,
    pub station_location_id: LocationId,
    pub expected_revision: OrderRevision,
}

/// Reads a resumable station session by durable identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackSessionQuery {
    pub session_id: PackSessionId,
}

/// Packing state of one full picked allocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PackAllocationDisposition {
    Available,
    Packed {
        content_id: CartonContentId,
        carton_id: CartonId,
        packed_by: UserId,
        packed_at: Timestamp,
    },
}

/// One immutable picked allocation available to or already processed by a station.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackableAllocation {
    pub inventory_allocation_id: InventoryAllocationId,
    pub order_line_id: OrderLineId,
    pub picked_tote_location_id: LocationId,
    pub picked_tote_location_barcode: PackScanValue,
    pub picked_tote_location_name: Option<String>,
    pub picked_tote_license_plate_id: LicensePlateId,
    pub picked_tote_license_plate_barcode: PackScanValue,
    pub inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub source_location_barcode: PackScanValue,
    pub source_location_name: Option<String>,
    pub license_plate_id: LicensePlateId,
    pub license_plate_barcode: PackScanValue,
    pub item_batch_id: ItemBatchId,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub item_barcodes: Vec<PackScanValue>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub quantity: PackQuantity,
    pub disposition: PackAllocationDisposition,
}

/// State-specific facts for one physical carton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PackCartonLifecycle {
    Open,
    Closed {
        measurements: CartonMeasurements,
        closed_by: UserId,
        closed_at: Timestamp,
    },
    Voided {
        voided_by: UserId,
        voided_at: Timestamp,
    },
}

/// One carton in a resumable pack session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackCarton {
    pub carton_id: CartonId,
    pub carton_barcode: PackScanValue,
    pub lifecycle: PackCartonLifecycle,
    pub content_count: i64,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

/// Complete read model needed to resume a station after reconnect or handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSessionReadModel {
    pub session_id: PackSessionId,
    pub order_id: OrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub station_location_id: LocationId,
    pub station_location_barcode: PackScanValue,
    pub station_location_name: Option<String>,
    pub order_key: String,
    pub revision: OrderRevision,
    pub progress: PackingProgress,
    pub cartons: Vec<PackCarton>,
    pub allocations: Vec<PackableAllocation>,
    pub started_by: UserId,
    pub started_at: Timestamp,
}

impl PackSessionReadModel {
    pub const fn status(&self) -> PackSessionStatus {
        self.progress.status()
    }
}

/// Replay-stable result of opening a pack session and advancing the order revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenPackSessionResult {
    pub session: PackSessionReadModel,
}

/// Creates the sole open carton in a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateCartonCommand {
    pub session_id: PackSessionId,
    pub carton_barcode: PackScanValue,
    pub expected_revision: OrderRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCartonResult {
    pub session_id: PackSessionId,
    pub order_id: OrderId,
    pub carton: PackCarton,
    pub revision: OrderRevision,
    pub progress: PackingProgress,
}

/// Packs the full immutable quantity of one picked allocation into an open carton.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackPickedAllocationCommand {
    pub session_id: PackSessionId,
    pub carton_id: CartonId,
    pub inventory_allocation_id: InventoryAllocationId,
    pub item_barcode: PackScanValue,
    pub lot_scan: Option<PackScanValue>,
    pub serial_scan: Option<PackScanValue>,
    pub source_license_plate_barcode: PackScanValue,
    pub carton_barcode: PackScanValue,
    pub expected_revision: OrderRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackPickedAllocationResult {
    pub content_id: CartonContentId,
    pub session_id: PackSessionId,
    pub carton_id: CartonId,
    pub order_id: OrderId,
    pub order_line_id: OrderLineId,
    pub inventory_allocation_id: InventoryAllocationId,
    pub inventory_transaction_id: i64,
    pub source_inventory_allocation_id: InventoryAllocationId,
    pub destination_inventory_allocation_id: InventoryAllocationId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub destination_inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub destination_location_id: LocationId,
    pub source_license_plate_id: LicensePlateId,
    pub destination_license_plate_id: LicensePlateId,
    pub item_batch_id: ItemBatchId,
    pub item_id: i64,
    pub quantity: PackQuantity,
    pub uom: String,
    pub packed_by: UserId,
    pub packed_at: Timestamp,
    pub revision: OrderRevision,
    pub progress: PackingProgress,
}

/// Returns one active content row from an open carton to its original picked tote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovePackedContentCommand {
    pub session_id: PackSessionId,
    pub carton_id: CartonId,
    pub content_id: CartonContentId,
    pub carton_barcode: PackScanValue,
    pub item_barcode: PackScanValue,
    pub lot_scan: Option<PackScanValue>,
    pub serial_scan: Option<PackScanValue>,
    pub destination_license_plate_barcode: PackScanValue,
    pub details: PackContentRemovalDetails,
    pub expected_revision: OrderRevision,
}

/// Replay-stable inventory and workflow evidence for one pack reversal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovePackedContentResult {
    pub removal_id: CartonContentRemovalId,
    pub content_id: CartonContentId,
    pub session_id: PackSessionId,
    pub carton_id: CartonId,
    pub order_id: OrderId,
    pub order_line_id: OrderLineId,
    pub inventory_transaction_id: i64,
    pub source_inventory_allocation_id: InventoryAllocationId,
    pub destination_inventory_allocation_id: InventoryAllocationId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub destination_inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub destination_location_id: LocationId,
    pub source_license_plate_id: LicensePlateId,
    pub destination_license_plate_id: LicensePlateId,
    pub item_batch_id: ItemBatchId,
    pub item_id: i64,
    pub quantity: PackQuantity,
    pub uom: String,
    pub details: PackContentRemovalDetails,
    pub removed_by: UserId,
    pub removed_at: Timestamp,
    pub revision: OrderRevision,
    pub progress: PackingProgress,
}

/// Closes one nonempty carton using exact scans and optional measured facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseCartonCommand {
    pub session_id: PackSessionId,
    pub carton_id: CartonId,
    pub carton_barcode: PackScanValue,
    pub measurements: CartonMeasurements,
    pub expected_revision: OrderRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseCartonResult {
    pub session_id: PackSessionId,
    pub carton_id: CartonId,
    pub order_id: OrderId,
    pub lifecycle: PackCartonLifecycle,
    pub order_status: OrderStatus,
    pub revision: OrderRevision,
    pub progress: PackingProgress,
}

/// Permanently abandons an empty carton while preserving its audit identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoidCartonCommand {
    pub session_id: PackSessionId,
    pub carton_id: CartonId,
    pub carton_barcode: PackScanValue,
    pub expected_revision: OrderRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoidCartonResult {
    pub session_id: PackSessionId,
    pub carton_id: CartonId,
    pub order_id: OrderId,
    pub lifecycle: PackCartonLifecycle,
    pub revision: OrderRevision,
    pub progress: PackingProgress,
}

impl CloseCartonResult {
    pub const fn session_status(&self) -> PackSessionStatus {
        self.progress.status()
    }

    pub const fn ready_to_manifest(&self) -> bool {
        self.progress.ready_to_manifest()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mutation_carries_the_mirrored_expected_revision() {
        let revision = OrderRevision::new(4).unwrap();
        let session_id = PackSessionId::new(5).unwrap();
        let carton_id = CartonId::new(6).unwrap();

        assert_eq!(
            CreateCartonCommand {
                session_id,
                carton_barcode: PackScanValue::new("CARTON-1").unwrap(),
                expected_revision: revision,
            }
            .expected_revision,
            revision
        );
        assert_eq!(
            PackPickedAllocationCommand {
                session_id,
                carton_id,
                inventory_allocation_id: InventoryAllocationId::new(7).unwrap(),
                item_barcode: PackScanValue::new("SKU-1").unwrap(),
                lot_scan: Some(PackScanValue::new("LOT-1").unwrap()),
                serial_scan: Some(PackScanValue::new("SERIAL-1").unwrap()),
                source_license_plate_barcode: PackScanValue::new("TOTE-1").unwrap(),
                carton_barcode: PackScanValue::new("CARTON-1").unwrap(),
                expected_revision: revision,
            }
            .expected_revision,
            revision
        );
        assert_eq!(
            CloseCartonCommand {
                session_id,
                carton_id,
                carton_barcode: PackScanValue::new("CARTON-1").unwrap(),
                measurements: CartonMeasurements::default(),
                expected_revision: revision,
            }
            .expected_revision,
            revision
        );
        assert_eq!(
            VoidCartonCommand {
                session_id,
                carton_id,
                carton_barcode: PackScanValue::new("CARTON-1").unwrap(),
                expected_revision: revision,
            }
            .expected_revision,
            revision
        );
    }

    #[test]
    fn close_result_derives_readiness_from_conserved_progress() {
        let result = CloseCartonResult {
            session_id: PackSessionId::new(1).unwrap(),
            carton_id: CartonId::new(2).unwrap(),
            order_id: OrderId::new(3).unwrap(),
            lifecycle: PackCartonLifecycle::Closed {
                measurements: CartonMeasurements::default(),
                closed_by: UserId::new(4).unwrap(),
                closed_at: "2026-08-08T20:00:00Z".parse().unwrap(),
            },
            order_status: OrderStatus::AwaitingShipment,
            revision: OrderRevision::new(8).unwrap(),
            progress: PackingProgress::new(2, 2, 8, 8, 0, 1).unwrap(),
        };

        assert!(result.ready_to_manifest());
        assert_eq!(result.session_status(), PackSessionStatus::ReadyToManifest);
    }
}
