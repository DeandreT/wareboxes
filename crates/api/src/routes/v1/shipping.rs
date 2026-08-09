use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, Response};
use axum::Json;
use wareboxes_api_contract::v1::{
    CancelShipmentRequest, CancelShipmentResponse, ConfirmShipmentDepartureRequest,
    ConfirmShipmentDepartureResponse, CreateShipmentRequest, CreateShipmentResponse,
    GenerateCartonLabelSetRequest, GenerateCartonLabelSetResponse, GeneratePackingSlipRequest,
    GeneratePackingSlipResponse, ManualCarrierManifestResponse, RecordManualManifestRequest,
    RecordManualManifestResponse, Revision,
    ShipmentCancellationReason as ApiShipmentCancellationReason, ShipmentCancellationResponse,
    ShipmentCartonResponse, ShipmentCartonTrackingResponse, ShipmentDemandResponse,
    ShipmentDepartureProgressResponse, ShipmentDocumentListResponse, ShipmentDocumentResponse,
    ShipmentDocumentType as ApiShipmentDocumentType, ShipmentOrderStatus, ShipmentResponse,
    ShipmentStatus as ApiShipmentStatus,
};
use wareboxes_application::shipping::{
    CancelShipmentCommand, CancelShipmentResult, ConfirmShipmentDepartureCommand,
    ConfirmShipmentDepartureResult, CreateShipmentCommand, CreateShipmentResult,
    GenerateCartonLabelSetCommand, GeneratePackingSlipCommand, ManualCarrierManifestReadModel,
    RecordManualManifestCommand, RecordManualManifestResult, ShipmentDocumentContentQuery,
    ShipmentDocumentListQuery, ShipmentDocumentReadModel, ShipmentQuery, ShipmentReadModel,
};
use wareboxes_domain::{
    CarrierCode, CarrierServiceCode, CartonId, CartonTrackingAssignment, ManifestReference,
    OrderId, OrderRevision, OrderStatus, PackSessionId, ShipmentCancellationDetails,
    ShipmentCancellationNote, ShipmentCancellationReason, ShipmentDocumentId, ShipmentDocumentType,
    ShipmentId, ShipmentRevision, ShipmentScanValue, ShipmentStatus, TrackingNumber,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(order_id): Path<i64>,
    Json(body): Json<CreateShipmentRequest>,
) -> V1Result<Json<CreateShipmentResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = CreateShipmentCommand {
        order_id: positive(order_id, OrderId::new, "order ID")?,
        packing_session_id: positive(
            body.packing_session_id,
            PackSessionId::new,
            "packing session ID",
        )?,
        expected_revision: order_revision(body.expected_revision)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::shipping::create_shipment(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_create(result)?))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(shipment_id): Path<i64>,
) -> V1Result<Json<ShipmentResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let shipment = repo::shipping::get_shipment(
        &state.db,
        &user.tenant,
        ShipmentQuery {
            shipment_id: positive(shipment_id, ShipmentId::new, "shipment ID")?,
        },
    )
    .await?;
    Ok(Json(map_shipment(shipment)?))
}

pub async fn record_manifest(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(shipment_id): Path<i64>,
    Json(body): Json<RecordManualManifestRequest>,
) -> V1Result<Json<RecordManualManifestResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = RecordManualManifestCommand {
        shipment_id: positive(shipment_id, ShipmentId::new, "shipment ID")?,
        carrier_code: CarrierCode::new(body.carrier_code).map_err(domain_validation)?,
        service_code: body
            .service_code
            .map(CarrierServiceCode::new)
            .transpose()
            .map_err(domain_validation)?,
        manifest_reference: ManifestReference::new(body.manifest_reference)
            .map_err(domain_validation)?,
        carton_tracking_assignments: body
            .carton_tracking_assignments
            .into_iter()
            .map(|assignment| {
                Ok(CartonTrackingAssignment::new(
                    positive(assignment.carton_id, CartonId::new, "carton ID")?,
                    TrackingNumber::new(assignment.tracking_number).map_err(domain_validation)?,
                ))
            })
            .collect::<V1Result<Vec<_>>>()?,
        expected_revision: shipment_revision(body.expected_revision)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::shipping::record_manual_manifest(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_manifest_result(result)?))
}

pub async fn cancel(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(shipment_id): Path<i64>,
    Json(body): Json<CancelShipmentRequest>,
) -> V1Result<Json<CancelShipmentResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let note = body
        .note
        .map(ShipmentCancellationNote::new)
        .transpose()
        .map_err(domain_validation)?;
    let command = CancelShipmentCommand {
        shipment_id: positive(shipment_id, ShipmentId::new, "shipment ID")?,
        expected_shipment_revision: shipment_revision(body.expected_shipment_revision)?,
        expected_order_revision: order_revision(body.expected_order_revision)?,
        details: ShipmentCancellationDetails::new(map_cancellation_reason(body.reason), note)
            .map_err(domain_validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::shipping::cancel_shipment(&state.db, &user.tenant, &context, &command).await?;
    map_cancellation(result).map(Json)
}

pub async fn confirm_departure(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(shipment_id): Path<i64>,
    Json(body): Json<ConfirmShipmentDepartureRequest>,
) -> V1Result<Json<ConfirmShipmentDepartureResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ConfirmShipmentDepartureCommand {
        shipment_id: positive(shipment_id, ShipmentId::new, "shipment ID")?,
        scanned_carton_barcodes: body
            .scanned_carton_barcodes
            .into_iter()
            .map(ShipmentScanValue::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(domain_validation)?,
        expected_shipment_revision: shipment_revision(body.expected_shipment_revision)?,
        expected_order_revision: order_revision(body.expected_order_revision)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::shipping::confirm_departure(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_departure(result)?))
}

pub async fn generate_packing_slip(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(shipment_id): Path<i64>,
    Json(body): Json<GeneratePackingSlipRequest>,
) -> V1Result<Json<GeneratePackingSlipResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = GeneratePackingSlipCommand {
        shipment_id: positive(shipment_id, ShipmentId::new, "shipment ID")?,
        expected_revision: shipment_revision(body.expected_shipment_revision)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::shipping::generate_packing_slip(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(GeneratePackingSlipResponse {
        document: map_document(result.document)?,
    }))
}

pub async fn generate_carton_label_set(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(shipment_id): Path<i64>,
    Json(body): Json<GenerateCartonLabelSetRequest>,
) -> V1Result<Json<GenerateCartonLabelSetResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = GenerateCartonLabelSetCommand {
        shipment_id: positive(shipment_id, ShipmentId::new, "shipment ID")?,
        expected_revision: shipment_revision(body.expected_shipment_revision)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::shipping::generate_carton_label_set(&state.db, &user.tenant, &context, &command)
            .await?;
    Ok(Json(GenerateCartonLabelSetResponse {
        document: map_document(result.document)?,
    }))
}

pub async fn list_documents(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(shipment_id): Path<i64>,
) -> V1Result<Json<ShipmentDocumentListResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let documents = repo::shipping::list_documents(
        &state.db,
        &user.tenant,
        ShipmentDocumentListQuery {
            shipment_id: positive(shipment_id, ShipmentId::new, "shipment ID")?,
        },
    )
    .await?;
    Ok(Json(ShipmentDocumentListResponse {
        documents: documents
            .into_iter()
            .map(map_document)
            .collect::<V1Result<Vec<_>>>()?,
    }))
}

pub async fn download_document(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(document_id): Path<i64>,
) -> V1Result<Response<Body>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let result = repo::shipping::get_document_content(
        &state.db,
        &user.tenant,
        ShipmentDocumentContentQuery {
            document_id: positive(document_id, ShipmentDocumentId::new, "shipment document ID")?,
        },
    )
    .await?;
    let disposition = format!("attachment; filename=\"{}\"", result.document.file_name);
    let mut response = Response::new(Body::from(result.content));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&result.document.media_type)
            .map_err(|_| V1Error::internal("shipment document media type is invalid"))?,
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .map_err(|_| V1Error::internal("shipment document file name is invalid"))?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&result.document.content_length.to_string())
            .map_err(|_| V1Error::internal("shipment document length is invalid"))?,
    );
    Ok(response)
}

fn map_create(result: CreateShipmentResult) -> V1Result<CreateShipmentResponse> {
    Ok(CreateShipmentResponse {
        shipment: map_shipment(result.shipment)?,
        order_status: map_order_status(result.order_status)?,
        order_revision: revision(result.order_revision.get())?,
    })
}

fn map_shipment(shipment: ShipmentReadModel) -> V1Result<ShipmentResponse> {
    Ok(ShipmentResponse {
        shipment_id: shipment.shipment_id.get(),
        attempt: shipment.attempt,
        packing_session_id: shipment.packing_session_id.get(),
        order_id: shipment.order_id.get(),
        order_key: shipment.order_key,
        inventory_owner_id: shipment.inventory_owner_id.get(),
        facility_id: shipment.facility_id.get(),
        status: map_shipment_status(shipment.status),
        revision: revision(shipment.revision.get())?,
        order_status: map_order_status(shipment.order_status)?,
        order_revision: revision(shipment.order_revision.get())?,
        demand: map_demand(shipment.demand),
        departure_progress: ShipmentDepartureProgressResponse {
            total_carton_count: shipment.departure_progress.total_carton_count,
            departed_carton_count: shipment.departure_progress.departed_carton_count,
            remaining_carton_count: shipment.departure_progress.remaining_carton_count,
            total_quantity: shipment.departure_progress.total_quantity,
            departed_quantity: shipment.departure_progress.departed_quantity,
            remaining_quantity: shipment.departure_progress.remaining_quantity,
        },
        cartons: shipment
            .cartons
            .into_iter()
            .map(|carton| ShipmentCartonResponse {
                carton_id: carton.carton_id.get(),
                carton_barcode: carton.carton_barcode.into_inner(),
                sequence: carton.sequence,
                content_count: carton.content_count,
                packed_quantity: carton.packed_quantity,
                weight_grams: carton.weight_grams,
                length_mm: carton.length_mm,
                width_mm: carton.width_mm,
                height_mm: carton.height_mm,
                tracking_assignment_id: carton
                    .tracking_assignment_id
                    .map(|assignment_id| assignment_id.get()),
                tracking_number: carton
                    .tracking_number
                    .map(wareboxes_domain::TrackingNumber::into_inner),
                departed_at: carton.departed_at.map(|timestamp| timestamp.to_rfc3339()),
            })
            .collect(),
        manifest: shipment.manifest.map(map_manifest),
        cancellation: shipment
            .cancellation
            .map(|cancellation| ShipmentCancellationResponse {
                cancellation_id: cancellation.cancellation_id.get(),
                previous_status: map_shipment_status(cancellation.previous_status),
                reason: api_cancellation_reason(cancellation.details.reason()),
                note: cancellation
                    .details
                    .note()
                    .map(|note| note.as_str().to_owned()),
                cancelled_by: cancellation.cancelled_by.get(),
                cancelled_at: cancellation.cancelled_at.to_rfc3339(),
            }),
        created_by: shipment.created_by.get(),
        created_at: shipment.created_at.to_rfc3339(),
        departed_by: shipment.departed_by.map(|user_id| user_id.get()),
        departed_at: shipment.departed_at.map(|timestamp| timestamp.to_rfc3339()),
    })
}

fn map_cancellation(result: CancelShipmentResult) -> V1Result<CancelShipmentResponse> {
    Ok(CancelShipmentResponse {
        shipment: map_shipment(result.shipment)?,
        packing_session_revision: revision(result.packing_session_revision.get())?,
    })
}

fn map_manifest_result(
    result: RecordManualManifestResult,
) -> V1Result<RecordManualManifestResponse> {
    Ok(RecordManualManifestResponse {
        shipment_id: result.shipment_id.get(),
        order_id: result.order_id.get(),
        status: map_shipment_status(result.status),
        revision: revision(result.revision.get())?,
        manifest: map_manifest(result.manifest),
    })
}

fn map_manifest(manifest: ManualCarrierManifestReadModel) -> ManualCarrierManifestResponse {
    ManualCarrierManifestResponse {
        manifest_id: manifest.manifest_id.get(),
        carrier_code: manifest.carrier_code.into_inner(),
        service_code: manifest.service_code.map(CarrierServiceCode::into_inner),
        manifest_reference: manifest.manifest_reference.into_inner(),
        carton_tracking_assignments: manifest
            .carton_tracking_assignments
            .into_iter()
            .map(|assignment| ShipmentCartonTrackingResponse {
                tracking_assignment_id: assignment.tracking_assignment_id.get(),
                carton_id: assignment.carton_id.get(),
                tracking_number: assignment.tracking_number.into_inner(),
            })
            .collect(),
        manifested_by: manifest.manifested_by.get(),
        manifested_at: manifest.manifested_at.to_rfc3339(),
    }
}

fn map_departure(
    result: ConfirmShipmentDepartureResult,
) -> V1Result<ConfirmShipmentDepartureResponse> {
    Ok(ConfirmShipmentDepartureResponse {
        shipment_id: result.shipment_id.get(),
        order_id: result.order_id.get(),
        shipment_status: map_shipment_status(result.shipment_status),
        shipment_revision: revision(result.shipment_revision.get())?,
        order_status: map_order_status(result.order_status)?,
        order_revision: revision(result.order_revision.get())?,
        scanned_carton_count: result.scanned_carton_count,
        departure_quantity: result.departure_quantity,
        cumulative_departed_quantity: result.cumulative_departed_quantity,
        remaining_quantity: result.remaining_quantity,
        remaining_carton_count: result.remaining_carton_count,
        demand: map_demand(result.demand),
        departed_by: result.departed_by.get(),
        departed_at: result.departed_at.to_rfc3339(),
    })
}

fn map_document(document: ShipmentDocumentReadModel) -> V1Result<ShipmentDocumentResponse> {
    Ok(ShipmentDocumentResponse {
        document_id: document.document_id.get(),
        shipment_id: document.shipment_id.get(),
        order_id: document.order_id.get(),
        document_type: match document.document_type {
            ShipmentDocumentType::PackingSlip => ApiShipmentDocumentType::PackingSlip,
            ShipmentDocumentType::CartonLabelSet => ApiShipmentDocumentType::CartonLabelSet,
        },
        manifest_id: document.manifest_id.map(|value| value.get()),
        carrier_code: document.carrier_code.map(|value| value.as_str().to_owned()),
        service_code: document.service_code.map(|value| value.as_str().to_owned()),
        manifest_reference: document
            .manifest_reference
            .map(|value| value.as_str().to_owned()),
        file_name: document.file_name,
        media_type: document.media_type,
        content_length: document.content_length,
        content_sha256: document.content_sha256,
        shipment_revision_at_generation: revision(document.shipment_revision_at_generation.get())?,
        carton_count: document.carton_count,
        line_count: document.line_count,
        demand: map_demand(document.demand),
        generated_by: document.generated_by.get(),
        generated_at: document.generated_at.to_rfc3339(),
    })
}

const fn map_demand(demand: wareboxes_domain::ShortShipDemandQuantities) -> ShipmentDemandResponse {
    ShipmentDemandResponse {
        ordered_quantity: demand.ordered().get(),
        shipped_quantity: demand.effective().get(),
        accepted_short_quantity: demand.accepted_short().get(),
        accepted_substitute_quantity: demand.accepted_substitute().get(),
    }
}

const fn map_shipment_status(status: ShipmentStatus) -> ApiShipmentStatus {
    match status {
        ShipmentStatus::AwaitingManifest => ApiShipmentStatus::AwaitingManifest,
        ShipmentStatus::Manifested => ApiShipmentStatus::Manifested,
        ShipmentStatus::PartiallyDeparted => ApiShipmentStatus::PartiallyDeparted,
        ShipmentStatus::Departed => ApiShipmentStatus::Departed,
        ShipmentStatus::Cancelled => ApiShipmentStatus::Cancelled,
    }
}

fn map_order_status(status: OrderStatus) -> V1Result<ShipmentOrderStatus> {
    match status {
        OrderStatus::Packing => Ok(ShipmentOrderStatus::Packing),
        OrderStatus::AwaitingShipment => Ok(ShipmentOrderStatus::AwaitingShipment),
        OrderStatus::Shipped => Ok(ShipmentOrderStatus::Shipped),
        OrderStatus::Cancelled => Ok(ShipmentOrderStatus::Cancelled),
        _ => Err(V1Error::internal(
            "shipping workflow produced an invalid order status",
        )),
    }
}

const fn map_cancellation_reason(
    reason: ApiShipmentCancellationReason,
) -> ShipmentCancellationReason {
    match reason {
        ApiShipmentCancellationReason::PackingCorrection => {
            ShipmentCancellationReason::PackingCorrection
        }
        ApiShipmentCancellationReason::ShippingDataCorrection => {
            ShipmentCancellationReason::ShippingDataCorrection
        }
        ApiShipmentCancellationReason::DuplicateShipment => {
            ShipmentCancellationReason::DuplicateShipment
        }
        ApiShipmentCancellationReason::OperatorError => ShipmentCancellationReason::OperatorError,
        ApiShipmentCancellationReason::Other => ShipmentCancellationReason::Other,
    }
}

const fn api_cancellation_reason(
    reason: ShipmentCancellationReason,
) -> ApiShipmentCancellationReason {
    match reason {
        ShipmentCancellationReason::PackingCorrection => {
            ApiShipmentCancellationReason::PackingCorrection
        }
        ShipmentCancellationReason::ShippingDataCorrection => {
            ApiShipmentCancellationReason::ShippingDataCorrection
        }
        ShipmentCancellationReason::DuplicateShipment => {
            ApiShipmentCancellationReason::DuplicateShipment
        }
        ShipmentCancellationReason::OperatorError => ApiShipmentCancellationReason::OperatorError,
        ShipmentCancellationReason::Other => ApiShipmentCancellationReason::Other,
    }
}

fn order_revision(value: Revision) -> V1Result<OrderRevision> {
    OrderRevision::new(value.get()).map_err(domain_validation)
}

fn shipment_revision(value: Revision) -> V1Result<ShipmentRevision> {
    ShipmentRevision::new(value.get()).map_err(domain_validation)
}

fn revision(value: i64) -> V1Result<Revision> {
    Revision::new(value).map_err(|_| V1Error::internal("shipping produced an invalid revision"))
}

fn positive<T, E>(
    value: i64,
    constructor: impl FnOnce(i64) -> Result<T, E>,
    field: &str,
) -> V1Result<T>
where
    E: std::fmt::Display,
{
    constructor(value).map_err(|error| invalid(format!("{field}: {error}")))
}

fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    invalid(error.to_string())
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}
