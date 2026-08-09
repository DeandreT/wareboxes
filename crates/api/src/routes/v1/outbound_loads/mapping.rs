use wareboxes_api_contract::v1::{
    CancelOutboundLoadResponse, CompleteOutboundLoadLoadingResponse,
    ConfirmOutboundLoadDepartureResponse, MovePackedCartonResponse, OpaqueCursor,
    OutboundLoadCartonResponse, OutboundLoadProgressResponse, OutboundLoadQueueEntryResponse,
    OutboundLoadQueuePage, OutboundLoadResponse, OutboundLoadShipmentDepartureResponse,
    OutboundLoadShipmentResponse, OutboundLoadStatus as ApiStatus,
    PackedCartonContentPositionResponse, PackedCartonMovementDetailResponse,
    PackedCartonMovementKind as ApiMovementKind, PackedCartonMovementResponse,
    PackedCartonPositionResponse, PackedCartonPositionStateResponse, PlanOutboundLoadResponse,
    ReleaseOutboundLoadResponse, Revision, ShipmentDemandResponse, ShipmentOrderStatus,
    ShipmentStatus as ApiShipmentStatus, StartOutboundLoadLoadingResponse,
};
use wareboxes_application::outbound_load::{
    CancelOutboundLoadResult, CompleteOutboundLoadLoadingResult,
    ConfirmOutboundLoadDepartureResult, MovePackedCartonResult, OutboundLoadProgressReadModel,
    OutboundLoadQueueEntryReadModel, OutboundLoadReadModel, OutboundLoadShipmentDepartureResult,
    OutboundLoadShipmentReadModel, PackedCartonPositionReadModel, PlanOutboundLoadResult,
    ReleaseOutboundLoadResult, StartOutboundLoadLoadingResult,
};
use wareboxes_domain::{
    OrderStatus, OutboundLoadStatus, PackedCartonMovementKind, PackedCartonPositionState,
    ShipmentStatus,
};

use super::{V1Error, V1Result};
use crate::error::{AppError, AppResult};

pub(super) fn plan_result(result: PlanOutboundLoadResult) -> V1Result<PlanOutboundLoadResponse> {
    Ok(PlanOutboundLoadResponse {
        outbound_load: load(result.outbound_load)?,
    })
}

pub(super) fn load(value: OutboundLoadReadModel) -> V1Result<OutboundLoadResponse> {
    Ok(OutboundLoadResponse {
        outbound_load_id: value.outbound_load_id.get(),
        load_reference: value.load_reference.into_inner(),
        load_barcode: value.load_barcode.into_inner(),
        carrier_code: value.carrier_code.into_inner(),
        facility_id: value.facility_id.get(),
        status: status(value.status),
        revision: revision(value.revision.get())?,
        progress: progress(value.progress),
        staging_location_id: value.staging_location_id.get(),
        staging_location_barcode: value.staging_location_barcode,
        staging_location_name: value.staging_location_name,
        dock_location_id: value.dock_location_id.map(|id| id.get()),
        dock_location_barcode: value.dock_location_barcode,
        dock_location_name: value.dock_location_name,
        virtual_trailer_location_id: value.virtual_trailer_location_id.get(),
        trailer_number: value.trailer_number.map(|value| value.into_inner()),
        seal_number: value.seal_number.map(|value| value.into_inner()),
        scheduled_departure_at: value.scheduled_departure_at.map(|value| value.to_rfc3339()),
        shipments: value
            .shipments
            .into_iter()
            .map(shipment)
            .collect::<V1Result<Vec<_>>>()?,
        cartons: value
            .cartons
            .into_iter()
            .map(|carton| {
                Ok(OutboundLoadCartonResponse {
                    outbound_load_carton_id: carton.outbound_load_carton_id.get(),
                    shipment_id: carton.shipment_id.get(),
                    carton_id: carton.carton_id.get(),
                    carton_barcode: carton.carton_barcode.into_inner(),
                    license_plate_id: carton.license_plate_id.get(),
                    load_sequence: carton.load_sequence,
                    state: position_state(carton.state),
                    position_revision: revision(carton.position_revision.get())?,
                    content_count: carton.content_count,
                    packed_quantity: carton.packed_quantity,
                    last_movement_id: carton.last_movement_id.map(|id| id.get()),
                })
            })
            .collect::<V1Result<Vec<_>>>()?,
        planned_by: value.planned_by.get(),
        planned_at: value.planned_at.to_rfc3339(),
        released_by: value.released_by.map(|id| id.get()),
        released_at: value.released_at.map(|value| value.to_rfc3339()),
        loading_started_by: value.loading_started_by.map(|id| id.get()),
        loading_started_at: value.loading_started_at.map(|value| value.to_rfc3339()),
        ready_to_depart_by: value.ready_to_depart_by.map(|id| id.get()),
        ready_to_depart_at: value.ready_to_depart_at.map(|value| value.to_rfc3339()),
        departed_by: value.departed_by.map(|id| id.get()),
        departed_at: value.departed_at.map(|value| value.to_rfc3339()),
        cancelled_by: value.cancelled_by.map(|id| id.get()),
        cancelled_at: value.cancelled_at.map(|value| value.to_rfc3339()),
    })
}

pub(super) fn queue_page(
    entries: Vec<OutboundLoadQueueEntryReadModel>,
    next_cursor: Option<OpaqueCursor>,
) -> AppResult<OutboundLoadQueuePage> {
    Ok(OutboundLoadQueuePage::new(
        entries
            .into_iter()
            .map(|entry| {
                Ok(OutboundLoadQueueEntryResponse {
                    outbound_load_id: entry.outbound_load_id.get(),
                    load_reference: entry.load_reference.into_inner(),
                    carrier_code: entry.carrier_code.into_inner(),
                    facility_id: entry.facility_id.get(),
                    facility_name: entry.facility_name,
                    status: status(entry.status),
                    revision: Revision::new(entry.revision.get())
                        .map_err(|error| AppError::internal(error.to_string()))?,
                    progress: progress(entry.progress),
                    staging_location_name: entry.staging_location_name,
                    dock_location_name: entry.dock_location_name,
                    trailer_number: entry.trailer_number.map(|value| value.into_inner()),
                    scheduled_departure_at: entry
                        .scheduled_departure_at
                        .map(|value| value.to_rfc3339()),
                })
            })
            .collect::<AppResult<Vec<_>>>()?,
        next_cursor,
    ))
}

pub(super) fn position(
    value: PackedCartonPositionReadModel,
) -> V1Result<PackedCartonPositionResponse> {
    Ok(PackedCartonPositionResponse {
        carton_id: value.carton_id.get(),
        carton_barcode: value.carton_barcode.into_inner(),
        inventory_owner_id: value.inventory_owner_id.get(),
        facility_id: value.facility_id.get(),
        state: position_state(value.state),
        revision: revision(value.revision.get())?,
        contents: value
            .contents
            .into_iter()
            .map(|content| PackedCartonContentPositionResponse {
                position_id: content.position_id.get(),
                carton_content_id: content.carton_content_id.get(),
                current_inventory_allocation_id: content
                    .current_inventory_allocation_id
                    .map(|id| id.get()),
                current_inventory_balance_id: content
                    .current_inventory_balance_id
                    .map(|id| id.get()),
                current_location_id: content.current_location_id.map(|id| id.get()),
                current_license_plate_id: content.current_license_plate_id.map(|id| id.get()),
                packed_quantity: content.packed_quantity,
            })
            .collect(),
        positioned_at: value.positioned_at.to_rfc3339(),
        departed_at: value.departed_at.map(|value| value.to_rfc3339()),
    })
}

pub(super) fn release(value: ReleaseOutboundLoadResult) -> V1Result<ReleaseOutboundLoadResponse> {
    Ok(ReleaseOutboundLoadResponse {
        outbound_load_id: value.outbound_load_id.get(),
        status: status(value.status),
        revision: revision(value.revision.get())?,
        progress: progress(value.progress),
        released_by: value.released_by.get(),
        released_at: value.released_at.to_rfc3339(),
    })
}

pub(super) fn start(
    value: StartOutboundLoadLoadingResult,
) -> V1Result<StartOutboundLoadLoadingResponse> {
    Ok(StartOutboundLoadLoadingResponse {
        outbound_load_id: value.outbound_load_id.get(),
        status: status(value.status),
        revision: revision(value.revision.get())?,
        dock_location_id: value.dock_location_id.get(),
        trailer_number: value.trailer_number.into_inner(),
        started_by: value.started_by.get(),
        started_at: value.started_at.to_rfc3339(),
    })
}

pub(super) fn complete(
    value: CompleteOutboundLoadLoadingResult,
) -> V1Result<CompleteOutboundLoadLoadingResponse> {
    Ok(CompleteOutboundLoadLoadingResponse {
        outbound_load_id: value.outbound_load_id.get(),
        status: status(value.status),
        revision: revision(value.revision.get())?,
        seal_number: value.seal_number.into_inner(),
        completed_by: value.completed_by.get(),
        completed_at: value.completed_at.to_rfc3339(),
    })
}

pub(super) fn movement_result(value: MovePackedCartonResult) -> V1Result<MovePackedCartonResponse> {
    Ok(MovePackedCartonResponse {
        movement: PackedCartonMovementResponse {
            movement_id: value.movement.movement_id.get(),
            outbound_load_id: value.movement.outbound_load_id.get(),
            outbound_load_carton_id: value.movement.outbound_load_carton_id.get(),
            carton_id: value.movement.carton_id.get(),
            kind: movement_kind(value.movement.kind),
            inventory_transaction_id: value.movement.inventory_transaction_id,
            source_location_id: value.movement.source_location_id.get(),
            destination_location_id: value.movement.destination_location_id.get(),
            quantity: value.movement.quantity,
            details: value
                .movement
                .details
                .into_iter()
                .map(|detail| PackedCartonMovementDetailResponse {
                    carton_content_id: detail.carton_content_id.get(),
                    source_inventory_allocation_id: detail.source_inventory_allocation_id.get(),
                    destination_inventory_allocation_id: detail
                        .destination_inventory_allocation_id
                        .get(),
                    source_inventory_balance_id: detail.source_inventory_balance_id.get(),
                    destination_inventory_balance_id: detail.destination_inventory_balance_id.get(),
                    quantity: detail.quantity,
                })
                .collect(),
            moved_by: value.movement.moved_by.get(),
            moved_at: value.movement.moved_at.to_rfc3339(),
        },
        position: position(value.position)?,
        outbound_load_id: value.outbound_load_id.get(),
        load_status: status(value.load_status),
        load_revision: revision(value.load_revision.get())?,
        progress: progress(value.progress),
    })
}

pub(super) fn cancel(value: CancelOutboundLoadResult) -> V1Result<CancelOutboundLoadResponse> {
    Ok(CancelOutboundLoadResponse {
        cancellation_id: value.cancellation_id.get(),
        outbound_load_id: value.outbound_load_id.get(),
        status: status(value.status),
        revision: revision(value.revision.get())?,
        cancelled_by: value.cancelled_by.get(),
        cancelled_at: value.cancelled_at.to_rfc3339(),
    })
}

pub(super) fn departure(
    value: ConfirmOutboundLoadDepartureResult,
) -> V1Result<ConfirmOutboundLoadDepartureResponse> {
    Ok(ConfirmOutboundLoadDepartureResponse {
        outbound_load_id: value.outbound_load_id.get(),
        status: status(value.status),
        revision: revision(value.revision.get())?,
        shipment_departures: value
            .shipment_departures
            .into_iter()
            .map(shipment_departure)
            .collect::<V1Result<Vec<_>>>()?,
        departed_by: value.departed_by.get(),
        departed_at: value.departed_at.to_rfc3339(),
    })
}

fn shipment(value: OutboundLoadShipmentReadModel) -> V1Result<OutboundLoadShipmentResponse> {
    Ok(OutboundLoadShipmentResponse {
        outbound_load_shipment_id: value.outbound_load_shipment_id.get(),
        shipment_id: value.shipment_id.get(),
        order_id: value.order_id.get(),
        order_key: value.order_key,
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        shipment_sequence: value.shipment_sequence,
        shipment_status: shipment_status(value.shipment_status),
        shipment_revision: revision(value.shipment_revision.get())?,
        order_status: order_status(value.order_status)?,
        order_revision: revision(value.order_revision.get())?,
        demand: demand(value.demand),
    })
}

fn shipment_departure(
    value: OutboundLoadShipmentDepartureResult,
) -> V1Result<OutboundLoadShipmentDepartureResponse> {
    Ok(OutboundLoadShipmentDepartureResponse {
        shipment_id: value.shipment_id.get(),
        order_id: value.order_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_transaction_id: value.inventory_transaction_id,
        shipment_status: shipment_status(value.shipment_status),
        shipment_revision: revision(value.shipment_revision.get())?,
        order_status: order_status(value.order_status)?,
        order_revision: revision(value.order_revision.get())?,
        demand: demand(value.demand),
    })
}

const fn progress(value: OutboundLoadProgressReadModel) -> OutboundLoadProgressResponse {
    OutboundLoadProgressResponse {
        planned_shipment_count: value.planned_shipment_count,
        planned_carton_count: value.planned_carton_count,
        staged_carton_count: value.staged_carton_count,
        loaded_carton_count: value.loaded_carton_count,
    }
}
const fn status(value: OutboundLoadStatus) -> ApiStatus {
    match value {
        OutboundLoadStatus::Planned => ApiStatus::Planned,
        OutboundLoadStatus::Staging => ApiStatus::Staging,
        OutboundLoadStatus::Loading => ApiStatus::Loading,
        OutboundLoadStatus::ReadyToDepart => ApiStatus::ReadyToDepart,
        OutboundLoadStatus::Departed => ApiStatus::Departed,
        OutboundLoadStatus::Cancelled => ApiStatus::Cancelled,
    }
}
const fn movement_kind(value: PackedCartonMovementKind) -> ApiMovementKind {
    match value {
        PackedCartonMovementKind::Stage => ApiMovementKind::Stage,
        PackedCartonMovementKind::Load => ApiMovementKind::Load,
        PackedCartonMovementKind::Unload => ApiMovementKind::Unload,
        PackedCartonMovementKind::Unstage => ApiMovementKind::Unstage,
    }
}
fn position_state(value: PackedCartonPositionState) -> PackedCartonPositionStateResponse {
    match value {
        PackedCartonPositionState::Packed { location_id } => {
            PackedCartonPositionStateResponse::Packed {
                location_id: location_id.get(),
            }
        }
        PackedCartonPositionState::Staged {
            outbound_load_id,
            staging_location_id,
        } => PackedCartonPositionStateResponse::Staged {
            outbound_load_id: outbound_load_id.get(),
            staging_location_id: staging_location_id.get(),
        },
        PackedCartonPositionState::Loaded {
            outbound_load_id,
            load_sequence,
        } => PackedCartonPositionStateResponse::Loaded {
            outbound_load_id: outbound_load_id.get(),
            load_sequence,
        },
        PackedCartonPositionState::Departed {
            outbound_load_id,
            load_sequence,
        } => PackedCartonPositionStateResponse::Departed {
            outbound_load_id: outbound_load_id.map(|id| id.get()),
            load_sequence,
        },
    }
}
const fn shipment_status(value: ShipmentStatus) -> ApiShipmentStatus {
    match value {
        ShipmentStatus::AwaitingManifest => ApiShipmentStatus::AwaitingManifest,
        ShipmentStatus::Manifested => ApiShipmentStatus::Manifested,
        ShipmentStatus::PartiallyDeparted => ApiShipmentStatus::PartiallyDeparted,
        ShipmentStatus::Departed => ApiShipmentStatus::Departed,
        ShipmentStatus::Cancelled => ApiShipmentStatus::Cancelled,
    }
}
fn order_status(value: OrderStatus) -> V1Result<ShipmentOrderStatus> {
    match value {
        OrderStatus::AwaitingShipment => Ok(ShipmentOrderStatus::AwaitingShipment),
        OrderStatus::Shipped => Ok(ShipmentOrderStatus::Shipped),
        _ => Err(V1Error::internal(
            "outbound load produced an invalid order status",
        )),
    }
}
const fn demand(value: wareboxes_domain::ShortShipDemandQuantities) -> ShipmentDemandResponse {
    ShipmentDemandResponse {
        ordered_quantity: value.ordered().get(),
        shipped_quantity: value.effective().get(),
        accepted_short_quantity: value.accepted_short().get(),
        accepted_substitute_quantity: value.accepted_substitute().get(),
    }
}
fn revision(value: i64) -> V1Result<Revision> {
    Revision::new(value)
        .map_err(|_| V1Error::internal("outbound load produced an invalid revision"))
}
