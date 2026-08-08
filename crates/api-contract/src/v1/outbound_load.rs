//! Strict V1 transport contracts for outbound-load execution.

use serde::{Deserialize, Serialize};

use super::{
    CursorPage, OpaqueCursor, PageLimit, Revision, ShipmentDemandResponse, ShipmentOrderStatus,
    ShipmentStatus,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundLoadStatus {
    Planned,
    Staging,
    Loading,
    ReadyToDepart,
    Departed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackedCartonMovementKind {
    Stage,
    Load,
    Unload,
    Unstage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundLoadCancellationReason {
    RouteCancelled,
    CarrierCancelled,
    EquipmentUnavailable,
    PlanningError,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanOutboundLoadCartonRequest {
    pub carton_id: i64,
    pub load_sequence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanOutboundLoadShipmentRequest {
    pub shipment_id: i64,
    pub expected_shipment_revision: Revision,
    pub expected_order_revision: Revision,
    pub shipment_sequence: u32,
    pub cartons: Vec<PlanOutboundLoadCartonRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanOutboundLoadRequest {
    pub facility_id: i64,
    pub load_reference: String,
    pub carrier_code: String,
    pub staging_location_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_departure_at: Option<String>,
    pub shipments: Vec<PlanOutboundLoadShipmentRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseOutboundLoadRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageOutboundCartonRequest {
    pub expected_load_revision: Revision,
    pub expected_position_revision: Revision,
    pub source_location_barcode: String,
    pub carton_barcode: String,
    pub staging_location_barcode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartOutboundLoadLoadingRequest {
    pub expected_revision: Revision,
    pub load_barcode: String,
    pub staging_location_barcode: String,
    pub dock_location_barcode: String,
    pub trailer_number: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadOutboundCartonRequest {
    pub expected_load_revision: Revision,
    pub expected_position_revision: Revision,
    pub staging_location_barcode: String,
    pub carton_barcode: String,
    pub trailer_number: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteOutboundLoadLoadingRequest {
    pub expected_revision: Revision,
    pub load_barcode: String,
    pub dock_location_barcode: String,
    pub trailer_number: String,
    pub seal_number: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnloadOutboundCartonRequest {
    pub expected_load_revision: Revision,
    pub expected_position_revision: Revision,
    pub trailer_number: String,
    pub carton_barcode: String,
    pub staging_location_barcode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnstageOutboundCartonRequest {
    pub expected_load_revision: Revision,
    pub expected_position_revision: Revision,
    pub staging_location_barcode: String,
    pub carton_barcode: String,
    pub return_location_barcode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelOutboundLoadRequest {
    pub expected_revision: Revision,
    pub reason: OutboundLoadCancellationReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmOutboundLoadDepartureRequest {
    pub expected_revision: Revision,
    pub load_barcode: String,
    pub dock_location_barcode: String,
    pub trailer_number: String,
    pub seal_number: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundLoadQueuePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<OutboundLoadStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundLoadProgressResponse {
    pub planned_shipment_count: u32,
    pub planned_carton_count: u32,
    pub staged_carton_count: u32,
    pub loaded_carton_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PackedCartonPositionStateResponse {
    Packed {
        location_id: i64,
    },
    Staged {
        outbound_load_id: i64,
        staging_location_id: i64,
    },
    Loaded {
        outbound_load_id: i64,
        load_sequence: u32,
    },
    Departed {
        outbound_load_id: Option<i64>,
        load_sequence: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackedCartonContentPositionResponse {
    pub position_id: i64,
    pub carton_content_id: i64,
    pub current_inventory_allocation_id: Option<i64>,
    pub current_inventory_balance_id: Option<i64>,
    pub current_location_id: Option<i64>,
    pub current_license_plate_id: Option<i64>,
    pub packed_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackedCartonPositionResponse {
    pub carton_id: i64,
    pub carton_barcode: String,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub state: PackedCartonPositionStateResponse,
    pub revision: Revision,
    pub contents: Vec<PackedCartonContentPositionResponse>,
    pub positioned_at: String,
    pub departed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundLoadCartonResponse {
    pub outbound_load_carton_id: i64,
    pub shipment_id: i64,
    pub carton_id: i64,
    pub carton_barcode: String,
    pub license_plate_id: i64,
    pub load_sequence: u32,
    pub state: PackedCartonPositionStateResponse,
    pub position_revision: Revision,
    pub content_count: i64,
    pub packed_quantity: i64,
    pub last_movement_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundLoadShipmentResponse {
    pub outbound_load_shipment_id: i64,
    pub shipment_id: i64,
    pub order_id: i64,
    pub order_key: String,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub shipment_sequence: u32,
    pub shipment_status: ShipmentStatus,
    pub shipment_revision: Revision,
    pub order_status: ShipmentOrderStatus,
    pub order_revision: Revision,
    pub demand: ShipmentDemandResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundLoadResponse {
    pub outbound_load_id: i64,
    pub load_reference: String,
    pub load_barcode: String,
    pub carrier_code: String,
    pub facility_id: i64,
    pub status: OutboundLoadStatus,
    pub revision: Revision,
    pub progress: OutboundLoadProgressResponse,
    pub staging_location_id: i64,
    pub staging_location_barcode: String,
    pub staging_location_name: String,
    pub dock_location_id: Option<i64>,
    pub dock_location_barcode: Option<String>,
    pub dock_location_name: Option<String>,
    pub virtual_trailer_location_id: i64,
    pub trailer_number: Option<String>,
    pub seal_number: Option<String>,
    pub scheduled_departure_at: Option<String>,
    pub shipments: Vec<OutboundLoadShipmentResponse>,
    pub cartons: Vec<OutboundLoadCartonResponse>,
    pub planned_by: i64,
    pub planned_at: String,
    pub released_by: Option<i64>,
    pub released_at: Option<String>,
    pub loading_started_by: Option<i64>,
    pub loading_started_at: Option<String>,
    pub ready_to_depart_by: Option<i64>,
    pub ready_to_depart_at: Option<String>,
    pub departed_by: Option<i64>,
    pub departed_at: Option<String>,
    pub cancelled_by: Option<i64>,
    pub cancelled_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundLoadQueueEntryResponse {
    pub outbound_load_id: i64,
    pub load_reference: String,
    pub carrier_code: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub status: OutboundLoadStatus,
    pub revision: Revision,
    pub progress: OutboundLoadProgressResponse,
    pub staging_location_name: String,
    pub dock_location_name: Option<String>,
    pub trailer_number: Option<String>,
    pub scheduled_departure_at: Option<String>,
}

pub type OutboundLoadQueuePage = CursorPage<OutboundLoadQueueEntryResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanOutboundLoadResponse {
    pub outbound_load: OutboundLoadResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseOutboundLoadResponse {
    pub outbound_load_id: i64,
    pub status: OutboundLoadStatus,
    pub revision: Revision,
    pub progress: OutboundLoadProgressResponse,
    pub released_by: i64,
    pub released_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartOutboundLoadLoadingResponse {
    pub outbound_load_id: i64,
    pub status: OutboundLoadStatus,
    pub revision: Revision,
    pub dock_location_id: i64,
    pub trailer_number: String,
    pub started_by: i64,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteOutboundLoadLoadingResponse {
    pub outbound_load_id: i64,
    pub status: OutboundLoadStatus,
    pub revision: Revision,
    pub seal_number: String,
    pub completed_by: i64,
    pub completed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackedCartonMovementDetailResponse {
    pub carton_content_id: i64,
    pub source_inventory_allocation_id: i64,
    pub destination_inventory_allocation_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_inventory_balance_id: i64,
    pub quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackedCartonMovementResponse {
    pub movement_id: i64,
    pub outbound_load_id: i64,
    pub outbound_load_carton_id: i64,
    pub carton_id: i64,
    pub kind: PackedCartonMovementKind,
    pub inventory_transaction_id: i64,
    pub source_location_id: i64,
    pub destination_location_id: i64,
    pub quantity: i64,
    pub details: Vec<PackedCartonMovementDetailResponse>,
    pub moved_by: i64,
    pub moved_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MovePackedCartonResponse {
    pub movement: PackedCartonMovementResponse,
    pub position: PackedCartonPositionResponse,
    pub outbound_load_id: i64,
    pub load_status: OutboundLoadStatus,
    pub load_revision: Revision,
    pub progress: OutboundLoadProgressResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelOutboundLoadResponse {
    pub cancellation_id: i64,
    pub outbound_load_id: i64,
    pub status: OutboundLoadStatus,
    pub revision: Revision,
    pub cancelled_by: i64,
    pub cancelled_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundLoadShipmentDepartureResponse {
    pub shipment_id: i64,
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_transaction_id: i64,
    pub shipment_status: ShipmentStatus,
    pub shipment_revision: Revision,
    pub order_status: ShipmentOrderStatus,
    pub order_revision: Revision,
    pub demand: ShipmentDemandResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmOutboundLoadDepartureResponse {
    pub outbound_load_id: i64,
    pub status: OutboundLoadStatus,
    pub revision: Revision,
    pub shipment_departures: Vec<OutboundLoadShipmentDepartureResponse>,
    pub departed_by: i64,
    pub departed_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_is_strict() {
        let value = serde_json::json!({
            "facility_id": 1,
            "load_reference": "LOAD-1",
            "carrier_code": "CARRIER",
            "staging_location_id": 2,
            "shipments": [{
                "shipment_id": 3,
                "expected_shipment_revision": 2,
                "expected_order_revision": 5,
                "shipment_sequence": 1,
                "cartons": [{"carton_id": 4, "load_sequence": 1}]
            }]
        });
        assert!(serde_json::from_value::<PlanOutboundLoadRequest>(value.clone()).is_ok());
        let mut unknown = value;
        unknown["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<PlanOutboundLoadRequest>(unknown).is_err());
    }

    #[test]
    fn scanner_requests_reject_unknown_fields() {
        let request = serde_json::json!({
            "expected_load_revision": 1,
            "expected_position_revision": 1,
            "source_location_barcode": "PACK-01",
            "carton_barcode": "CARTON-01",
            "staging_location_barcode": "STAGE-01",
            "quantity": 1
        });
        assert!(serde_json::from_value::<StageOutboundCartonRequest>(request).is_err());
    }
}
