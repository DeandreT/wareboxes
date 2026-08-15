use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
#[cfg(feature = "openapi")]
use wareboxes_api_contract::v1::ErrorResponse;
use wareboxes_api_contract::v1::{
    CorrectIntegrationOrderRequest, CorrectIntegrationOrderResponse, CreateFulfillmentOrderRequest,
    IntegrationOrderEnvelopeRequest, IntegrationOrderIntakeResponse,
    IntegrationOrderProcessingStatus, ReprocessIntegrationOrderRequest,
    ReprocessIntegrationOrderResponse, Revision,
};
use wareboxes_application::integration::{
    CorrectIntegrationOrderCommand, IntegrationInboxReceipt, IntegrationOrderEnvelope,
    IntegrationOrderEnvelopeLine, IntegrationOrderProcessingResult,
    ReprocessIntegrationOrderCommand,
};
use wareboxes_application::{ApplicationError, CommandContext};
use wareboxes_domain::{
    ExternalInventoryOwnerKey, ExternalItemKey, ExternalItemUom, IntegrationInboxCorrectionReason,
    IntegrationInboxProcessingRevision, IntegrationInboxProcessingStatus, IntegrationSourceKey,
    InventoryOwnerId, OrderKey, OrderLineKey, OrderQuantity, ShippingDestination,
    ShippingRecipient,
};

use super::error::{V1Error, V1Result};
use super::orders;
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::{current_request_id_or_new, IdempotencyKey};
use crate::state::AppState;

const PERMISSION: &str = "orders";
const INVALID_PAYLOAD_CODE: &str = "invalid_payload";
const MAPPING_VALIDATION_CODE: &str = "mapping_validation_failed";
const BUSINESS_REJECTION_CODE: &str = "business_rejected";

/// Submit a partner fulfillment order.
///
/// Wareboxes durably retains the original payload before mapping partner owner, item, and UOM
/// identities. A `202` response therefore reports either `processed` or `quarantined`; callers
/// must inspect `status` instead of treating every accepted receipt as a created order.
#[cfg_attr(feature = "openapi", utoipa::path(
    post,
    path = "/api/v1/integrations/order-intake/{source_key}/inventory-owners/{external_inventory_owner_key}/orders",
    operation_id = "submitIntegrationOrder",
    tag = "Orders",
    request_body(
        content = IntegrationOrderEnvelopeRequest,
        description = "Partner order envelope. The path supplies the source and inventory-owner identities; internal Wareboxes IDs are not accepted in this payload.",
        content_type = "application/json",
        example = json!({
            "order_key": "SO-1001",
            "rush": false,
            "ship_by": "2027-08-12T17:00:00Z",
            "destination": {
                "recipient_name": "Receiving Team",
                "company": "Northstar Retail",
                "phone": "+1 775 555 0100",
                "email": "receiving@example.com",
                "line1": "125 Shipping Lane",
                "line2": "Dock 4",
                "city": "Reno",
                "region": "NV",
                "postal_code": "89502",
                "country": "US"
            },
            "lines": [{
                "line_key": "1",
                "external_item_key": "CLIENT-CASE",
                "external_uom": "CS",
                "quantity": 4
            }]
        })
    ),
    params(
        (
            "source_key" = String,
            Path,
            description = "Provisioned integration source identity. Item and UOM mappings are source-specific.",
            min_length = 1,
            max_length = 200,
            example = "partner-api"
        ),
        (
            "external_inventory_owner_key" = String,
            Path,
            description = "Partner inventory-owner identity configured for this source. It is resolved within the authenticated tenant and owner scope.",
            min_length = 1,
            max_length = 200,
            example = "NORTHSTAR"
        ),
        (
            "x-wareboxes-tenant-id" = i64,
            Header,
            description = "Positive tenant context for the bearer credential. The request fails closed when the identity is not a member of this tenant.",
            minimum = 1,
            example = 12
        ),
        (
            "idempotency-key" = String,
            Header,
            description = "Caller-generated identity for this submission. An exact retry returns the original outcome; reuse with a different payload or scope returns 409.",
            min_length = 1,
            max_length = 200,
            pattern = "^[!-~]{1,200}$",
            example = "partner-order-SO-1001-v1"
        ),
        (
            "x-request-id" = Option<String>,
            Header,
            description = "Optional caller correlation ID. Wareboxes echoes a valid value or assigns one.",
            min_length = 1,
            max_length = 128,
            pattern = "^[A-Za-z0-9._:-]{1,128}$",
            example = "partner-order-SO-1001-attempt-1"
        )
    ),
    responses(
        (
            status = 202,
            description = "The payload is durably retained. Inspect `status`: `processed` includes `order_id`; `quarantined` includes `error_code` and `error_message` for operator remediation.",
            body = IntegrationOrderIntakeResponse,
            headers(("x-request-id" = String, description = "Request correlation ID.")),
            examples(
                (
                    "processed" = (
                        summary = "Fulfillment demand created",
                        value = json!({
                            "receipt_id": 501,
                            "processing_id": 601,
                            "processing_attempt_id": 701,
                            "correction_id": null,
                            "input_payload_sha256": "4cacc15b0023683e11cc4c371c585f8aefe1a12221edeb64290fbe35be4e4ccd",
                            "inventory_owner_id": 42,
                            "adapter_key": "wareboxes.fulfillment_order",
                            "mapping_version": 2,
                            "status": "processed",
                            "revision": 1,
                            "attempt_count": 1,
                            "applied_mapping_count": 1,
                            "order_id": 9001,
                            "order_revision": 1,
                            "error_code": null,
                            "error_message": null,
                            "attempted_by": 7,
                            "attempted_at": "2026-08-11T19:30:00Z",
                            "processed_at": "2026-08-11T19:30:00Z"
                        })
                    )
                ),
                (
                    "quarantined" = (
                        summary = "Document retained for mapping remediation",
                        value = json!({
                            "receipt_id": 502,
                            "processing_id": 602,
                            "processing_attempt_id": 702,
                            "correction_id": null,
                            "input_payload_sha256": "4cacc15b0023683e11cc4c371c585f8aefe1a12221edeb64290fbe35be4e4ccd",
                            "inventory_owner_id": 42,
                            "adapter_key": "wareboxes.fulfillment_order",
                            "mapping_version": 2,
                            "status": "quarantined",
                            "revision": 1,
                            "attempt_count": 1,
                            "applied_mapping_count": 0,
                            "order_id": null,
                            "order_revision": null,
                            "error_code": "item_mapping_not_found",
                            "error_message": "no active item mapping for line 1 (CLIENT-CASE / CS)",
                            "attempted_by": 7,
                            "attempted_at": "2026-08-11T19:30:00Z",
                            "processed_at": null
                        })
                    )
                )
            )
        ),
        (status = 400, description = "A required header or path value is missing or invalid.", body = ErrorResponse),
        (status = 401, description = "The bearer credential is missing or invalid.", body = ErrorResponse),
        (status = 403, description = "The identity lacks tenant membership or the orders permission.", body = ErrorResponse),
        (status = 404, description = "No active inventory-owner mapping is visible for the source, external owner key, and caller owner scope.", body = ErrorResponse),
        (status = 409, description = "The idempotency key was reused with a different payload, content type, or scope.", body = ErrorResponse),
        (status = 413, description = "The request exceeds the deployment request-body limit.", body = ErrorResponse),
        (status = 415, description = "Content-Type is not application/json or a +json media type.", body = ErrorResponse),
        (status = 500, description = "Wareboxes could not safely retain or process the submission.", body = ErrorResponse)
    ),
    security(("bearerAuth" = []))
))]
pub async fn receive_order(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((source_key, external_inventory_owner_key)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> V1Result<(StatusCode, Json<IntegrationOrderIntakeResponse>)> {
    user.require_permission(&state.db, PERMISSION).await?;
    let content_type = json_content_type(&headers)?;
    receive_external_order(
        &state,
        &user,
        &idempotency_key,
        source_key,
        external_inventory_owner_key,
        content_type,
        &body,
        repo::integration_order_intake::JSON_ORDER_ADAPTER,
    )
    .await
}

/// Submit an X12 940 Warehouse Shipping Order using the Wareboxes v1 profile.
#[cfg_attr(feature = "openapi", utoipa::path(
    post,
    path = "/api/v1/integrations/x12-940/{source_key}/inventory-owners/{external_inventory_owner_key}/orders",
    operation_id = "submitX12940Order",
    tag = "Orders",
    request_body(
        content = String,
        content_type = "application/edi-x12",
        description = "One X12 004010 940 transaction in an ISA/IEA interchange. The Wareboxes v1 profile accepts W0501=N, an N1/N3/N4 ship-to loop, optional N9*RU and G62*10, and LX/W01 lines with SK or VP item keys."
    ),
    params(
        ("source_key" = String, Path, min_length = 1, max_length = 200, example = "partner-edi"),
        ("external_inventory_owner_key" = String, Path, min_length = 1, max_length = 200, example = "NORTHSTAR"),
        ("x-wareboxes-tenant-id" = i64, Header, minimum = 1, example = 12),
        ("idempotency-key" = String, Header, min_length = 1, max_length = 200, example = "x12-isa-000000001-st-0001"),
        ("x-request-id" = Option<String>, Header, min_length = 1, max_length = 128)
    ),
    responses(
        (status = 202, description = "The raw interchange is retained and either processed or quarantined.", body = IntegrationOrderIntakeResponse),
        (status = 400, description = "A header or path value is invalid.", body = ErrorResponse),
        (status = 401, description = "The bearer credential is missing or invalid.", body = ErrorResponse),
        (status = 403, description = "The identity lacks tenant membership or order permission.", body = ErrorResponse),
        (status = 404, description = "No visible active owner mapping exists.", body = ErrorResponse),
        (status = 409, description = "The idempotency identity conflicts with a prior payload.", body = ErrorResponse),
        (status = 413, description = "The interchange exceeds the deployment body limit.", body = ErrorResponse),
        (status = 415, description = "Content-Type is not application/edi-x12.", body = ErrorResponse),
        (status = 500, description = "The submission could not be retained safely.", body = ErrorResponse)
    ),
    security(("bearerAuth" = []))
))]
pub async fn receive_x12_940_order(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((source_key, external_inventory_owner_key)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> V1Result<(StatusCode, Json<IntegrationOrderIntakeResponse>)> {
    user.require_permission(&state.db, PERMISSION).await?;
    let content_type = x12_content_type(&headers)?;
    receive_external_order(
        &state,
        &user,
        &idempotency_key,
        source_key,
        external_inventory_owner_key,
        content_type,
        &body,
        repo::integration_order_intake::X12_940_ORDER_ADAPTER,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn receive_external_order(
    state: &AppState,
    user: &CurrentTenant,
    idempotency_key: &IdempotencyKey,
    source_key: String,
    external_inventory_owner_key: String,
    content_type: &str,
    body: &[u8],
    adapter: repo::integration_order_intake::AdapterDescriptor,
) -> V1Result<(StatusCode, Json<IntegrationOrderIntakeResponse>)> {
    let source_key = IntegrationSourceKey::new(source_key)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let external_inventory_owner_key = ExternalInventoryOwnerKey::new(external_inventory_owner_key)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let request_id = current_request_id_or_new();
    let received = repo::integration_order_intake::receive_external_order(
        &state.db,
        &user.tenant,
        user.tenant.user_id,
        repo::integration_order_intake::ExternalOrderReceipt {
            source_key: &source_key,
            external_inventory_owner_key: &external_inventory_owner_key,
            deduplication_key: idempotency_key.as_str(),
            content_type,
            raw_payload: body,
            request_id: &request_id,
        },
    )
    .await?;
    let inventory_owner_id = received
        .receipt
        .inventory_owner_id
        .ok_or_else(|| AppError::internal("mapped order receipt has no inventory owner"))?;
    let context = user.command_context(idempotency_key);
    let input = repo::integration_order_intake::ProcessingInput::retained(&received.receipt)?;
    let result = process_payload(
        state,
        user,
        &context,
        &received.receipt,
        &received.receipt.raw_payload,
        input,
        inventory_owner_id,
        None,
        None,
        adapter,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(response(result)?)))
}

pub async fn reprocess_order(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(receipt_id): Path<i64>,
    Json(body): Json<ReprocessIntegrationOrderRequest>,
) -> V1Result<Json<ReprocessIntegrationOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let revision = IntegrationInboxProcessingRevision::new(body.expected_revision.get())
        .map_err(AppError::bad_request)?;
    let command = ReprocessIntegrationOrderCommand::new(receipt_id, revision)?;
    let envelope = repo::integration_order_intake::receipt_for_reprocessing(
        &state.db,
        &user.tenant,
        receipt_id,
    )
    .await?
    .ok_or_else(|| AppError::not_found("integration inbox receipt"))?;
    let inventory_owner_id = envelope
        .receipt
        .inventory_owner_id
        .ok_or_else(|| AppError::not_found("integration inbox receipt"))?;
    let context = user.command_context(&idempotency_key);
    let result = process_payload(
        &state,
        &user,
        &context,
        &envelope.receipt,
        &envelope.input_payload,
        repo::integration_order_intake::ProcessingInput {
            payload_sha256: envelope.input_payload_sha256,
            correction_id: envelope.correction_id,
            attempted_at: None,
        },
        inventory_owner_id,
        Some(revision),
        Some(&command),
        envelope.adapter,
    )
    .await?;
    Ok(Json(response(result)?))
}

pub async fn correct_order(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(receipt_id): Path<i64>,
    Json(body): Json<CorrectIntegrationOrderRequest>,
) -> V1Result<Json<CorrectIntegrationOrderResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let revision = IntegrationInboxProcessingRevision::new(body.expected_revision.get())
        .map_err(AppError::bad_request)?;
    let reason = IntegrationInboxCorrectionReason::new(body.reason)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let corrected_payload = serde_json::to_vec(&body.order)
        .map_err(|error| AppError::internal(format!("serializing corrected order: {error}")))?;
    let corrected_payload_sha256: [u8; 32] = Sha256::digest(&corrected_payload).into();
    let command = CorrectIntegrationOrderCommand::new(
        receipt_id,
        revision,
        reason,
        corrected_payload_sha256,
    )?;
    let envelope = repo::integration_order_intake::receipt_for_reprocessing(
        &state.db,
        &user.tenant,
        receipt_id,
    )
    .await?
    .ok_or_else(|| AppError::not_found("integration inbox receipt"))?;
    let owner_id = envelope
        .receipt
        .inventory_owner_id
        .ok_or_else(|| AppError::not_found("integration inbox receipt"))?;
    if body.order.inventory_owner_id != owner_id.get() {
        return Err(AppError::bad_request(
            "corrected order inventory owner must match the retained receipt",
        )
        .into());
    }
    let order = orders::new_fulfillment_order(body.order)?;
    let context = user.command_context(&idempotency_key);
    let correction_input = repo::integration_order_intake::CorrectionInput {
        command: &command,
        corrected_payload: &corrected_payload,
    };
    let result = match repo::integration_order_intake::correct(
        &state.db,
        &user.tenant,
        &context,
        &envelope.receipt,
        &order,
        correction_input,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            let Some(message) = quarantinable_message(&error) else {
                return Err(error.into());
            };
            repo::integration_order_intake::quarantine_correction(
                &state.db,
                &user.tenant,
                &context,
                &envelope.receipt,
                correction_input,
                repo::integration_order_intake::QuarantineReason {
                    code: BUSINESS_REJECTION_CODE,
                    message: &message,
                },
            )
            .await?
        }
    };
    Ok(Json(response(result)?))
}

#[allow(clippy::too_many_arguments)]
async fn process_payload(
    state: &AppState,
    user: &CurrentTenant,
    context: &CommandContext,
    receipt: &IntegrationInboxReceipt,
    input_payload: &[u8],
    input: repo::integration_order_intake::ProcessingInput,
    expected_owner_id: InventoryOwnerId,
    expected_revision: Option<IntegrationInboxProcessingRevision>,
    reprocess: Option<&ReprocessIntegrationOrderCommand>,
    adapter: repo::integration_order_intake::AdapterDescriptor,
) -> Result<IntegrationOrderProcessingResult, AppError> {
    if reprocess.is_none() {
        if let Some(existing) =
            repo::integration_order_intake::current_processing(&state.db, &user.tenant, receipt)
                .await?
        {
            return Ok(existing);
        }
    }
    let processing_request = repo::integration_order_intake::ProcessingRequest::new(
        receipt,
        expected_revision,
        input,
        reprocess,
        adapter,
    );

    if input.correction_id.is_some() {
        let request = match serde_json::from_slice::<CreateFulfillmentOrderRequest>(input_payload) {
            Ok(request) => request,
            Err(_) => {
                return repo::integration_order_intake::quarantine(
                    &state.db,
                    &user.tenant,
                    context,
                    processing_request,
                    repo::integration_order_intake::QuarantineReason {
                        code: INVALID_PAYLOAD_CODE,
                        message: "corrected payload is not a valid fulfillment order v1 document",
                    },
                )
                .await;
            }
        };
        if request.inventory_owner_id != expected_owner_id.get() {
            return Err(AppError::not_found("integration inbox receipt"));
        }
        let order = match orders::new_fulfillment_order(request) {
            Ok(order) => order,
            Err(_) => {
                return repo::integration_order_intake::quarantine(
                    &state.db,
                    &user.tenant,
                    context,
                    processing_request,
                    repo::integration_order_intake::QuarantineReason {
                        code: MAPPING_VALIDATION_CODE,
                        message: "corrected payload values do not satisfy the fulfillment order v1 contract",
                    },
                )
                .await;
            }
        };
        return match repo::integration_order_intake::process_internal(
            &state.db,
            &user.tenant,
            context,
            processing_request,
            &order,
        )
        .await
        {
            Ok(result) => Ok(result),
            Err(error) => {
                let Some(message) = quarantinable_message(&error) else {
                    return Err(error);
                };
                repo::integration_order_intake::quarantine(
                    &state.db,
                    &user.tenant,
                    context,
                    processing_request,
                    repo::integration_order_intake::QuarantineReason {
                        code: BUSINESS_REJECTION_CODE,
                        message: &message,
                    },
                )
                .await
            }
        };
    }

    let request = match adapter {
        repo::integration_order_intake::JSON_ORDER_ADAPTER => {
            serde_json::from_slice::<IntegrationOrderEnvelopeRequest>(input_payload).map_err(|_| ())
        }
        repo::integration_order_intake::X12_940_ORDER_ADAPTER => {
            super::x12_940::parse(input_payload).map_err(|_| ())
        }
        _ => Err(()),
    };
    let request = match request {
        Ok(request) => request,
        Err(()) => {
            return repo::integration_order_intake::quarantine(
                &state.db,
                &user.tenant,
                context,
                processing_request,
                repo::integration_order_intake::QuarantineReason {
                    code: INVALID_PAYLOAD_CODE,
                    message: "payload does not satisfy the selected order adapter contract",
                },
            )
            .await;
        }
    };
    let envelope = match integration_order_envelope(request, expected_owner_id) {
        Ok(envelope) => envelope,
        Err(_) => {
            return repo::integration_order_intake::quarantine(
                &state.db,
                &user.tenant,
                context,
                processing_request,
                repo::integration_order_intake::QuarantineReason {
                    code: MAPPING_VALIDATION_CODE,
                    message:
                        "payload values do not satisfy the integration order envelope v1 contract",
                },
            )
            .await;
        }
    };
    repo::integration_order_intake::process_external(
        &state.db,
        &user.tenant,
        context,
        processing_request,
        &envelope,
    )
    .await
}

fn integration_order_envelope(
    request: IntegrationOrderEnvelopeRequest,
    inventory_owner_id: InventoryOwnerId,
) -> Result<IntegrationOrderEnvelope, AppError> {
    let order_key = OrderKey::new(request.order_key)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let ship_by = request
        .ship_by
        .map(|value| {
            if value.trim() != value || value.is_empty() {
                return Err(AppError::bad_request(
                    "ship_by must be a nonempty RFC3339 timestamp",
                ));
            }
            DateTime::parse_from_rfc3339(&value)
                .map(|timestamp| timestamp.with_timezone(&Utc))
                .map_err(|_| AppError::bad_request("ship_by must be an RFC3339 timestamp"))
        })
        .transpose()?;
    let destination = request.destination;
    let recipient = ShippingRecipient::new(
        destination.recipient_name,
        destination.company,
        destination.phone,
        destination.email,
    )
    .map_err(|error| AppError::bad_request(error.to_string()))?;
    let destination = ShippingDestination::new(
        recipient,
        destination.line1,
        destination.line2,
        destination.city,
        destination.region,
        destination.postal_code,
        destination.country,
    )
    .map_err(|error| AppError::bad_request(error.to_string()))?;
    let lines = request
        .lines
        .into_iter()
        .map(|line| {
            Ok(IntegrationOrderEnvelopeLine {
                line_key: OrderLineKey::new(line.line_key)
                    .map_err(|error| AppError::bad_request(error.to_string()))?,
                external_item_key: ExternalItemKey::new(line.external_item_key)
                    .map_err(|error| AppError::bad_request(error.to_string()))?,
                external_uom: ExternalItemUom::new(line.external_uom)
                    .map_err(|error| AppError::bad_request(error.to_string()))?,
                quantity: OrderQuantity::new(line.quantity)
                    .map_err(|error| AppError::bad_request(error.to_string()))?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    IntegrationOrderEnvelope::new(
        inventory_owner_id,
        order_key,
        request.rush,
        ship_by,
        destination,
        lines,
    )
    .map_err(AppError::from)
}

fn quarantinable_message(error: &AppError) -> Option<String> {
    let message = match error.public_application_error() {
        ApplicationError::NotFound(resource) => format!("not found: {resource}"),
        ApplicationError::Validation(details) => details
            .into_iter()
            .map(|detail| format!("{}: {}", detail.field, detail.message))
            .collect::<Vec<_>>()
            .join("; "),
        ApplicationError::Conflict(message) | ApplicationError::InvalidRequest(message) => message,
        _ => return None,
    };
    let clean = message
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(1_000)
        .collect::<String>();
    Some(if clean.trim().is_empty() {
        "order intake was rejected by current business rules".into()
    } else {
        clean.trim().to_owned()
    })
}

fn json_content_type(headers: &HeaderMap) -> V1Result<&str> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| V1Error::unsupported_media_type("Content-Type must be application/json"))?;
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    if media_type != "application/json" && !media_type.ends_with("+json") {
        return Err(V1Error::unsupported_media_type(
            "Content-Type must be application/json",
        ));
    }
    Ok(content_type)
}

fn x12_content_type(headers: &HeaderMap) -> V1Result<&str> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            V1Error::unsupported_media_type("Content-Type must be application/edi-x12")
        })?;
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    if media_type != "application/edi-x12" {
        return Err(V1Error::unsupported_media_type(
            "Content-Type must be application/edi-x12",
        ));
    }
    Ok(content_type)
}

fn response(result: IntegrationOrderProcessingResult) -> V1Result<IntegrationOrderIntakeResponse> {
    Ok(IntegrationOrderIntakeResponse {
        receipt_id: result.receipt_id,
        processing_id: result.processing_id.get(),
        processing_attempt_id: result.processing_attempt_id.get(),
        correction_id: result.correction_id.map(|id| id.get()),
        input_payload_sha256: hex::encode(result.input_payload_sha256),
        inventory_owner_id: result.inventory_owner_id.get(),
        adapter_key: result.adapter_key,
        mapping_version: result.mapping_version,
        status: match result.status {
            IntegrationInboxProcessingStatus::Quarantined => {
                IntegrationOrderProcessingStatus::Quarantined
            }
            IntegrationInboxProcessingStatus::Processed => {
                IntegrationOrderProcessingStatus::Processed
            }
        },
        revision: Revision::new(result.revision.get())
            .map_err(|_| V1Error::internal("integration processing revision is invalid"))?,
        attempt_count: result.attempt_count,
        applied_mapping_count: result.applied_mapping_count,
        order_id: result.order_id.map(|id| id.get()),
        order_revision: result
            .order_revision
            .map(|revision| Revision::new(revision.get()))
            .transpose()
            .map_err(|_| V1Error::internal("processed order revision is invalid"))?,
        error_code: result.error_code,
        error_message: result.error_message,
        attempted_by: result.attempted_by.get(),
        attempted_at: result.attempted_at.to_rfc3339(),
        processed_at: result.processed_at.map(|value| value.to_rfc3339()),
    })
}
