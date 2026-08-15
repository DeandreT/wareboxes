use axum::extract::{Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    OpaqueCursor, WorkOrchestrationWorkerOptionResponse,
    WorkOrchestrationWorkerPage as ApiWorkerPage, WorkOrchestrationWorkerPageRequest,
};
use wareboxes_application::work_orchestration::{
    WorkOrchestrationWorkerCursor, WorkOrchestrationWorkerPageQuery,
};
use wareboxes_domain::EmployeeId;

use super::super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::repo;
use crate::state::AppState;

const SUPERVISOR_PERMISSION: &str = "wms_supervisor";
const WORKER_CURSOR_PREFIX: &str = "wow1.";

pub async fn list_workers(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<WorkOrchestrationWorkerPageRequest>,
) -> V1Result<Json<ApiWorkerPage>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let facility_id = user.require_facility(request.facility_id)?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| user.require_inventory_owner(id))
        .transpose()?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, &request))
        .transpose()?;
    let page = repo::work_orchestration::worker_page(
        &state.db,
        &user.tenant,
        WorkOrchestrationWorkerPageQuery {
            facility_id,
            inventory_owner_id,
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_cursor(cursor, &request))
        .transpose()?;
    Ok(Json(ApiWorkerPage::new(
        page.items
            .into_iter()
            .map(|item| WorkOrchestrationWorkerOptionResponse {
                employee_id: item.employee_id.get(),
                user_id: item.user_id.get(),
                display_name: item.display_name,
                title: item.title,
            })
            .collect(),
        next_cursor,
    )))
}

fn cursor_filter(request: &WorkOrchestrationWorkerPageRequest) -> String {
    format!(
        "{:016x}.{:016x}",
        request.facility_id,
        request.inventory_owner_id.unwrap_or_default()
    )
}

fn encode_cursor(
    cursor: WorkOrchestrationWorkerCursor,
    request: &WorkOrchestrationWorkerPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{WORKER_CURSOR_PREFIX}{}.{:016x}",
        cursor_filter(request),
        cursor.after_employee_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid work orchestration worker cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    request: &WorkOrchestrationWorkerPageRequest,
) -> V1Result<WorkOrchestrationWorkerCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(WORKER_CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration worker"))?;
    let (filter, employee_id) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("work orchestration worker"))?;
    if filter != cursor_filter(request) {
        return Err(V1Error::invalid_cursor_for("work orchestration worker"));
    }
    let employee_id = i64::from_str_radix(employee_id, 16)
        .map_err(|_| V1Error::invalid_cursor_for("work orchestration worker"))?;
    Ok(WorkOrchestrationWorkerCursor {
        after_employee_id: EmployeeId::new(employee_id)
            .map_err(|_| V1Error::invalid_cursor_for("work orchestration worker"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::PageLimit;

    #[test]
    fn worker_cursor_is_bound_to_facility_and_owner() {
        let request = WorkOrchestrationWorkerPageRequest {
            facility_id: 7,
            inventory_owner_id: Some(11),
            cursor: None,
            limit: PageLimit::default(),
        };
        let cursor = WorkOrchestrationWorkerCursor {
            after_employee_id: EmployeeId::new(41).unwrap(),
        };
        let encoded = encode_cursor(cursor, &request).unwrap();
        assert_eq!(decode_cursor(&encoded, &request).unwrap(), cursor);
        let mut changed = request;
        changed.inventory_owner_id = Some(12);
        assert!(decode_cursor(&encoded, &changed).is_err());
    }
}
