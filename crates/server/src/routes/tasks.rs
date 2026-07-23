use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use wareboxes_core::dto::{
    AssignWorkTask, CompleteWorkTask, CreateBreakMasterPackTask, CreateItemLocationCycleCountTask,
    CreateLocationCycleCountTask, CreateUnpackCancelledOrderTask, RecordWorkTaskProgress,
    StartNextWorkTask, WorkTaskIdRequest,
};
use wareboxes_core::models::{
    UnpackCancelledOrderTaskLine, WorkTask, WorkTaskStatus, WorkTaskType,
};

use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "wms";

#[derive(Debug, Deserialize)]
pub struct WorkTaskListQuery {
    pub show_deleted: Option<bool>,
    pub status: Option<WorkTaskStatus>,
    pub task_type: Option<WorkTaskType>,
    pub assigned_user_id: Option<i64>,
    pub location_id: Option<i64>,
    pub order_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct WorkTaskLinesQuery {
    pub task_id: i64,
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<WorkTaskListQuery>,
) -> AppResult<Json<Vec<WorkTask>>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::tasks::get_tasks_in_scope(
            &state.db,
            &user.tenant,
            repo::tasks::WorkTaskFilters {
                show_deleted: q.show_deleted.unwrap_or(false),
                status: q.status,
                task_type: q.task_type,
                assigned_user_id: q.assigned_user_id,
                location_id: q.location_id,
                order_id: q.order_id,
            },
        )
        .await?,
    ))
}

pub async fn list_unpack_cancelled_order_lines(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<WorkTaskLinesQuery>,
) -> AppResult<Json<Vec<UnpackCancelledOrderTaskLine>>> {
    user.require_permission(&state.db, PERM).await?;
    if q.task_id <= 0 {
        return Err(AppError::bad_request("invalid task ID"));
    }
    Ok(Json(
        repo::tasks::get_unpack_cancelled_order_task_lines_in_scope(
            &state.db,
            &user.tenant,
            q.task_id,
        )
        .await?,
    ))
}

pub async fn create_item_location_cycle_count(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<CreateItemLocationCycleCountTask>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::tasks::create_item_location_cycle_count_task_in_scope(
            &state.db,
            &user.tenant,
            user.user.id,
            body.location_id,
            body.item_id,
            body.source.as_deref(),
            body.order_id,
            body.order_item_id,
            body.inventory_balance_id,
            body.note.as_deref(),
        )
        .await?,
    ))
}

pub async fn create_location_cycle_count(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<CreateLocationCycleCountTask>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::tasks::create_location_cycle_count_task_in_scope(
            &state.db,
            &user.tenant,
            user.user.id,
            body.location_id,
            body.priority,
            body.assigned_user_id,
            body.scheduled_for,
            body.due_at,
            body.instructions,
        )
        .await?,
    ))
}

pub async fn create_break_master_pack(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<CreateBreakMasterPackTask>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::tasks::create_break_master_pack_task_in_scope(
            &state.db,
            &user.tenant,
            user.user.id,
            body.master_item_id,
            body.single_item_id,
            body.location_id,
            body.qty,
            body.priority,
            body.assigned_user_id,
            body.scheduled_for,
            body.due_at,
            body.instructions,
        )
        .await?,
    ))
}

pub async fn create_unpack_cancelled_order(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<CreateUnpackCancelledOrderTask>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    Ok(Json(
        repo::tasks::create_unpack_cancelled_order_task_in_scope(
            &state.db,
            &user.tenant,
            Some(user.user.id),
            body.order_id,
            body.facility_id,
            body.priority,
            body.assigned_user_id,
            body.scheduled_for,
            body.due_at,
            body.instructions,
        )
        .await?,
    ))
}

pub async fn assign(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<AssignWorkTask>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let command = user.command_context(&idempotency_key);
    let ok = repo::tasks::assign_task_in_scope(
        &state.db,
        &user.tenant,
        &command,
        body.task_id,
        body.assigned_user_id,
    )
    .await?;
    if !ok {
        return Err(AppError::conflict("task cannot be assigned"));
    }
    Ok(Json(ok))
}

pub async fn start_next(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<StartNextWorkTask>,
) -> AppResult<Json<Option<WorkTask>>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let command = user.command_context(&idempotency_key);
    Ok(Json(
        repo::tasks::start_next_task_in_scope(&state.db, &user.tenant, &command, body.task_type)
            .await?,
    ))
}

pub async fn start(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<WorkTaskIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let command = user.command_context(&idempotency_key);
    let ok =
        repo::tasks::start_task_in_scope(&state.db, &user.tenant, &command, body.task_id).await?;
    if !ok {
        return Err(AppError::conflict("task cannot be started"));
    }
    Ok(Json(ok))
}

pub async fn progress(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<RecordWorkTaskProgress>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let command = user.command_context(&idempotency_key);
    let ok = repo::tasks::record_task_progress_in_scope(
        &state.db,
        &user.tenant,
        &command,
        body.task_id,
        body.task_line_id,
        body.action,
        body.qty_completed,
        body.from_location_id,
        body.to_location_id,
        body.note.as_deref(),
    )
    .await?;
    if !ok {
        return Err(AppError::conflict("task progress cannot be recorded"));
    }
    Ok(Json(ok))
}

pub async fn complete(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CompleteWorkTask>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let command = user.command_context(&idempotency_key);
    let ok = repo::tasks::complete_task_in_scope(
        &state.db,
        &user.tenant,
        &command,
        body.task_id,
        body.qty_completed,
    )
    .await?;
    if !ok {
        return Err(AppError::conflict("task cannot be completed"));
    }
    Ok(Json(ok))
}

pub async fn abort(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<WorkTaskIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let command = user.command_context(&idempotency_key);
    let ok =
        repo::tasks::abort_task_in_scope(&state.db, &user.tenant, &command, body.task_id).await?;
    if !ok {
        return Err(AppError::conflict("task cannot be aborted"));
    }
    Ok(Json(ok))
}

pub async fn release_expired(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> AppResult<Json<u64>> {
    user.require_permission(&state.db, PERM).await?;
    Ok(Json(
        repo::tasks::release_expired_tasks_in_scope(&state.db, &user.tenant).await?,
    ))
}

pub async fn cancel(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<WorkTaskIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let command = user.command_context(&idempotency_key);
    let ok =
        repo::tasks::cancel_task_in_scope(&state.db, &user.tenant, &command, body.task_id).await?;
    if !ok {
        return Err(AppError::conflict("task cannot be cancelled"));
    }
    Ok(Json(ok))
}
