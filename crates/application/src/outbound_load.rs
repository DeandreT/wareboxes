//! Application contracts for outbound-load planning and physical carton execution.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CarrierCode, CartonContentId, CartonId, FacilityId, InventoryAllocationId, InventoryBalanceId,
    InventoryOwnerId, LocationId, OrderId, OrderRevision, OrderStatus,
    OutboundLoadCancellationDetails, OutboundLoadCancellationId, OutboundLoadCartonId,
    OutboundLoadId, OutboundLoadProgress, OutboundLoadReference, OutboundLoadRevision,
    OutboundLoadScanValue, OutboundLoadShipmentId, OutboundLoadStatus, PackedCartonMovementId,
    PackedCartonMovementKind, PackedCartonPositionId, PackedCartonPositionRevision,
    PackedCartonPositionState, SealNumber, ShipmentId, ShipmentRevision, ShipmentScanValue,
    ShipmentStatus, ShortShipDemandQuantities, Timestamp, TrailerNumber, UserId,
};

pub const PLAN_OUTBOUND_LOAD_OPERATION: &str = "outbound.load.plan.v1";
pub const RELEASE_OUTBOUND_LOAD_OPERATION: &str = "outbound.load.release.v1";
pub const STAGE_OUTBOUND_LOAD_CARTON_OPERATION: &str = "outbound.load.carton.stage.v1";
pub const START_OUTBOUND_LOAD_LOADING_OPERATION: &str = "outbound.load.loading.start.v1";
pub const LOAD_OUTBOUND_LOAD_CARTON_OPERATION: &str = "outbound.load.carton.load.v1";
pub const COMPLETE_OUTBOUND_LOAD_LOADING_OPERATION: &str = "outbound.load.loading.complete.v1";
pub const UNLOAD_OUTBOUND_LOAD_CARTON_OPERATION: &str = "outbound.load.carton.unload.v1";
pub const UNSTAGE_OUTBOUND_LOAD_CARTON_OPERATION: &str = "outbound.load.carton.unstage.v1";
pub const CANCEL_OUTBOUND_LOAD_OPERATION: &str = "outbound.load.cancel.v1";
pub const CONFIRM_OUTBOUND_LOAD_DEPARTURE_OPERATION: &str = "outbound.load.depart.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanOutboundLoadCarton {
    pub carton_id: CartonId,
    pub load_sequence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanOutboundLoadShipment {
    pub shipment_id: ShipmentId,
    pub expected_shipment_revision: ShipmentRevision,
    pub expected_order_revision: OrderRevision,
    pub shipment_sequence: u32,
    pub cartons: Vec<PlanOutboundLoadCarton>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanOutboundLoadCommand {
    pub facility_id: FacilityId,
    pub load_reference: OutboundLoadReference,
    pub carrier_code: CarrierCode,
    pub staging_location_id: LocationId,
    pub scheduled_departure_at: Option<Timestamp>,
    pub shipments: Vec<PlanOutboundLoadShipment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundLoadQuery {
    pub outbound_load_id: OutboundLoadId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackedCartonPositionQuery {
    pub carton_id: CartonId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundLoadQueueQuery {
    pub facility_id: Option<FacilityId>,
    pub status: Option<OutboundLoadStatus>,
    pub scheduled_from: Option<Timestamp>,
    pub scheduled_to: Option<Timestamp>,
    pub cursor: Option<OutboundLoadCursor>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundLoadCursor {
    pub scheduled_departure_at: Option<Timestamp>,
    pub outbound_load_id: OutboundLoadId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundLoadProgressReadModel {
    pub planned_shipment_count: u32,
    pub planned_carton_count: u32,
    pub staged_carton_count: u32,
    pub loaded_carton_count: u32,
}

impl From<OutboundLoadProgress> for OutboundLoadProgressReadModel {
    fn from(value: OutboundLoadProgress) -> Self {
        Self {
            planned_shipment_count: value.planned_shipment_count(),
            planned_carton_count: value.planned_carton_count(),
            staged_carton_count: value.staged_carton_count(),
            loaded_carton_count: value.loaded_carton_count(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackedCartonContentPositionReadModel {
    pub position_id: PackedCartonPositionId,
    pub carton_content_id: CartonContentId,
    pub current_inventory_allocation_id: Option<InventoryAllocationId>,
    pub current_inventory_balance_id: Option<InventoryBalanceId>,
    pub current_location_id: Option<LocationId>,
    pub current_license_plate_id: Option<wareboxes_domain::LicensePlateId>,
    pub packed_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackedCartonPositionReadModel {
    pub carton_id: CartonId,
    pub carton_barcode: ShipmentScanValue,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub state: PackedCartonPositionState,
    pub revision: PackedCartonPositionRevision,
    pub contents: Vec<PackedCartonContentPositionReadModel>,
    pub positioned_at: Timestamp,
    pub departed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundLoadCartonReadModel {
    pub outbound_load_carton_id: OutboundLoadCartonId,
    pub shipment_id: ShipmentId,
    pub carton_id: CartonId,
    pub carton_barcode: ShipmentScanValue,
    pub license_plate_id: wareboxes_domain::LicensePlateId,
    pub load_sequence: u32,
    pub state: PackedCartonPositionState,
    pub position_revision: PackedCartonPositionRevision,
    pub content_count: i64,
    pub packed_quantity: i64,
    pub last_movement_id: Option<PackedCartonMovementId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundLoadShipmentReadModel {
    pub outbound_load_shipment_id: OutboundLoadShipmentId,
    pub shipment_id: ShipmentId,
    pub order_id: OrderId,
    pub order_key: String,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub shipment_sequence: u32,
    pub shipment_status: ShipmentStatus,
    pub shipment_revision: ShipmentRevision,
    pub order_status: OrderStatus,
    pub order_revision: OrderRevision,
    pub demand: ShortShipDemandQuantities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundLoadReadModel {
    pub outbound_load_id: OutboundLoadId,
    pub load_reference: OutboundLoadReference,
    pub load_barcode: OutboundLoadScanValue,
    pub carrier_code: CarrierCode,
    pub facility_id: FacilityId,
    pub status: OutboundLoadStatus,
    pub revision: OutboundLoadRevision,
    pub progress: OutboundLoadProgressReadModel,
    pub staging_location_id: LocationId,
    pub staging_location_barcode: String,
    pub staging_location_name: String,
    pub dock_location_id: Option<LocationId>,
    pub dock_location_barcode: Option<String>,
    pub dock_location_name: Option<String>,
    pub virtual_trailer_location_id: LocationId,
    pub trailer_number: Option<TrailerNumber>,
    pub seal_number: Option<SealNumber>,
    pub scheduled_departure_at: Option<Timestamp>,
    pub shipments: Vec<OutboundLoadShipmentReadModel>,
    pub cartons: Vec<OutboundLoadCartonReadModel>,
    pub planned_by: UserId,
    pub planned_at: Timestamp,
    pub released_by: Option<UserId>,
    pub released_at: Option<Timestamp>,
    pub loading_started_by: Option<UserId>,
    pub loading_started_at: Option<Timestamp>,
    pub ready_to_depart_by: Option<UserId>,
    pub ready_to_depart_at: Option<Timestamp>,
    pub departed_by: Option<UserId>,
    pub departed_at: Option<Timestamp>,
    pub cancelled_by: Option<UserId>,
    pub cancelled_at: Option<Timestamp>,
}

impl OutboundLoadReadModel {
    pub fn is_consistent(&self) -> bool {
        if self.shipments.is_empty() || self.cartons.is_empty() {
            return false;
        }
        let progress_matches = self.progress.planned_shipment_count as usize
            == self.shipments.len()
            && self.progress.planned_carton_count as usize == self.cartons.len();
        let phase_fields_match = match self.status {
            OutboundLoadStatus::Planned => self.released_at.is_none(),
            OutboundLoadStatus::Staging => self.released_at.is_some(),
            OutboundLoadStatus::Loading => {
                self.released_at.is_some() && self.loading_started_at.is_some()
            }
            OutboundLoadStatus::ReadyToDepart => {
                self.loading_started_at.is_some() && self.ready_to_depart_at.is_some()
            }
            OutboundLoadStatus::Departed => {
                self.ready_to_depart_at.is_some() && self.departed_at.is_some()
            }
            OutboundLoadStatus::Cancelled => self.cancelled_at.is_some(),
        };
        progress_matches && phase_fields_match
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundLoadQueueEntryReadModel {
    pub outbound_load_id: OutboundLoadId,
    pub load_reference: OutboundLoadReference,
    pub carrier_code: CarrierCode,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub status: OutboundLoadStatus,
    pub revision: OutboundLoadRevision,
    pub progress: OutboundLoadProgressReadModel,
    pub staging_location_name: String,
    pub dock_location_name: Option<String>,
    pub trailer_number: Option<TrailerNumber>,
    pub scheduled_departure_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundLoadQueuePage {
    pub entries: Vec<OutboundLoadQueueEntryReadModel>,
    pub next_cursor: Option<OutboundLoadCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanOutboundLoadResult {
    pub outbound_load: OutboundLoadReadModel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseOutboundLoadCommand {
    pub outbound_load_id: OutboundLoadId,
    pub expected_revision: OutboundLoadRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseOutboundLoadResult {
    pub outbound_load_id: OutboundLoadId,
    pub status: OutboundLoadStatus,
    pub revision: OutboundLoadRevision,
    pub progress: OutboundLoadProgressReadModel,
    pub released_by: UserId,
    pub released_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagePackedCartonCommand {
    pub outbound_load_id: OutboundLoadId,
    pub carton_id: CartonId,
    pub expected_load_revision: OutboundLoadRevision,
    pub expected_position_revision: PackedCartonPositionRevision,
    pub source_location_barcode: OutboundLoadScanValue,
    pub carton_barcode: OutboundLoadScanValue,
    pub staging_location_barcode: OutboundLoadScanValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOutboundLoadLoadingCommand {
    pub outbound_load_id: OutboundLoadId,
    pub expected_revision: OutboundLoadRevision,
    pub load_barcode: OutboundLoadScanValue,
    pub staging_location_barcode: OutboundLoadScanValue,
    pub dock_location_barcode: OutboundLoadScanValue,
    pub trailer_number: TrailerNumber,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOutboundLoadLoadingResult {
    pub outbound_load_id: OutboundLoadId,
    pub status: OutboundLoadStatus,
    pub revision: OutboundLoadRevision,
    pub dock_location_id: LocationId,
    pub trailer_number: TrailerNumber,
    pub started_by: UserId,
    pub started_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadPackedCartonCommand {
    pub outbound_load_id: OutboundLoadId,
    pub carton_id: CartonId,
    pub expected_load_revision: OutboundLoadRevision,
    pub expected_position_revision: PackedCartonPositionRevision,
    pub staging_location_barcode: OutboundLoadScanValue,
    pub carton_barcode: OutboundLoadScanValue,
    pub trailer_number: TrailerNumber,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteOutboundLoadLoadingCommand {
    pub outbound_load_id: OutboundLoadId,
    pub expected_revision: OutboundLoadRevision,
    pub load_barcode: OutboundLoadScanValue,
    pub dock_location_barcode: OutboundLoadScanValue,
    pub trailer_number: TrailerNumber,
    pub seal_number: SealNumber,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteOutboundLoadLoadingResult {
    pub outbound_load_id: OutboundLoadId,
    pub status: OutboundLoadStatus,
    pub revision: OutboundLoadRevision,
    pub seal_number: SealNumber,
    pub completed_by: UserId,
    pub completed_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnloadPackedCartonCommand {
    pub outbound_load_id: OutboundLoadId,
    pub carton_id: CartonId,
    pub expected_load_revision: OutboundLoadRevision,
    pub expected_position_revision: PackedCartonPositionRevision,
    pub trailer_number: TrailerNumber,
    pub carton_barcode: OutboundLoadScanValue,
    pub staging_location_barcode: OutboundLoadScanValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnstagePackedCartonCommand {
    pub outbound_load_id: OutboundLoadId,
    pub carton_id: CartonId,
    pub expected_load_revision: OutboundLoadRevision,
    pub expected_position_revision: PackedCartonPositionRevision,
    pub staging_location_barcode: OutboundLoadScanValue,
    pub carton_barcode: OutboundLoadScanValue,
    pub return_location_barcode: OutboundLoadScanValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackedCartonMovementDetailReadModel {
    pub carton_content_id: CartonContentId,
    pub source_inventory_allocation_id: InventoryAllocationId,
    pub destination_inventory_allocation_id: InventoryAllocationId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub destination_inventory_balance_id: InventoryBalanceId,
    pub quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackedCartonMovementReadModel {
    pub movement_id: PackedCartonMovementId,
    pub outbound_load_id: OutboundLoadId,
    pub outbound_load_carton_id: OutboundLoadCartonId,
    pub carton_id: CartonId,
    pub kind: PackedCartonMovementKind,
    pub inventory_transaction_id: i64,
    pub source_location_id: LocationId,
    pub destination_location_id: LocationId,
    pub quantity: i64,
    pub details: Vec<PackedCartonMovementDetailReadModel>,
    pub moved_by: UserId,
    pub moved_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovePackedCartonResult {
    pub movement: PackedCartonMovementReadModel,
    pub position: PackedCartonPositionReadModel,
    pub outbound_load_id: OutboundLoadId,
    pub load_status: OutboundLoadStatus,
    pub load_revision: OutboundLoadRevision,
    pub progress: OutboundLoadProgressReadModel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelOutboundLoadCommand {
    pub outbound_load_id: OutboundLoadId,
    pub expected_revision: OutboundLoadRevision,
    pub details: OutboundLoadCancellationDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelOutboundLoadResult {
    pub cancellation_id: OutboundLoadCancellationId,
    pub outbound_load_id: OutboundLoadId,
    pub status: OutboundLoadStatus,
    pub revision: OutboundLoadRevision,
    pub cancelled_by: UserId,
    pub cancelled_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmOutboundLoadDepartureCommand {
    pub outbound_load_id: OutboundLoadId,
    pub expected_revision: OutboundLoadRevision,
    pub load_barcode: OutboundLoadScanValue,
    pub dock_location_barcode: OutboundLoadScanValue,
    pub trailer_number: TrailerNumber,
    pub seal_number: SealNumber,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundLoadShipmentDepartureResult {
    pub shipment_id: ShipmentId,
    pub order_id: OrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_transaction_id: i64,
    pub shipment_status: ShipmentStatus,
    pub shipment_revision: ShipmentRevision,
    pub order_status: OrderStatus,
    pub order_revision: OrderRevision,
    pub demand: ShortShipDemandQuantities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmOutboundLoadDepartureResult {
    pub outbound_load_id: OutboundLoadId,
    pub status: OutboundLoadStatus,
    pub revision: OutboundLoadRevision,
    pub shipment_departures: Vec<OutboundLoadShipmentDepartureResult>,
    pub departed_by: UserId,
    pub departed_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_mapping_preserves_execution_counts() {
        let progress =
            OutboundLoadProgress::restore(2, 4, 1, 2, OutboundLoadStatus::Loading).unwrap();
        let read = OutboundLoadProgressReadModel::from(progress);
        assert_eq!(read.planned_shipment_count, 2);
        assert_eq!(read.planned_carton_count, 4);
        assert_eq!(read.staged_carton_count, 1);
        assert_eq!(read.loaded_carton_count, 2);
    }
}
