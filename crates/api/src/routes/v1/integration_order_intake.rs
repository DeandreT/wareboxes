use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use wareboxes_api_contract::v1::{
    CreateFulfillmentOrderRequest, IntegrationOrderIntakeResponse,
    IntegrationOrderProcessingStatus, ReprocessIntegrationOrderRequest,
    ReprocessIntegrationOrderResponse, Revision,
};
use wareboxes_application::integration::{
    IntegrationInboxReceipt, IntegrationOrderProcessingResult, NewIntegrationInboxReceipt,
    ReprocessIntegrationOrderCommand,
};
use wareboxes_application::{ApplicationError, CommandContext};
use wareboxes_domain::{
    IntegrationInboxProcessingRevision, IntegrationInboxProcessingStatus, InventoryOwnerId,
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
const OWNER_MISMATCH_CODE: &str = "inventory_owner_mismatch";
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
    let content_type = json_content_type(&headers)?;
    let request_id = current_request_id_or_new();
    let received = integration_inbox::receive(
        &state.db,
        &NewIntegrationInboxReceipt {
            tenant_id: user.tenant.tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: None,
            source_key: &source_key,
            deduplication_key: idempotency_key.as_str(),
            content_type,
            raw_payload: &body,
            request_id: Some(&request_id),
        },
    )
    .await
    .map_err(AppError::from)?;
    let context = user.command_context(&idempotency_key);
    let result = process_payload(
        &state,
        &user,
        &context,
        &received.receipt,
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
    let receipt = repo::integration_order_intake::receipt_for_reprocessing(
        &state.db,
        &user.tenant,
        receipt_id,
    )
    .await?
    .ok_or_else(|| AppError::not_found("integration inbox receipt"))?;
    let inventory_owner_id = receipt
        .inventory_owner_id
        .ok_or_else(|| AppError::not_found("integration inbox receipt"))?;
    let context = user.command_context(&idempotency_key);
    let result = process_payload(
        &state,
        &user,
        &context,
        &receipt,
        inventory_owner_id,
        Some(revision),
        Some(&command),
    )
    .await?;
    Ok(Json(response(result)?))
}

#[allow(clippy::too_many_arguments)]
async fn process_payload(
    state: &AppState,
    user: &CurrentTenant,
    context: &CommandContext,
    receipt: &IntegrationInboxReceipt,
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

    let request =
        match serde_json::from_slice::<CreateFulfillmentOrderRequest>(&receipt.raw_payload) {
            Ok(request) => request,
            Err(_) => {
                return repo::integration_order_intake::quarantine(
                    &state.db,
                    &user.tenant,
                    context,
                    receipt,
                    expected_revision,
                    repo::integration_order_intake::QuarantineReason {
                        code: INVALID_PAYLOAD_CODE,
                        message: "payload is not a valid fulfillment order v1 JSON document",
                    },
                    reprocess,
                )
                .await;
            }
        };
    if request.inventory_owner_id != expected_owner_id.get() {
        return repo::integration_order_intake::quarantine(
            &state.db,
            &user.tenant,
            context,
            receipt,
            expected_revision,
            repo::integration_order_intake::QuarantineReason {
                code: OWNER_MISMATCH_CODE,
                message: "payload inventory owner does not match the intake endpoint scope",
            },
            reprocess,
        )
        .await;
    }
    let order = match orders::new_fulfillment_order(request) {
        Ok(order) => order,
        Err(_) => {
            return repo::integration_order_intake::quarantine(
                &state.db,
                &user.tenant,
                context,
                receipt,
                expected_revision,
                repo::integration_order_intake::QuarantineReason {
                    code: MAPPING_VALIDATION_CODE,
                    message: "payload values do not satisfy the fulfillment order v1 mapping",
                },
                reprocess,
            )
            .await;
        }
    };
    match repo::integration_order_intake::process(
        &state.db,
        &user.tenant,
        context,
        receipt,
        &order,
        expected_revision,
        reprocess,
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
                receipt,
                expected_revision,
                repo::integration_order_intake::QuarantineReason {
                    code: BUSINESS_REJECTION_CODE,
                    message: &message,
                },
                reprocess,
            )
            .await
        }
    }
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
