use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use wareboxes_api_contract::v1::{
    CorrectIntegrationOrderRequest, CorrectIntegrationOrderResponse, CreateFulfillmentOrderRequest,
    IntegrationOrderEnvelopeRequest, IntegrationOrderIntakeResponse,
    IntegrationOrderProcessingStatus, ReprocessIntegrationOrderRequest,
    ReprocessIntegrationOrderResponse, Revision,
};
use wareboxes_application::integration::{
    CorrectIntegrationOrderCommand, IntegrationInboxReceipt, IntegrationOrderEnvelope,
    IntegrationOrderEnvelopeLine, IntegrationOrderProcessingResult, NewIntegrationInboxReceipt,
    ReprocessIntegrationOrderCommand,
};
use wareboxes_application::{ApplicationError, CommandContext};
use wareboxes_domain::{
    ExternalItemKey, ExternalItemUom, IntegrationInboxCorrectionReason,
    IntegrationInboxProcessingRevision, IntegrationInboxProcessingStatus, IntegrationSourceKey,
    InventoryOwnerId, OrderKey, OrderLineKey, OrderQuantity, ShippingDestination,
    ShippingRecipient,
};
use wareboxes_persistence_postgres::integration_inbox;

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

pub async fn receive_order(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((source_key, inventory_owner_id)): Path<(String, i64)>,
    headers: HeaderMap,
    body: Bytes,
) -> V1Result<(StatusCode, Json<IntegrationOrderIntakeResponse>)> {
    user.require_permission(&state.db, PERMISSION).await?;
    let inventory_owner_id = user.require_inventory_owner(inventory_owner_id)?;
    let source_key = IntegrationSourceKey::new(source_key)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let content_type = json_content_type(&headers)?;
    let request_id = current_request_id_or_new();
    let received = integration_inbox::receive(
        &state.db,
        &NewIntegrationInboxReceipt {
            tenant_id: user.tenant.tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: None,
            source_key: source_key.as_str(),
            deduplication_key: idempotency_key.as_str(),
            content_type,
            raw_payload: &body,
            request_id: Some(&request_id),
        },
    )
    .await
    .map_err(AppError::from)?;
    let context = user.command_context(&idempotency_key);
    let input = repo::integration_order_intake::ProcessingInput::retained(&received.receipt)?;
    let result = process_payload(
        &state,
        &user,
        &context,
        &received.receipt,
        &received.receipt.raw_payload,
        input,
        inventory_owner_id,
        None,
        None,
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

    let request = match serde_json::from_slice::<IntegrationOrderEnvelopeRequest>(input_payload) {
        Ok(request) => request,
        Err(_) => {
            return repo::integration_order_intake::quarantine(
                &state.db,
                &user.tenant,
                context,
                processing_request,
                repo::integration_order_intake::QuarantineReason {
                    code: INVALID_PAYLOAD_CODE,
                    message: "payload is not a valid integration order envelope v1 JSON document",
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
