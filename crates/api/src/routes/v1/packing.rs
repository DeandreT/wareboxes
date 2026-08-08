use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CartonDimensions as ApiCartonDimensions, CartonMeasurements as ApiCartonMeasurements,
    CloseCartonRequest, CloseCartonResponse, CreateCartonRequest, CreateCartonResponse,
    DimensionMillimeters as ApiDimensionMillimeters, OpaqueCursor, OpenPackSessionRequest,
    OpenPackSessionResponse, PackAllocationDispositionResponse, PackCartonLifecycleResponse,
    PackCartonResponse, PackPickedAllocationRequest, PackPickedAllocationResponse,
    PackSessionResponse, PackSessionStatus as ApiSessionStatus, PackableAllocationResponse,
    PackingOrderStatus, PackingProgressResponse, PackingQueueEntryResponse,
    PackingQueueOrderStatus, PackingQueuePage as ApiPackingQueuePage, PackingQueuePageRequest,
    PackingQueueSessionResponse, Revision, VoidCartonRequest, VoidCartonResponse,
    WeightGrams as ApiWeightGrams,
};
use wareboxes_application::packing::{
    CloseCartonCommand, CloseCartonResult, CreateCartonCommand, CreateCartonResult,
    OpenPackSessionCommand, OpenPackSessionResult, PackAllocationDisposition, PackCarton,
    PackCartonLifecycle, PackPickedAllocationCommand, PackPickedAllocationResult, PackSessionQuery,
    PackSessionReadModel, PackableAllocation, VoidCartonCommand, VoidCartonResult,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CartonDimensions, CartonId, CartonMeasurements, DimensionMillimeters, FacilityId,
    InventoryAllocationId, LocationId, OrderId, OrderRevision, OrderStatus, PackScanValue,
    PackSessionId, PackSessionStatus, PackingProgress, WeightGrams, MAX_PACK_SCAN_VALUE_LENGTH,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const QUEUE_CURSOR_PREFIX: &str = "pq1.";

pub async fn queue(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<PackingQueuePageRequest>,
) -> V1Result<Json<ApiPackingQueuePage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let after = query.cursor.as_ref().map(decode_queue_cursor).transpose()?;
    let facility_id = query.facility_id.map(|id| id.get());
    if after
        .as_ref()
        .is_some_and(|cursor| cursor.facility_id != facility_id)
    {
        return Err(V1Error::invalid_cursor_for("packing queue"));
    }
    Ok(Json(
        page_for_access(
            &state,
            &user.tenant,
            facility_id,
            after.as_ref(),
            query.limit.get(),
        )
        .await?,
    ))
}

pub(crate) async fn page_for_access(
    state: &AppState,
    access: &TenantAccess,
    facility_id: Option<i64>,
    after: Option<&repo::packing::PackingQueueCursor>,
    limit: u16,
) -> AppResult<ApiPackingQueuePage> {
    let page = repo::packing::packing_queue(&state.db, access, facility_id, after, limit).await?;
    let items = page
        .items
        .into_iter()
        .map(map_queue_entry)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = page.next_cursor.map(encode_queue_cursor).transpose()?;
    Ok(ApiPackingQueuePage::new(items, next_cursor))
}

pub async fn for_order(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(order_id): Path<i64>,
) -> V1Result<Json<Option<PackSessionResponse>>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let session = repo::packing::packing_session_for_order(
        &state.db,
        &user.tenant,
        order_id_value(order_id)?,
    )
    .await?;
    Ok(Json(session.map(map_session).transpose()?))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(session_id): Path<i64>,
) -> V1Result<Json<PackSessionResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let session = repo::packing::packing_session(
        &state.db,
        &user.tenant,
        PackSessionQuery {
            session_id: session_id_value(session_id)?,
        },
    )
    .await?;
    Ok(Json(map_session(session)?))
}

pub async fn open(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(order_id): Path<i64>,
    Json(body): Json<OpenPackSessionRequest>,
) -> V1Result<Json<OpenPackSessionResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = open_command(order_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::packing::open_session(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_open(result)?))
}

pub async fn create_carton(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(session_id): Path<i64>,
    Json(body): Json<CreateCartonRequest>,
) -> V1Result<Json<CreateCartonResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = create_carton_command(session_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::packing::create_carton(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_created_carton(result)?))
}

pub async fn pack_content(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((session_id, carton_id)): Path<(i64, i64)>,
    Json(body): Json<PackPickedAllocationRequest>,
) -> V1Result<Json<PackPickedAllocationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = pack_command(session_id, carton_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::packing::pack_picked_allocation(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_packed_allocation(result)?))
}

pub async fn close_carton(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((session_id, carton_id)): Path<(i64, i64)>,
    Json(body): Json<CloseCartonRequest>,
) -> V1Result<Json<CloseCartonResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = close_carton_command(session_id, carton_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::packing::close_carton(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_closed_carton(result)?))
}

pub async fn void_carton(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((session_id, carton_id)): Path<(i64, i64)>,
    Json(body): Json<VoidCartonRequest>,
) -> V1Result<Json<VoidCartonResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = void_carton_command(session_id, carton_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::packing::void_carton(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_voided_carton(result)?))
}

fn open_command(order_id: i64, body: OpenPackSessionRequest) -> V1Result<OpenPackSessionCommand> {
    Ok(OpenPackSessionCommand {
        order_id: order_id_value(order_id)?,
        facility_id: FacilityId::new(body.facility_id).map_err(domain_validation)?,
        station_location_id: LocationId::new(body.station_location_id)
            .map_err(domain_validation)?,
        expected_revision: order_revision(body.expected_revision)?,
    })
}

fn create_carton_command(
    session_id: i64,
    body: CreateCartonRequest,
) -> V1Result<CreateCartonCommand> {
    Ok(CreateCartonCommand {
        session_id: session_id_value(session_id)?,
        carton_barcode: scan(body.carton_barcode, "carton barcode")?,
        expected_revision: order_revision(body.expected_revision)?,
    })
}

fn pack_command(
    session_id: i64,
    carton_id: i64,
    body: PackPickedAllocationRequest,
) -> V1Result<PackPickedAllocationCommand> {
    Ok(PackPickedAllocationCommand {
        session_id: session_id_value(session_id)?,
        carton_id: carton_id_value(carton_id)?,
        inventory_allocation_id: InventoryAllocationId::new(body.inventory_allocation_id)
            .map_err(domain_validation)?,
        item_barcode: scan(body.item_barcode, "item barcode")?,
        lot_scan: body.lot_scan.map(|value| scan(value, "lot")).transpose()?,
        serial_scan: body
            .serial_scan
            .map(|value| scan(value, "serial"))
            .transpose()?,
        source_license_plate_barcode: scan(
            body.source_license_plate_barcode,
            "source license plate barcode",
        )?,
        carton_barcode: scan(body.carton_barcode, "carton barcode")?,
        expected_revision: order_revision(body.expected_revision)?,
    })
}

fn close_carton_command(
    session_id: i64,
    carton_id: i64,
    body: CloseCartonRequest,
) -> V1Result<CloseCartonCommand> {
    Ok(CloseCartonCommand {
        session_id: session_id_value(session_id)?,
        carton_id: carton_id_value(carton_id)?,
        carton_barcode: scan(body.carton_barcode, "carton barcode")?,
        measurements: measurements_from_api(body.measurements)?,
        expected_revision: order_revision(body.expected_revision)?,
    })
}

fn void_carton_command(
    session_id: i64,
    carton_id: i64,
    body: VoidCartonRequest,
) -> V1Result<VoidCartonCommand> {
    Ok(VoidCartonCommand {
        session_id: session_id_value(session_id)?,
        carton_id: carton_id_value(carton_id)?,
        carton_barcode: scan(body.carton_barcode, "carton barcode")?,
        expected_revision: order_revision(body.expected_revision)?,
    })
}

fn map_open(result: OpenPackSessionResult) -> V1Result<OpenPackSessionResponse> {
    Ok(OpenPackSessionResponse {
        session: map_session(result.session)?,
    })
}

fn map_queue_entry(
    entry: repo::packing::PackingQueueEntry,
) -> AppResult<PackingQueueEntryResponse> {
    let status = match entry.order_status.as_str() {
        "awaiting packing" => PackingQueueOrderStatus::AwaitingPacking,
        "packing" => PackingQueueOrderStatus::Packing,
        _ => {
            return Err(AppError::internal(
                "packing queue returned an invalid order status",
            ))
        }
    };
    let session = entry
        .session
        .map(|session| {
            let status = match session.state.as_str() {
                "open" => ApiSessionStatus::Open,
                "ready_to_manifest" => ApiSessionStatus::ReadyToManifest,
                _ => {
                    return Err(AppError::internal(
                        "packing queue returned an invalid session state",
                    ))
                }
            };
            Ok(PackingQueueSessionResponse {
                session_id: session.session_id.get(),
                station_location_id: session.station_location_id,
                station_location_barcode: session.station_location_barcode,
                station_location_name: session.station_location_name,
                status,
                started_at: session.started_at.to_rfc3339(),
            })
        })
        .transpose()?;
    Ok(PackingQueueEntryResponse {
        order_id: entry.order_id.get(),
        order_key: entry.order_key,
        inventory_owner_id: entry.inventory_owner_id,
        inventory_owner_name: entry.inventory_owner_name,
        facility_id: entry.facility_id,
        facility_name: entry.facility_name,
        status,
        revision: Revision::new(entry.revision.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        rush: entry.rush,
        ship_by: entry.ship_by.map(|value| value.to_rfc3339()),
        session,
    })
}

fn decode_queue_cursor(cursor: &OpaqueCursor) -> V1Result<repo::packing::PackingQueueCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(QUEUE_CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("packing queue"))?;
    let parts = encoded.split('.').collect::<Vec<_>>();
    if parts.len() != 4 {
        return Err(V1Error::invalid_cursor_for("packing queue"));
    }
    let facility_id = match parts[0] {
        "a" => None,
        encoded if encoded.len() == 16 => Some(
            i64::from_str_radix(encoded, 16)
                .ok()
                .filter(|id| *id > 0)
                .ok_or_else(|| V1Error::invalid_cursor_for("packing queue"))?,
        ),
        _ => return Err(V1Error::invalid_cursor_for("packing queue")),
    };
    let rush_rank = match parts[1] {
        "0" => 0,
        "1" => 1,
        _ => return Err(V1Error::invalid_cursor_for("packing queue")),
    };
    let ship_by = match parts[2] {
        "n" => None,
        encoded if encoded.len() == 17 && encoded.starts_with('t') => {
            let sortable = u64::from_str_radix(&encoded[1..], 16)
                .map_err(|_| V1Error::invalid_cursor_for("packing queue"))?;
            let micros = (sortable ^ (1_u64 << 63)) as i64;
            Some(
                chrono::DateTime::<chrono::Utc>::from_timestamp_micros(micros)
                    .ok_or_else(|| V1Error::invalid_cursor_for("packing queue"))?,
            )
        }
        _ => return Err(V1Error::invalid_cursor_for("packing queue")),
    };
    if parts[3].len() != 16 {
        return Err(V1Error::invalid_cursor_for("packing queue"));
    }
    let order_id = i64::from_str_radix(parts[3], 16)
        .ok()
        .and_then(|id| OrderId::new(id).ok())
        .ok_or_else(|| V1Error::invalid_cursor_for("packing queue"))?;
    Ok(repo::packing::PackingQueueCursor {
        facility_id,
        rush_rank,
        ship_by,
        order_id,
    })
}

fn encode_queue_cursor(cursor: repo::packing::PackingQueueCursor) -> AppResult<OpaqueCursor> {
    if !matches!(cursor.rush_rank, 0 | 1) {
        return Err(AppError::internal(
            "generated an invalid packing queue cursor",
        ));
    }
    if cursor.facility_id.is_some_and(|id| id <= 0) {
        return Err(AppError::internal(
            "generated an invalid packing queue cursor",
        ));
    }
    let ship_by = cursor.ship_by.map_or_else(
        || "n".to_owned(),
        |value| {
            let sortable = (value.timestamp_micros() as u64) ^ (1_u64 << 63);
            format!("t{sortable:016x}")
        },
    );
    let facility_id = cursor
        .facility_id
        .map_or_else(|| "a".to_owned(), |id| format!("{id:016x}"));
    OpaqueCursor::new(format!(
        "{QUEUE_CURSOR_PREFIX}{}.{}.{}.{:016x}",
        facility_id,
        cursor.rush_rank,
        ship_by,
        cursor.order_id.get()
    ))
    .map_err(|_| AppError::internal("generated an invalid packing queue cursor"))
}

fn map_session(session: PackSessionReadModel) -> V1Result<PackSessionResponse> {
    Ok(PackSessionResponse {
        session_id: session.session_id.get(),
        order_id: session.order_id.get(),
        inventory_owner_id: session.inventory_owner_id.get(),
        facility_id: session.facility_id.get(),
        station_location_id: session.station_location_id.get(),
        station_location_barcode: session.station_location_barcode.into_inner(),
        station_location_name: session.station_location_name,
        order_key: session.order_key,
        revision: revision(session.revision)?,
        progress: map_progress(session.progress),
        cartons: session
            .cartons
            .into_iter()
            .map(map_carton)
            .collect::<V1Result<Vec<_>>>()?,
        allocations: session
            .allocations
            .into_iter()
            .map(map_allocation)
            .collect(),
        started_by: session.started_by.get(),
        started_at: session.started_at.to_rfc3339(),
    })
}

fn map_created_carton(result: CreateCartonResult) -> V1Result<CreateCartonResponse> {
    Ok(CreateCartonResponse {
        session_id: result.session_id.get(),
        order_id: result.order_id.get(),
        carton: map_carton(result.carton)?,
        revision: revision(result.revision)?,
        progress: map_progress(result.progress),
    })
}

fn map_packed_allocation(
    result: PackPickedAllocationResult,
) -> V1Result<PackPickedAllocationResponse> {
    Ok(PackPickedAllocationResponse {
        content_id: result.content_id.get(),
        session_id: result.session_id.get(),
        carton_id: result.carton_id.get(),
        order_id: result.order_id.get(),
        order_line_id: result.order_line_id.get(),
        inventory_allocation_id: result.inventory_allocation_id.get(),
        inventory_transaction_id: result.inventory_transaction_id,
        source_inventory_allocation_id: result.source_inventory_allocation_id.get(),
        destination_inventory_allocation_id: result.destination_inventory_allocation_id.get(),
        source_inventory_balance_id: result.source_inventory_balance_id.get(),
        destination_inventory_balance_id: result.destination_inventory_balance_id.get(),
        source_location_id: result.source_location_id.get(),
        destination_location_id: result.destination_location_id.get(),
        source_license_plate_id: result.source_license_plate_id.get(),
        destination_license_plate_id: result.destination_license_plate_id.get(),
        item_batch_id: result.item_batch_id.get(),
        item_id: result.item_id,
        quantity: result.quantity.get(),
        uom: result.uom,
        packed_by: result.packed_by.get(),
        packed_at: result.packed_at.to_rfc3339(),
        revision: revision(result.revision)?,
        progress: map_progress(result.progress),
    })
}

fn map_closed_carton(result: CloseCartonResult) -> V1Result<CloseCartonResponse> {
    let order_status = match result.order_status {
        OrderStatus::Packing => PackingOrderStatus::Packing,
        OrderStatus::AwaitingShipment => PackingOrderStatus::AwaitingShipment,
        _ => {
            return Err(V1Error::internal(
                "carton close produced an invalid order status",
            ))
        }
    };
    Ok(CloseCartonResponse {
        session_id: result.session_id.get(),
        carton_id: result.carton_id.get(),
        order_id: result.order_id.get(),
        lifecycle: map_carton_lifecycle(result.lifecycle)?,
        order_status,
        revision: revision(result.revision)?,
        progress: map_progress(result.progress),
        ready_to_manifest: result.ready_to_manifest(),
    })
}

fn map_voided_carton(result: VoidCartonResult) -> V1Result<VoidCartonResponse> {
    Ok(VoidCartonResponse {
        session_id: result.session_id.get(),
        carton_id: result.carton_id.get(),
        order_id: result.order_id.get(),
        lifecycle: map_carton_lifecycle(result.lifecycle)?,
        revision: revision(result.revision)?,
        progress: map_progress(result.progress),
    })
}

fn map_carton(carton: PackCarton) -> V1Result<PackCartonResponse> {
    Ok(PackCartonResponse {
        carton_id: carton.carton_id.get(),
        carton_barcode: carton.carton_barcode.into_inner(),
        lifecycle: map_carton_lifecycle(carton.lifecycle)?,
        content_count: carton.content_count,
        created_by: carton.created_by.get(),
        created_at: carton.created_at.to_rfc3339(),
    })
}

fn map_carton_lifecycle(lifecycle: PackCartonLifecycle) -> V1Result<PackCartonLifecycleResponse> {
    Ok(match lifecycle {
        PackCartonLifecycle::Open => PackCartonLifecycleResponse::Open,
        PackCartonLifecycle::Closed {
            measurements,
            closed_by,
            closed_at,
        } => PackCartonLifecycleResponse::Closed {
            measurements: measurements_to_api(measurements)?,
            closed_by: closed_by.get(),
            closed_at: closed_at.to_rfc3339(),
        },
        PackCartonLifecycle::Voided {
            voided_by,
            voided_at,
        } => PackCartonLifecycleResponse::Voided {
            voided_by: voided_by.get(),
            voided_at: voided_at.to_rfc3339(),
        },
    })
}

fn map_allocation(allocation: PackableAllocation) -> PackableAllocationResponse {
    PackableAllocationResponse {
        inventory_allocation_id: allocation.inventory_allocation_id.get(),
        order_line_id: allocation.order_line_id.get(),
        inventory_balance_id: allocation.inventory_balance_id.get(),
        source_location_id: allocation.source_location_id.get(),
        source_location_barcode: allocation.source_location_barcode.into_inner(),
        source_location_name: allocation.source_location_name,
        license_plate_id: allocation.license_plate_id.get(),
        license_plate_barcode: allocation.license_plate_barcode.into_inner(),
        item_batch_id: allocation.item_batch_id.get(),
        item_id: allocation.item_id,
        item_description: allocation.item_description,
        item_barcodes: allocation
            .item_barcodes
            .into_iter()
            .map(PackScanValue::into_inner)
            .collect(),
        uom: allocation.uom,
        lot: allocation.lot,
        serial: allocation.serial,
        expiration: allocation.expiration.map(|value| value.to_rfc3339()),
        quantity: allocation.quantity.get(),
        disposition: match allocation.disposition {
            PackAllocationDisposition::Available => PackAllocationDispositionResponse::Available,
            PackAllocationDisposition::Packed {
                content_id,
                carton_id,
                packed_by,
                packed_at,
            } => PackAllocationDispositionResponse::Packed {
                content_id: content_id.get(),
                carton_id: carton_id.get(),
                packed_by: packed_by.get(),
                packed_at: packed_at.to_rfc3339(),
            },
        },
    }
}

fn map_progress(progress: PackingProgress) -> PackingProgressResponse {
    PackingProgressResponse {
        expected_allocation_count: progress.expected_allocation_count(),
        packed_allocation_count: progress.packed_allocation_count(),
        expected_quantity: progress.expected_quantity(),
        packed_quantity: progress.packed_quantity(),
        open_carton_count: progress.open_carton_count(),
        closed_carton_count: progress.closed_carton_count(),
        status: match progress.status() {
            PackSessionStatus::Open => ApiSessionStatus::Open,
            PackSessionStatus::ReadyToManifest => ApiSessionStatus::ReadyToManifest,
        },
    }
}

fn measurements_from_api(value: ApiCartonMeasurements) -> V1Result<CartonMeasurements> {
    let weight = value
        .weight_grams
        .map(|weight| WeightGrams::new(weight.get()).map_err(domain_validation))
        .transpose()?;
    let dimensions = value
        .dimensions
        .map(|dimensions| {
            Ok::<_, V1Error>(CartonDimensions::new(
                DimensionMillimeters::new(dimensions.length_mm.get()).map_err(domain_validation)?,
                DimensionMillimeters::new(dimensions.width_mm.get()).map_err(domain_validation)?,
                DimensionMillimeters::new(dimensions.height_mm.get()).map_err(domain_validation)?,
            ))
        })
        .transpose()?;
    Ok(CartonMeasurements::new(weight, dimensions))
}

fn measurements_to_api(value: CartonMeasurements) -> V1Result<ApiCartonMeasurements> {
    Ok(ApiCartonMeasurements {
        weight_grams: value
            .weight_grams()
            .map(|weight| {
                ApiWeightGrams::new(weight.get())
                    .map_err(|error| V1Error::internal(error.to_string()))
            })
            .transpose()?,
        dimensions: value
            .dimensions()
            .map(|dimensions| {
                Ok::<_, V1Error>(ApiCartonDimensions {
                    length_mm: ApiDimensionMillimeters::new(dimensions.length_mm().get())
                        .map_err(|error| V1Error::internal(error.to_string()))?,
                    width_mm: ApiDimensionMillimeters::new(dimensions.width_mm().get())
                        .map_err(|error| V1Error::internal(error.to_string()))?,
                    height_mm: ApiDimensionMillimeters::new(dimensions.height_mm().get())
                        .map_err(|error| V1Error::internal(error.to_string()))?,
                })
            })
            .transpose()?,
    })
}

fn order_id_value(value: i64) -> V1Result<OrderId> {
    OrderId::new(value).map_err(domain_validation)
}

fn session_id_value(value: i64) -> V1Result<PackSessionId> {
    PackSessionId::new(value).map_err(domain_validation)
}

fn carton_id_value(value: i64) -> V1Result<CartonId> {
    CartonId::new(value).map_err(domain_validation)
}

fn order_revision(value: Revision) -> V1Result<OrderRevision> {
    OrderRevision::new(value.get()).map_err(domain_validation)
}

fn revision(value: OrderRevision) -> V1Result<Revision> {
    Revision::new(value.get()).map_err(|error| V1Error::internal(error.to_string()))
}

fn scan(value: String, label: &str) -> V1Result<PackScanValue> {
    PackScanValue::new(value).map_err(|error| {
        AppError::bad_request(format!(
            "invalid {label}: {error}; maximum length is {MAX_PACK_SCAN_VALUE_LENGTH}"
        ))
        .into()
    })
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision_value(value: i64) -> Revision {
        Revision::new(value).unwrap()
    }

    #[test]
    fn packing_queue_cursor_round_trips_priority_ship_by_and_identity() {
        let expected = repo::packing::PackingQueueCursor {
            facility_id: Some(9),
            rush_rank: 0,
            ship_by: Some("2026-08-09T16:00:00Z".parse().unwrap()),
            order_id: OrderId::new(41).unwrap(),
        };
        let encoded = encode_queue_cursor(expected.clone()).unwrap();
        let decoded = decode_queue_cursor(&encoded).unwrap();
        assert_eq!(decoded, expected);

        let without_ship_by = repo::packing::PackingQueueCursor {
            facility_id: None,
            rush_rank: 1,
            ship_by: None,
            order_id: OrderId::new(42).unwrap(),
        };
        let encoded = encode_queue_cursor(without_ship_by.clone()).unwrap();
        assert_eq!(decode_queue_cursor(&encoded).unwrap(), without_ship_by);
    }

    #[test]
    fn packing_queue_cursor_rejects_other_resources_and_malformed_values() {
        assert!(decode_queue_cursor(&OpaqueCursor::new("ib1.0000000000000029").unwrap()).is_err());
        assert!(
            decode_queue_cursor(&OpaqueCursor::new("pq1.a.2.n.0000000000000029").unwrap()).is_err()
        );
        assert!(decode_queue_cursor(
            &OpaqueCursor::new("pq1.a.0.tnot-a-timestamp.0000000000000029").unwrap()
        )
        .is_err());
    }

    #[test]
    fn open_command_uses_path_identity_and_validated_dimensions() {
        let command = open_command(
            7,
            OpenPackSessionRequest {
                facility_id: 8,
                station_location_id: 9,
                expected_revision: revision_value(3),
            },
        )
        .unwrap();
        assert_eq!(command.order_id.get(), 7);
        assert_eq!(command.facility_id.get(), 8);
        assert_eq!(command.station_location_id.get(), 9);
        assert_eq!(command.expected_revision.get(), 3);

        assert!(open_command(
            0,
            OpenPackSessionRequest {
                facility_id: 8,
                station_location_id: 9,
                expected_revision: revision_value(3),
            }
        )
        .is_err());
    }

    #[test]
    fn carton_mutations_validate_path_ids_scans_and_revisions() {
        let created = create_carton_command(
            10,
            CreateCartonRequest {
                carton_barcode: "CARTON-1".into(),
                expected_revision: revision_value(4),
            },
        )
        .unwrap();
        assert_eq!(created.session_id.get(), 10);
        assert_eq!(created.carton_barcode.as_str(), "CARTON-1");

        let voided = void_carton_command(
            10,
            11,
            VoidCartonRequest {
                carton_barcode: "CARTON-EMPTY".into(),
                expected_revision: revision_value(5),
            },
        )
        .unwrap();
        assert_eq!(voided.carton_id.get(), 11);
        assert_eq!(voided.carton_barcode.as_str(), "CARTON-EMPTY");
        assert!(void_carton_command(
            10,
            0,
            VoidCartonRequest {
                carton_barcode: " CARTON-EMPTY".into(),
                expected_revision: revision_value(5),
            }
        )
        .is_err());

        let packed = pack_command(
            10,
            11,
            PackPickedAllocationRequest {
                inventory_allocation_id: 12,
                item_barcode: "SKU-1".into(),
                lot_scan: Some("LOT-1".into()),
                serial_scan: Some("SERIAL-1".into()),
                source_license_plate_barcode: "TOTE-1".into(),
                carton_barcode: "CARTON-1".into(),
                expected_revision: revision_value(5),
            },
        )
        .unwrap();
        assert_eq!(packed.inventory_allocation_id.get(), 12);
        assert_eq!(packed.carton_id.get(), 11);
        assert_eq!(
            packed.lot_scan.as_ref().map(PackScanValue::as_str),
            Some("LOT-1")
        );
        assert_eq!(
            packed.serial_scan.as_ref().map(PackScanValue::as_str),
            Some("SERIAL-1")
        );
        assert!(pack_command(
            10,
            11,
            PackPickedAllocationRequest {
                inventory_allocation_id: 12,
                item_barcode: "SKU-1".into(),
                lot_scan: Some(" LOT-1".into()),
                serial_scan: Some("SERIAL-1".into()),
                source_license_plate_barcode: "TOTE-1".into(),
                carton_barcode: "CARTON-1".into(),
                expected_revision: revision_value(5),
            }
        )
        .is_err());
        assert!(pack_command(
            10,
            11,
            PackPickedAllocationRequest {
                inventory_allocation_id: 12,
                item_barcode: "SKU-1".into(),
                lot_scan: Some("LOT-1".into()),
                serial_scan: Some("SERIAL-1 ".into()),
                source_license_plate_barcode: "TOTE-1".into(),
                carton_barcode: "CARTON-1".into(),
                expected_revision: revision_value(5),
            }
        )
        .is_err());
    }

    #[test]
    fn close_mapping_preserves_optional_metric_measurements() {
        let command = close_carton_command(
            10,
            11,
            CloseCartonRequest {
                carton_barcode: "CARTON-1".into(),
                measurements: ApiCartonMeasurements {
                    weight_grams: Some(ApiWeightGrams::new(1_250).unwrap()),
                    dimensions: Some(ApiCartonDimensions {
                        length_mm: ApiDimensionMillimeters::new(300).unwrap(),
                        width_mm: ApiDimensionMillimeters::new(200).unwrap(),
                        height_mm: ApiDimensionMillimeters::new(150).unwrap(),
                    }),
                },
                expected_revision: revision_value(6),
            },
        )
        .unwrap();
        assert_eq!(
            command.measurements.weight_grams().map(WeightGrams::get),
            Some(1_250)
        );
        assert_eq!(
            command
                .measurements
                .dimensions()
                .map(|dimensions| dimensions.length_mm().get()),
            Some(300)
        );
    }

    #[test]
    fn progress_mapping_derives_manifest_readiness_from_domain_state() {
        let response = map_progress(PackingProgress::new(2, 2, 8, 8, 0, 1).unwrap());
        assert_eq!(response.status, ApiSessionStatus::ReadyToManifest);
        assert_eq!(response.expected_quantity, 8);
        assert_eq!(response.packed_quantity, 8);
    }

    #[test]
    fn packed_response_preserves_complete_inventory_movement_provenance() {
        use wareboxes_domain::{
            CartonContentId, InventoryBalanceId, ItemBatchId, LicensePlateId, OrderLineId,
            PackQuantity, UserId,
        };

        let response = map_packed_allocation(PackPickedAllocationResult {
            content_id: CartonContentId::new(1).unwrap(),
            session_id: PackSessionId::new(2).unwrap(),
            carton_id: CartonId::new(3).unwrap(),
            order_id: OrderId::new(4).unwrap(),
            order_line_id: OrderLineId::new(5).unwrap(),
            inventory_allocation_id: InventoryAllocationId::new(6).unwrap(),
            inventory_transaction_id: 7,
            source_inventory_allocation_id: InventoryAllocationId::new(6).unwrap(),
            destination_inventory_allocation_id: InventoryAllocationId::new(8).unwrap(),
            source_inventory_balance_id: InventoryBalanceId::new(9).unwrap(),
            destination_inventory_balance_id: InventoryBalanceId::new(10).unwrap(),
            source_location_id: LocationId::new(11).unwrap(),
            destination_location_id: LocationId::new(12).unwrap(),
            source_license_plate_id: LicensePlateId::new(13).unwrap(),
            destination_license_plate_id: LicensePlateId::new(14).unwrap(),
            item_batch_id: ItemBatchId::new(15).unwrap(),
            item_id: 16,
            quantity: PackQuantity::new(4).unwrap(),
            uom: "each".into(),
            packed_by: UserId::new(17).unwrap(),
            packed_at: "2026-08-08T20:00:00Z".parse().unwrap(),
            revision: OrderRevision::new(18).unwrap(),
            progress: PackingProgress::new(1, 1, 4, 4, 1, 0).unwrap(),
        })
        .unwrap();

        assert_eq!(response.inventory_transaction_id, 7);
        assert_eq!(response.source_inventory_allocation_id, 6);
        assert_eq!(response.destination_inventory_allocation_id, 8);
        assert_eq!(response.source_inventory_balance_id, 9);
        assert_eq!(response.destination_inventory_balance_id, 10);
        assert_eq!(response.source_location_id, 11);
        assert_eq!(response.destination_location_id, 12);
        assert_eq!(response.source_license_plate_id, 13);
        assert_eq!(response.destination_license_plate_id, 14);
    }
}
