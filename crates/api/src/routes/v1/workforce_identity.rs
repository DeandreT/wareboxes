use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    EmployeeIdentityChangeKind as ApiChangeKind, EmployeeIdentityChangeResponse,
    LinkEmployeeIdentityRequest, UnlinkEmployeeIdentityRequest,
};
use wareboxes_application::workforce_identity::{
    EmployeeIdentityChangeResult, LinkEmployeeIdentityCommand, UnlinkEmployeeIdentityCommand,
};
use wareboxes_domain::{EmployeeId, EmployeeIdentityChangeKind, EmployeeIdentityReason, UserId};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "admin";

pub async fn link(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(employee_id): Path<i64>,
    Json(body): Json<LinkEmployeeIdentityRequest>,
) -> V1Result<Json<EmployeeIdentityChangeResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = LinkEmployeeIdentityCommand {
        employee_id: EmployeeId::new(employee_id).map_err(invalid)?,
        user_id: UserId::new(body.user_id).map_err(invalid)?,
        expected_user_id: body
            .expected_user_id
            .map(UserId::new)
            .transpose()
            .map_err(invalid)?,
        reason: EmployeeIdentityReason::new(body.reason).map_err(invalid)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::employees::link_employee_identity(&state.db, &user.tenant, &context, &command)
            .await?;
    Ok(Json(response(result)))
}

pub async fn unlink(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(employee_id): Path<i64>,
    Json(body): Json<UnlinkEmployeeIdentityRequest>,
) -> V1Result<Json<EmployeeIdentityChangeResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = UnlinkEmployeeIdentityCommand {
        employee_id: EmployeeId::new(employee_id).map_err(invalid)?,
        expected_user_id: UserId::new(body.expected_user_id).map_err(invalid)?,
        reason: EmployeeIdentityReason::new(body.reason).map_err(invalid)?,
    };
    let context = user.command_context(&idempotency_key);
    let result =
        repo::employees::unlink_employee_identity(&state.db, &user.tenant, &context, &command)
            .await?;
    Ok(Json(response(result)))
}

fn response(result: EmployeeIdentityChangeResult) -> EmployeeIdentityChangeResponse {
    EmployeeIdentityChangeResponse {
        change_id: result.change_id.get(),
        employee_id: result.employee_id.get(),
        previous_user_id: result.previous_user_id.map(UserId::get),
        user_id: result.user_id.map(UserId::get),
        kind: match result.kind {
            EmployeeIdentityChangeKind::Linked => ApiChangeKind::Linked,
            EmployeeIdentityChangeKind::Relinked => ApiChangeKind::Relinked,
            EmployeeIdentityChangeKind::Unlinked => ApiChangeKind::Unlinked,
        },
        reason: result.reason.as_str().to_owned(),
        changed_by: result.changed_by.get(),
        changed_at: result.changed_at.to_rfc3339(),
        resulting_revision: result.resulting_revision,
    }
}

fn invalid(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_and_payload_values_are_validated() {
        assert!(EmployeeId::new(0).is_err());
        assert!(UserId::new(0).is_err());
        assert!(EmployeeIdentityReason::new(" ").is_err());
    }
}
