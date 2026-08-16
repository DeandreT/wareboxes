use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, Response};
use axum::Json;
use wareboxes_api_contract::v1::{
    AutomationCommandStatus as ApiAutomationCommandStatus,
    AutomationHealthState as ApiAutomationHealthState, CancelShipmentDocumentPrintRequest,
    CancelShipmentRequest, CancelShipmentResponse, ConfigurationScope as ApiConfigurationScope,
    ConfirmShipmentDepartureRequest, ConfirmShipmentDepartureResponse, CreateShipmentRequest,
    CreateShipmentResponse, CursorPage, DocumentPolicyExpectation as ApiDocumentPolicyExpectation,
    DocumentPolicyResponse as ApiDocumentPolicyResponse,
    DocumentPolicySource as ApiDocumentPolicySource, GenerateCartonLabelSetRequest,
    GenerateCartonLabelSetResponse, GeneratePackingSlipRequest, GeneratePackingSlipResponse,
    ManualCarrierManifestResponse, OpaqueCursor, PrintShipmentDocumentRequest,
    PrintShipmentDocumentResponse, RecordManualManifestRequest, RecordManualManifestResponse,
    Revision, ShipmentCancellationReason as ApiShipmentCancellationReason,
    ShipmentCancellationResponse, ShipmentCartonResponse, ShipmentCartonTrackingResponse,
    ShipmentDemandResponse, ShipmentDepartureProgressResponse, ShipmentDocumentListResponse,
    ShipmentDocumentPrintJobPage, ShipmentDocumentPrintJobPageRequest,
    ShipmentDocumentPrintJobResponse, ShipmentDocumentResponse,
    ShipmentDocumentType as ApiShipmentDocumentType, ShipmentOrderStatus,
    ShipmentPrinterDevicePage, ShipmentPrinterDeviceResponse, ShipmentResponse,
    ShipmentStatus as ApiShipmentStatus,
};
use wareboxes_application::automation::{AutomationCommandReadModel, AutomationCommandStatus};
use wareboxes_application::shipping::{
    CancelShipmentCommand, CancelShipmentDocumentPrintCommand, CancelShipmentResult,
    ConfirmShipmentDepartureCommand, ConfirmShipmentDepartureResult, CreateShipmentCommand,
    CreateShipmentResult, DocumentPolicyExpectation, DocumentPolicyReadModel, DocumentPolicySource,
    GenerateCartonLabelSetCommand, GeneratePackingSlipCommand, ManualCarrierManifestReadModel,
    RecordManualManifestCommand, RecordManualManifestResult, ShipmentDocumentContentQuery,
    ShipmentDocumentListQuery, ShipmentDocumentReadModel, ShipmentQuery, ShipmentReadModel,
};
use wareboxes_domain::{
    AutomationCommandId, AutomationCommandResult, AutomationDeviceCommand, AutomationDeviceId,
    AutomationHealthState, AutomationPrinterCommand, CarrierCode, CarrierServiceCode, CartonId,
    CartonTrackingAssignment, ConfigurationScope, ConfigurationVersionId, ManifestReference,
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
const PRINT_CURSOR_PREFIX: &str = "sdp1.";

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
        expected_policy: map_policy_expectation(body.expected_policy)?,
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
        expected_policy: map_policy_expectation(body.expected_policy)?,
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
    let result = repo::shipping::list_documents(
        &state.db,
        &user.tenant,
        ShipmentDocumentListQuery {
            shipment_id: positive(shipment_id, ShipmentId::new, "shipment ID")?,
        },
    )
    .await?;
    Ok(Json(ShipmentDocumentListResponse {
        policy: map_policy(result.policy),
        documents: result
            .documents
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

pub async fn list_document_printers(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(document_id): Path<i64>,
) -> V1Result<Json<ShipmentPrinterDevicePage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let printers = repo::shipping::available_printers(
        &state.db,
        &user.tenant,
        positive(document_id, ShipmentDocumentId::new, "shipment document ID")?,
    )
    .await?;
    Ok(Json(ShipmentPrinterDevicePage {
        items: printers
            .into_iter()
            .map(|printer| {
                Ok(ShipmentPrinterDeviceResponse {
                    device_id: printer.device_id.get(),
                    device_key: printer.device_key,
                    display_name: printer.display_name,
                    health: map_automation_health(printer.health),
                    last_heartbeat_at: printer
                        .last_heartbeat_at
                        .map(|time| time.to_rfc3339())
                        .ok_or_else(|| V1Error::internal("available printer lacks heartbeat"))?,
                })
            })
            .collect::<V1Result<Vec<_>>>()?,
    }))
}

pub async fn print_document(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(document_id): Path<i64>,
    Json(body): Json<PrintShipmentDocumentRequest>,
) -> V1Result<Json<PrintShipmentDocumentResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let document_id = positive(document_id, ShipmentDocumentId::new, "shipment document ID")?;
    let command = repo::shipping::print_document(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        document_id,
        positive(body.device_id, AutomationDeviceId::new, "printer device ID")?,
        body.copies,
        &body.expected_content_sha256,
    )
    .await?;
    Ok(Json(PrintShipmentDocumentResponse {
        print_job: map_print_job(command, document_id)?,
    }))
}

pub async fn list_document_print_jobs(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(document_id): Path<i64>,
    Query(query): Query<ShipmentDocumentPrintJobPageRequest>,
) -> V1Result<Json<ShipmentDocumentPrintJobPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let document_id = positive(document_id, ShipmentDocumentId::new, "shipment document ID")?;
    let before = query
        .cursor
        .as_ref()
        .map(|cursor| decode_print_cursor(cursor, document_id))
        .transpose()?;
    let page = repo::shipping::print_jobs(
        &state.db,
        &user.tenant,
        document_id,
        before,
        query.limit.get(),
    )
    .await?;
    let items = page
        .items
        .into_iter()
        .map(|command| map_print_job(command, document_id))
        .collect::<V1Result<Vec<_>>>()?;
    let next_cursor = page
        .next_command_id
        .map(|command_id| encode_print_cursor(document_id, command_id))
        .transpose()?;
    Ok(Json(CursorPage::new(items, next_cursor)))
}

pub async fn get_document_print_job(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path((document_id, command_id)): Path<(i64, i64)>,
) -> V1Result<Json<ShipmentDocumentPrintJobResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let document_id = positive(document_id, ShipmentDocumentId::new, "shipment document ID")?;
    let command = repo::shipping::print_job(
        &state.db,
        &user.tenant,
        document_id,
        positive(
            command_id,
            AutomationCommandId::new,
            "automation command ID",
        )?,
    )
    .await?;
    Ok(Json(map_print_job(command, document_id)?))
}

pub async fn cancel_document_print_job(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((document_id, command_id)): Path<(i64, i64)>,
    Json(body): Json<CancelShipmentDocumentPrintRequest>,
) -> V1Result<Json<PrintShipmentDocumentResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let document_id = positive(document_id, ShipmentDocumentId::new, "shipment document ID")?;
    let command = CancelShipmentDocumentPrintCommand {
        document_id,
        command_id: positive(
            command_id,
            AutomationCommandId::new,
            "automation command ID",
        )?,
        expected_revision: u32::try_from(body.expected_revision.get())
            .map_err(|_| invalid("automation command revision is too large"))?,
    };
    let result = repo::shipping::cancel_print_job(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(PrintShipmentDocumentResponse {
        print_job: map_print_job(result, document_id)?,
    }))
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
        policy: map_policy(document.policy),
        generated_by: document.generated_by.get(),
        generated_at: document.generated_at.to_rfc3339(),
    })
}

fn map_print_job(
    command: AutomationCommandReadModel,
    expected_document_id: ShipmentDocumentId,
) -> V1Result<ShipmentDocumentPrintJobResponse> {
    let context = command
        .shipping_document_print_context
        .ok_or_else(|| V1Error::internal("shipping print command lacks document context"))?;
    if context.document_id != expected_document_id {
        return Err(V1Error::internal(
            "shipping print command targets the wrong document",
        ));
    }
    let copies = match command.command {
        AutomationDeviceCommand::Printer(AutomationPrinterCommand::PrintDocument {
            copies,
            ..
        }) => copies,
        _ => {
            return Err(V1Error::internal(
                "shipping print command has an invalid payload",
            ))
        }
    };
    let spool_job_id = match command.result {
        Some(AutomationCommandResult::Printer(result)) => Some(result.spool_job_id),
        None => None,
        Some(_) => {
            return Err(V1Error::internal(
                "shipping print command has an invalid result",
            ))
        }
    };
    Ok(ShipmentDocumentPrintJobResponse {
        command_id: command.command_id.get(),
        document_id: context.document_id.get(),
        shipment_id: context.shipment_id.get(),
        content_sha256: context.content_sha256,
        device_id: command.device_id.get(),
        device_key: command.device_key,
        copies,
        status: map_automation_status(command.status),
        revision: revision(i64::from(command.revision))?,
        delivery_attempts: command.delivery_attempts,
        assigned_service_account_id: command.assigned_service_account_id.map(|id| id.get()),
        agent_instance: command.agent_instance,
        delivered_at: command.delivered_at.map(|time| time.to_rfc3339()),
        accepted_at: command.accepted_at.map(|time| time.to_rfc3339()),
        completed_at: command.completed_at.map(|time| time.to_rfc3339()),
        spool_job_id,
        error_code: command.error_code,
        error_message: command.error_message,
        requested_by: command.requested_by.get(),
        requested_at: command.requested_at.to_rfc3339(),
    })
}

fn map_policy_expectation(
    value: ApiDocumentPolicyExpectation,
) -> V1Result<DocumentPolicyExpectation> {
    let expectation = DocumentPolicyExpectation {
        source: match value.source {
            ApiDocumentPolicySource::ProductDefault => DocumentPolicySource::ProductDefault,
            ApiDocumentPolicySource::Configuration => DocumentPolicySource::Configuration,
        },
        configuration_id: value
            .configuration_id
            .map(|id| positive(id, ConfigurationVersionId::new, "configuration ID"))
            .transpose()?,
        configuration_revision: value.configuration_revision,
        policy_hash: value.policy_hash,
    };
    if expectation.is_well_formed() {
        Ok(expectation)
    } else {
        Err(AppError::bad_request("document policy expectation is invalid").into())
    }
}

fn map_policy(value: DocumentPolicyReadModel) -> ApiDocumentPolicyResponse {
    ApiDocumentPolicyResponse {
        source: match value.source {
            DocumentPolicySource::ProductDefault => ApiDocumentPolicySource::ProductDefault,
            DocumentPolicySource::Configuration => ApiDocumentPolicySource::Configuration,
        },
        configuration_id: value.configuration_id.map(|id| id.get()),
        configuration_revision: value.configuration_revision,
        configuration_scope: value.configuration_scope.map(|scope| match scope {
            ConfigurationScope::Tenant => ApiConfigurationScope::Tenant,
            ConfigurationScope::InventoryOwner { inventory_owner_id } => {
                ApiConfigurationScope::InventoryOwner {
                    inventory_owner_id: inventory_owner_id.get(),
                }
            }
            ConfigurationScope::Facility { facility_id } => ApiConfigurationScope::Facility {
                facility_id: facility_id.get(),
            },
            ConfigurationScope::OwnerFacility {
                inventory_owner_id,
                facility_id,
            } => ApiConfigurationScope::OwnerFacility {
                inventory_owner_id: inventory_owner_id.get(),
                facility_id: facility_id.get(),
            },
        }),
        generate_packing_slip: value.generate_packing_slip,
        generate_carton_label: value.generate_carton_label,
        require_tracking_barcode: value.require_tracking_barcode,
        policy_hash: value.policy_hash,
    }
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

const fn map_automation_health(value: AutomationHealthState) -> ApiAutomationHealthState {
    match value {
        AutomationHealthState::Unknown => ApiAutomationHealthState::Unknown,
        AutomationHealthState::Healthy => ApiAutomationHealthState::Healthy,
        AutomationHealthState::Degraded => ApiAutomationHealthState::Degraded,
        AutomationHealthState::Offline => ApiAutomationHealthState::Offline,
        AutomationHealthState::Faulted => ApiAutomationHealthState::Faulted,
    }
}

const fn map_automation_status(value: AutomationCommandStatus) -> ApiAutomationCommandStatus {
    match value {
        AutomationCommandStatus::Queued => ApiAutomationCommandStatus::Queued,
        AutomationCommandStatus::Delivered => ApiAutomationCommandStatus::Delivered,
        AutomationCommandStatus::Accepted => ApiAutomationCommandStatus::Accepted,
        AutomationCommandStatus::Succeeded => ApiAutomationCommandStatus::Succeeded,
        AutomationCommandStatus::Failed => ApiAutomationCommandStatus::Failed,
        AutomationCommandStatus::ManualReview => ApiAutomationCommandStatus::ManualReview,
        AutomationCommandStatus::ResolvedManually => ApiAutomationCommandStatus::ResolvedManually,
        AutomationCommandStatus::Cancelled => ApiAutomationCommandStatus::Cancelled,
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

fn encode_print_cursor(
    document_id: ShipmentDocumentId,
    command_id: AutomationCommandId,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{PRINT_CURSOR_PREFIX}{:016x}.{:016x}",
        document_id.get(),
        command_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid shipment print cursor"))
}

fn decode_print_cursor(
    cursor: &OpaqueCursor,
    document_id: ShipmentDocumentId,
) -> V1Result<AutomationCommandId> {
    let value = cursor
        .as_str()
        .strip_prefix(PRINT_CURSOR_PREFIX)
        .ok_or_else(|| invalid("shipment print cursor is invalid"))?;
    let (document, command) = value
        .split_once('.')
        .ok_or_else(|| invalid("shipment print cursor is invalid"))?;
    let cursor_document = i64::from_str_radix(document, 16)
        .ok()
        .and_then(|value| ShipmentDocumentId::new(value).ok())
        .ok_or_else(|| invalid("shipment print cursor is invalid"))?;
    if cursor_document != document_id {
        return Err(invalid("shipment print cursor does not match the document"));
    }
    i64::from_str_radix(command, 16)
        .ok()
        .and_then(|value| AutomationCommandId::new(value).ok())
        .ok_or_else(|| invalid("shipment print cursor is invalid"))
}
