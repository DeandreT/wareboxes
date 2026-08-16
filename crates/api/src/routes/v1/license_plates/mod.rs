use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ChangeLicensePlateParentRequest, ChangeLicensePlateParentResponse,
    LicensePlateHierarchyAction as ContractAction, LicensePlateHierarchyEventResponse,
    LicensePlateHierarchyNodeResponse, LicensePlateHierarchyResponse,
    MAX_LICENSE_PLATE_HIERARCHY_REASON_LENGTH,
};
use wareboxes_application::license_plate::{
    ChangeLicensePlateParentCommand, ChangeLicensePlateParentResult, LicensePlateHierarchyAction,
    LicensePlateHierarchyEventReadModel, LicensePlateHierarchyNodeReadModel,
    LicensePlateHierarchyReadModel,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";

pub async fn hierarchy(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(license_plate_id): Path<i64>,
) -> V1Result<Json<LicensePlateHierarchyResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(license_plate_id, "license plate ID")?;
    let model = repo::license_plates::hierarchy(&state.db, &user.tenant, license_plate_id).await?;
    Ok(Json(map_hierarchy(model)))
}

pub async fn change_parent(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(license_plate_id): Path<i64>,
    Json(body): Json<ChangeLicensePlateParentRequest>,
) -> V1Result<Json<ChangeLicensePlateParentResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    validate_change(license_plate_id, &body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::license_plates::change_parent(
        &state.db,
        &user.tenant,
        &context,
        &ChangeLicensePlateParentCommand {
            license_plate_id,
            parent_license_plate_id: body.parent_license_plate_id,
            expected_revision: body.expected_revision,
            reason: body.reason,
        },
    )
    .await?;
    Ok(Json(map_change_result(result)))
}

fn validate_change(license_plate_id: i64, body: &ChangeLicensePlateParentRequest) -> V1Result<()> {
    require_positive(license_plate_id, "license plate ID")?;
    if let Some(parent_id) = body.parent_license_plate_id {
        require_positive(parent_id, "parent license plate ID")?;
        if parent_id == license_plate_id {
            return Err(invalid("a license plate cannot contain itself"));
        }
    }
    if body.expected_revision < 0 {
        return Err(invalid("expected_revision must be nonnegative"));
    }
    if body.reason.trim() != body.reason || body.reason.is_empty() {
        return Err(invalid("reason must be trimmed and nonempty"));
    }
    if body.reason.chars().count() > MAX_LICENSE_PLATE_HIERARCHY_REASON_LENGTH {
        return Err(invalid(format!(
            "reason cannot exceed {MAX_LICENSE_PLATE_HIERARCHY_REASON_LENGTH} characters"
        )));
    }
    Ok(())
}

fn require_positive(value: i64, label: &str) -> V1Result<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(invalid(format!("{label} must be positive")))
    }
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}

fn map_change_result(result: ChangeLicensePlateParentResult) -> ChangeLicensePlateParentResponse {
    ChangeLicensePlateParentResponse {
        license_plate_id: result.license_plate_id,
        previous_parent_license_plate_id: result.previous_parent_license_plate_id,
        parent_license_plate_id: result.parent_license_plate_id,
        root_license_plate_id: result.root_license_plate_id,
        depth: result.depth,
        resulting_revision: result.resulting_revision,
        changed_at: result.changed_at.to_rfc3339(),
        changed_by_user_id: result.changed_by.get(),
    }
}

fn map_node(node: LicensePlateHierarchyNodeReadModel) -> LicensePlateHierarchyNodeResponse {
    LicensePlateHierarchyNodeResponse {
        license_plate_id: node.license_plate_id,
        barcode: node.barcode,
        inventory_owner_id: node.inventory_owner_id.get(),
        facility_id: node.facility_id.get(),
        location_id: node.location_id,
        parent_license_plate_id: node.parent_license_plate_id,
        root_license_plate_id: node.root_license_plate_id,
        depth: node.depth,
        hierarchy_revision: node.hierarchy_revision,
        direct_child_ids: node.direct_child_ids,
        descendant_ids: node.descendant_ids,
        direct_unit_quantity: node.direct_unit_quantity,
        contained_unit_quantity: node.contained_unit_quantity,
        hierarchy_updated_at: node
            .hierarchy_updated_at
            .map(|timestamp| timestamp.to_rfc3339()),
        hierarchy_updated_by_user_id: node.hierarchy_updated_by.map(|actor| actor.get()),
    }
}

fn map_event(event: LicensePlateHierarchyEventReadModel) -> LicensePlateHierarchyEventResponse {
    LicensePlateHierarchyEventResponse {
        event_id: event.event_id,
        child_license_plate_id: event.child_license_plate_id,
        previous_parent_license_plate_id: event.previous_parent_license_plate_id,
        parent_license_plate_id: event.parent_license_plate_id,
        resulting_revision: event.resulting_revision,
        action: match event.action {
            LicensePlateHierarchyAction::Attached => ContractAction::Attached,
            LicensePlateHierarchyAction::Detached => ContractAction::Detached,
        },
        actor_user_id: event.actor_id.get(),
        occurred_at: event.occurred_at.to_rfc3339(),
        reason: event.reason,
    }
}

fn map_hierarchy(model: LicensePlateHierarchyReadModel) -> LicensePlateHierarchyResponse {
    LicensePlateHierarchyResponse {
        node: map_node(model.node),
        ancestors: model.ancestors.into_iter().map(map_node).collect(),
        descendants: model.descendants.into_iter().map(map_node).collect(),
        events: model.events.into_iter().map(map_event).collect(),
    }
}
