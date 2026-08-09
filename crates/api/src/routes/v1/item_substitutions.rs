use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ConfigureItemSubstitutionPolicyRequest, ItemSubstitutionPolicyListRequest,
    ItemSubstitutionPolicyResponse, ItemSubstitutionReason as ApiSubstitutionReason,
    RetireItemSubstitutionPolicyRequest, Revision, SubstitutePickShortageRequest,
    SubstitutePickShortageResponse, SubstitutePickWorkResponse,
};
use wareboxes_application::item_substitution::{
    ConfigureItemSubstitutionPolicyCommand, ItemSubstitutionPolicyFilter,
    ItemSubstitutionPolicyReadModel, RetireItemSubstitutionPolicyCommand,
    SubstitutePickShortageCommand, SubstitutePickShortageResult,
};
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryOwnerId, ItemSubstitutionDefinition,
    ItemSubstitutionDetails, ItemSubstitutionNote, ItemSubstitutionPolicyId,
    ItemSubstitutionPolicyRevision, ItemSubstitutionReason, OrderRevision, PickShortageId,
    PickShortageRevision, SubstitutionQuantity, SubstitutionUom,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

pub async fn configure(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ConfigureItemSubstitutionPolicyRequest>,
) -> V1Result<Json<ItemSubstitutionPolicyResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let definition = ItemSubstitutionDefinition::new(
        CatalogItemId::new(body.source_item_id).map_err(validation)?,
        SubstitutionUom::new(body.source_uom).map_err(validation)?,
        CatalogItemId::new(body.substitute_item_id).map_err(validation)?,
        SubstitutionUom::new(body.substitute_uom).map_err(validation)?,
        SubstitutionQuantity::new(body.source_quantity).map_err(validation)?,
        SubstitutionQuantity::new(body.substitute_quantity).map_err(validation)?,
    )
    .map_err(validation)?;
    let command = ConfigureItemSubstitutionPolicyCommand {
        inventory_owner_id: InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        definition,
        expected_revision: body
            .expected_revision
            .map(|value| ItemSubstitutionPolicyRevision::new(value.get()))
            .transpose()
            .map_err(validation)?,
    };
    let result = repo::item_substitution::configure_policy(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_policy(result)?))
}

pub async fn retire(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(policy_id): Path<i64>,
    Json(body): Json<RetireItemSubstitutionPolicyRequest>,
) -> V1Result<Json<ItemSubstitutionPolicyResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = RetireItemSubstitutionPolicyCommand {
        policy_id: ItemSubstitutionPolicyId::new(policy_id).map_err(validation)?,
        expected_revision: ItemSubstitutionPolicyRevision::new(body.expected_revision.get())
            .map_err(validation)?,
    };
    let result = repo::item_substitution::retire_policy(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_policy(result)?))
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<ItemSubstitutionPolicyListRequest>,
) -> V1Result<Json<Vec<ItemSubstitutionPolicyResponse>>> {
    user.require_permission(&state.db, "orders").await?;
    if query.source_item_id.is_some_and(|value| value <= 0) {
        return Err(AppError::bad_request("source_item_id must be positive").into());
    }
    let filter = ItemSubstitutionPolicyFilter {
        inventory_owner_id: InventoryOwnerId::new(query.inventory_owner_id).map_err(validation)?,
        facility_id: FacilityId::new(query.facility_id).map_err(validation)?,
        source_item_id: query.source_item_id,
        active_only: query.active_only,
    };
    let result = repo::item_substitution::list_policies(&state.db, &user.tenant, &filter).await?;
    Ok(Json(
        result
            .into_iter()
            .map(map_policy)
            .collect::<V1Result<Vec<_>>>()?,
    ))
}

pub async fn substitute_shortage(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(shortage_id): Path<i64>,
    Json(body): Json<SubstitutePickShortageRequest>,
) -> V1Result<Json<SubstitutePickShortageResponse>> {
    user.require_permission(&state.db, "wms_supervisor").await?;
    let command = SubstitutePickShortageCommand {
        shortage_id: PickShortageId::new(shortage_id).map_err(validation)?,
        policy_id: ItemSubstitutionPolicyId::new(body.policy_id).map_err(validation)?,
        expected_policy_revision: ItemSubstitutionPolicyRevision::new(
            body.expected_policy_revision.get(),
        )
        .map_err(validation)?,
        expected_shortage_revision: PickShortageRevision::new(
            body.expected_shortage_revision.get(),
        )
        .map_err(validation)?,
        expected_order_revision: OrderRevision::new(body.expected_order_revision.get())
            .map_err(validation)?,
        details: ItemSubstitutionDetails::new(
            map_reason(body.reason),
            body.note
                .map(ItemSubstitutionNote::new)
                .transpose()
                .map_err(validation)?,
        )
        .map_err(validation)?,
    };
    let result = repo::item_substitution::substitute_shortage(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_substitution(result)?))
}

fn map_policy(value: ItemSubstitutionPolicyReadModel) -> V1Result<ItemSubstitutionPolicyResponse> {
    Ok(ItemSubstitutionPolicyResponse {
        policy_id: value.policy_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        facility_id: value.facility_id.get(),
        source_item_id: value.definition.source_item_id.get(),
        source_uom: value.definition.source_uom.to_string(),
        substitute_item_id: value.definition.substitute_item_id.get(),
        substitute_uom: value.definition.substitute_uom.to_string(),
        source_quantity: value.definition.source_quantity.get(),
        substitute_quantity: value.definition.substitute_quantity.get(),
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        active: value.active,
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
        retired_by: value.retired_by.map(|id| id.get()),
        retired_at: value.retired_at.map(|value| value.to_rfc3339()),
    })
}

fn map_substitution(
    value: SubstitutePickShortageResult,
) -> V1Result<SubstitutePickShortageResponse> {
    Ok(SubstitutePickShortageResponse {
        substitution_id: value.substitution_id.get(),
        shortage_id: value.shortage_id.get(),
        shortage_revision: Revision::new(value.shortage_revision.get()).map_err(invalid_result)?,
        policy_id: value.policy_id.get(),
        policy_revision: Revision::new(value.policy_revision.get()).map_err(invalid_result)?,
        inventory_owner_id: value.inventory_owner_id.get(),
        facility_id: value.facility_id.get(),
        order_id: value.order_id.get(),
        order_revision: Revision::new(value.order_revision.get()).map_err(invalid_result)?,
        source_order_line_id: value.source_order_line_id.get(),
        substitute_order_line_id: value.substitute_order_line_id.get(),
        substitute_reservation_id: value.substitute_reservation_id,
        accepted_source_quantity: value.accepted_source_quantity.get(),
        substitute_quantity: value.substitute_quantity.get(),
        substitute_item_id: value.substitute_item_id,
        substitute_uom: value.substitute_uom,
        work: value
            .work
            .into_iter()
            .map(|work| SubstitutePickWorkResponse {
                task_id: work.task_id.get(),
                content_id: work.content_id.get(),
                inventory_allocation_id: work.inventory_allocation_id.get(),
                inventory_balance_id: work.inventory_balance_id.get(),
                source_location_id: work.source_location_id.get(),
                quantity: work.quantity.get(),
            })
            .collect(),
        reason: map_reason_to_api(value.details.reason),
        note: value.details.note.map(|note| note.as_str().to_owned()),
        substituted_by: value.substituted_by.get(),
        substituted_at: value.substituted_at.to_rfc3339(),
    })
}

const fn map_reason(value: ApiSubstitutionReason) -> ItemSubstitutionReason {
    match value {
        ApiSubstitutionReason::ClientAuthorized => ItemSubstitutionReason::ClientAuthorized,
        ApiSubstitutionReason::InventoryUnavailable => ItemSubstitutionReason::InventoryUnavailable,
        ApiSubstitutionReason::ServiceRecovery => ItemSubstitutionReason::ServiceRecovery,
        ApiSubstitutionReason::Other => ItemSubstitutionReason::Other,
    }
}

const fn map_reason_to_api(value: ItemSubstitutionReason) -> ApiSubstitutionReason {
    match value {
        ItemSubstitutionReason::ClientAuthorized => ApiSubstitutionReason::ClientAuthorized,
        ItemSubstitutionReason::InventoryUnavailable => ApiSubstitutionReason::InventoryUnavailable,
        ItemSubstitutionReason::ServiceRecovery => ApiSubstitutionReason::ServiceRecovery,
        ItemSubstitutionReason::Other => ApiSubstitutionReason::Other,
    }
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    V1Error::internal(format!("invalid item substitution result: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapper_preserves_conversion_and_lifecycle() {
        let model = ItemSubstitutionPolicyReadModel {
            policy_id: ItemSubstitutionPolicyId::new(1).unwrap(),
            inventory_owner_id: InventoryOwnerId::new(2).unwrap(),
            facility_id: FacilityId::new(3).unwrap(),
            definition: ItemSubstitutionDefinition::new(
                CatalogItemId::new(4).unwrap(),
                SubstitutionUom::new("case").unwrap(),
                CatalogItemId::new(5).unwrap(),
                SubstitutionUom::new("each").unwrap(),
                SubstitutionQuantity::new(1).unwrap(),
                SubstitutionQuantity::new(12).unwrap(),
            )
            .unwrap(),
            revision: ItemSubstitutionPolicyRevision::new(1).unwrap(),
            active: true,
            configured_by: wareboxes_domain::UserId::new(6).unwrap(),
            configured_at: "2026-08-09T00:00:00Z".parse().unwrap(),
            retired_by: None,
            retired_at: None,
        };
        let response = map_policy(model).unwrap();
        assert_eq!(response.substitute_quantity, 12);
        assert!(response.active);
    }
}
