use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    CartonReopenReason as ApiCartonReopenReason, PackingOrderStatus, ReopenCartonRequest,
    ReopenCartonResponse,
};
use wareboxes_application::packing::{ReopenCartonCommand, ReopenCartonResult};
use wareboxes_domain::{CartonReopenDetails, CartonReopenNote, CartonReopenReason, OrderStatus};

use super::{
    carton_id_value, domain_validation, map_carton_lifecycle, map_progress, map_weight_evidence,
    measurements_to_api, order_revision, revision, scan, session_id_value, PERMISSION,
};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::routes::v1::error::V1Result;
use crate::state::AppState;

pub async fn reopen_carton(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path((session_id, carton_id)): Path<(i64, i64)>,
    Json(body): Json<ReopenCartonRequest>,
) -> V1Result<Json<ReopenCartonResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = reopen_carton_command(session_id, carton_id, body)?;
    let context = user.command_context(&idempotency_key);
    let result =
        repo::packing::reopen_carton_command(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_result(result)?))
}

pub(super) fn reopen_carton_command(
    session_id: i64,
    carton_id: i64,
    body: ReopenCartonRequest,
) -> V1Result<ReopenCartonCommand> {
    let reason = match body.reason {
        ApiCartonReopenReason::PackingCorrection => CartonReopenReason::PackingCorrection,
        ApiCartonReopenReason::QualityIssue => CartonReopenReason::QualityIssue,
        ApiCartonReopenReason::OrderCancellation => CartonReopenReason::OrderCancellation,
        ApiCartonReopenReason::Other => CartonReopenReason::Other,
    };
    let note = body
        .note
        .map(CartonReopenNote::new)
        .transpose()
        .map_err(domain_validation)?;
    Ok(ReopenCartonCommand {
        session_id: session_id_value(session_id)?,
        carton_id: carton_id_value(carton_id)?,
        carton_barcode: scan(body.carton_barcode, "carton barcode")?,
        expected_revision: order_revision(body.expected_revision)?,
        details: CartonReopenDetails::new(reason, note).map_err(domain_validation)?,
    })
}

fn map_result(result: ReopenCartonResult) -> V1Result<ReopenCartonResponse> {
    let reason = match result.details.reason() {
        CartonReopenReason::PackingCorrection => ApiCartonReopenReason::PackingCorrection,
        CartonReopenReason::QualityIssue => ApiCartonReopenReason::QualityIssue,
        CartonReopenReason::OrderCancellation => ApiCartonReopenReason::OrderCancellation,
        CartonReopenReason::Other => ApiCartonReopenReason::Other,
    };
    Ok(ReopenCartonResponse {
        reopening_id: result.reopening_id.get(),
        session_id: result.session_id.get(),
        carton_id: result.carton_id.get(),
        order_id: result.order_id.get(),
        previous_order_status: map_order_status(result.previous_order_status)?,
        order_status: map_order_status(result.order_status)?,
        lifecycle: map_carton_lifecycle(result.lifecycle)?,
        previous_measurements: measurements_to_api(result.previous_measurements)?,
        previous_weight_evidence: result
            .previous_weight_evidence
            .map(map_weight_evidence)
            .transpose()?,
        previous_closed_by: result.previous_closed_by.get(),
        previous_closed_at: result.previous_closed_at.to_rfc3339(),
        revision: revision(result.revision)?,
        progress: map_progress(result.progress),
        reason,
        note: result.details.note().map(|value| value.as_str().to_owned()),
        reopened_by: result.reopened_by.get(),
        reopened_at: result.reopened_at.to_rfc3339(),
    })
}

fn map_order_status(status: OrderStatus) -> V1Result<PackingOrderStatus> {
    match status {
        OrderStatus::Packing => Ok(PackingOrderStatus::Packing),
        OrderStatus::AwaitingShipment => Ok(PackingOrderStatus::AwaitingShipment),
        _ => Err(AppError::internal("carton reopening returned an invalid order status").into()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use wareboxes_api_contract::v1::Revision;

    #[test]
    fn reopening_command_validates_scan_reason_and_note() {
        let command = reopen_carton_command(
            1,
            2,
            ReopenCartonRequest {
                carton_barcode: "CARTON-1".to_owned(),
                reason: ApiCartonReopenReason::PackingCorrection,
                note: None,
                expected_revision: Revision::new(3).unwrap(),
            },
        )
        .unwrap();
        assert_eq!(command.session_id.get(), 1);
        assert_eq!(command.carton_id.get(), 2);
        assert_eq!(command.expected_revision.get(), 3);

        assert!(reopen_carton_command(
            1,
            2,
            ReopenCartonRequest {
                carton_barcode: "CARTON-1".to_owned(),
                reason: ApiCartonReopenReason::Other,
                note: None,
                expected_revision: Revision::new(3).unwrap(),
            },
        )
        .is_err());
    }

    #[test]
    fn reopening_request_rejects_unknown_fields() {
        assert!(serde_json::from_value::<ReopenCartonRequest>(json!({
            "carton_barcode": "CARTON-1",
            "reason": "quality_issue",
            "expected_revision": 3,
            "unexpected": true
        }))
        .is_err());
    }
}
