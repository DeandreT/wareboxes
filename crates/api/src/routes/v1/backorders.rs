use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    BackorderPolicyMode as ApiPolicyMode, BackorderPolicyRequest, BackorderPolicyResponse,
    BackorderReason as ApiReason, BackorderSplitLineResponse, ConfigureBackorderPolicyRequest,
    Revision, SplitOrderBackorderRequest, SplitOrderBackorderResponse,
};
use wareboxes_application::backorder::{
    BackorderPolicyReadModel, ConfigureBackorderPolicyCommand, SplitOrderBackorderCommand,
    SplitOrderBackorderResult,
};
use wareboxes_domain::{
    BackorderDetails, BackorderNote, BackorderPolicyMode, BackorderPolicyRevision, BackorderReason,
    FacilityId, InventoryOwnerId, OrderId, OrderRevision,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

pub async fn configure_policy(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ConfigureBackorderPolicyRequest>,
) -> V1Result<Json<BackorderPolicyResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = ConfigureBackorderPolicyCommand {
        inventory_owner_id: InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        mode: map_policy_mode(body.mode),
        expected_revision: body
            .expected_revision
            .map(|revision| BackorderPolicyRevision::new(revision.get()))
            .transpose()
            .map_err(validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::backorder::configure_policy(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_policy(result)?))
}

pub async fn get_policy(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<BackorderPolicyRequest>,
) -> V1Result<Json<Option<BackorderPolicyResponse>>> {
    user.require_permission(&state.db, "orders").await?;
    let result = repo::backorder::active_policy(
        &state.db,
        &user.tenant,
        InventoryOwnerId::new(query.inventory_owner_id).map_err(validation)?,
        FacilityId::new(query.facility_id).map_err(validation)?,
    )
    .await?;
    Ok(Json(result.map(map_policy).transpose()?))
}

pub async fn split_shortage(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(order_id): Path<i64>,
    Json(body): Json<SplitOrderBackorderRequest>,
) -> V1Result<Json<SplitOrderBackorderResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let note = body
        .note
        .map(BackorderNote::new)
        .transpose()
        .map_err(validation)?;
    let details = BackorderDetails::new(map_reason(body.reason), note).map_err(validation)?;
    let command = SplitOrderBackorderCommand {
        order_id: OrderId::new(order_id).map_err(validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        expected_order_revision: OrderRevision::new(body.expected_order_revision.get())
            .map_err(validation)?,
        expected_policy_revision: BackorderPolicyRevision::new(body.expected_policy_revision.get())
            .map_err(validation)?,
        details,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::backorder::split_shortage(&state.db, &user.tenant, &context, &command).await?;
    Ok(Json(map_split(result)?))
}

fn map_policy(value: BackorderPolicyReadModel) -> V1Result<BackorderPolicyResponse> {
    Ok(BackorderPolicyResponse {
        policy_id: value.policy_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        facility_id: value.facility_id.get(),
        mode: match value.mode {
            BackorderPolicyMode::Block => ApiPolicyMode::Block,
            BackorderPolicyMode::SplitShortage => ApiPolicyMode::SplitShortage,
        },
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
    })
}

fn map_split(result: SplitOrderBackorderResult) -> V1Result<SplitOrderBackorderResponse> {
    if !result.quantities_are_consistent() {
        return Err(V1Error::internal(
            "backorder split result does not conserve demand",
        ));
    }
    Ok(SplitOrderBackorderResponse {
        split_id: result.split_id.get(),
        policy_id: result.policy_id.get(),
        policy_revision: Revision::new(result.policy_revision.get()).map_err(invalid_result)?,
        inventory_owner_id: result.inventory_owner_id.get(),
        facility_id: result.facility_id.get(),
        parent_order_id: result.parent_order_id.get(),
        parent_order_key: result.parent_order_key,
        parent_revision: Revision::new(result.parent_revision.get()).map_err(invalid_result)?,
        child_order_id: result.child_order_id.get(),
        child_order_key: result.child_order_key,
        child_revision: Revision::new(result.child_revision.get()).map_err(invalid_result)?,
        original_quantity: result.original_quantity,
        allocated_quantity: result.allocated_quantity,
        previously_backordered_quantity: result.previously_backordered_quantity,
        newly_backordered_quantity: result.newly_backordered_quantity,
        parent_effective_quantity: result.parent_effective_quantity,
        lines: result
            .lines
            .into_iter()
            .map(|line| BackorderSplitLineResponse {
                parent_order_line_id: line.parent_order_line_id.get(),
                child_order_line_id: line.child_order_line_id.get(),
                line_key: line.line_key,
                item_id: line.item_id,
                uom: line.uom,
                original_quantity: line.original_quantity,
                allocated_quantity: line.allocated_quantity,
                previously_backordered_quantity: line.previously_backordered_quantity,
                newly_backordered_quantity: line.newly_backordered_quantity,
                resulting_parent_quantity: line.resulting_parent_quantity,
            })
            .collect(),
        reason: map_reason_to_api(result.details.reason),
        note: result.details.note.map(|note| note.as_str().to_owned()),
        split_by: result.split_by.get(),
        split_at: result.split_at.to_rfc3339(),
    })
}

const fn map_policy_mode(value: ApiPolicyMode) -> BackorderPolicyMode {
    match value {
        ApiPolicyMode::Block => BackorderPolicyMode::Block,
        ApiPolicyMode::SplitShortage => BackorderPolicyMode::SplitShortage,
    }
}

const fn map_reason(value: ApiReason) -> BackorderReason {
    match value {
        ApiReason::InventoryUnavailable => BackorderReason::InventoryUnavailable,
        ApiReason::ClientRequested => BackorderReason::ClientRequested,
        ApiReason::ServiceLevel => BackorderReason::ServiceLevel,
        ApiReason::Other => BackorderReason::Other,
    }
}

const fn map_reason_to_api(value: BackorderReason) -> ApiReason {
    match value {
        BackorderReason::InventoryUnavailable => ApiReason::InventoryUnavailable,
        BackorderReason::ClientRequested => ApiReason::ClientRequested,
        BackorderReason::ServiceLevel => ApiReason::ServiceLevel,
        BackorderReason::Other => ApiReason::Other,
    }
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    V1Error::internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_mappings_are_total() {
        assert_eq!(
            map_policy_mode(ApiPolicyMode::SplitShortage),
            BackorderPolicyMode::SplitShortage
        );
        assert_eq!(
            map_reason_to_api(map_reason(ApiReason::ServiceLevel)),
            ApiReason::ServiceLevel
        );
    }
}
