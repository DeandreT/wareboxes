use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;
use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::pick_cluster::{
    CancelPickClusterCommand, ChangePickCartStatusCommand, ClaimNextClusterPickCommand,
    CreatePickCartCommand, PickCartReadModel, PickClusterReadModel, PlanPickClusterCommand,
    CANCEL_PICK_CLUSTER_OPERATION, CHANGE_PICK_CART_STATUS_OPERATION,
    CLAIM_NEXT_CLUSTER_PICK_OPERATION, CREATE_PICK_CART_OPERATION, PLAN_PICK_CLUSTER_OPERATION,
};
use wareboxes_application::picking::PickClaim;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    validate_pick_cart_slot_count, validate_pick_cluster_plan, FacilityId, InventoryOwnerId,
    OrderId, PickCartId, PickCartSlotId, PickCartStatus, PickClusterId, PickClusterPlanLine,
    PickClusterStatus, PickTaskId, TenantId, UserId, MAX_PICK_CLUSTER_CANCEL_NOTE_LENGTH,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::next_outbox_sequence_tx;

use super::super::claim::{
    active_task_for_user_tx, claim_open_task_tx, load_claim_tx, release_expired_claims_tx,
    release_inaccessible_claim_tx,
};
use super::models::{cart_status, cart_status_text, internal, read_cart_tx, read_cluster_tx};

#[derive(Debug)]
struct PlanTask {
    task_id: PickTaskId,
    order_id: OrderId,
}

pub async fn create_cart(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreatePickCartCommand,
) -> AppResult<PickCartReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    validate_pick_cart_slot_count(command.slot_codes.len())
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let unique_codes = command
        .slot_codes
        .iter()
        .map(|code| code.as_str())
        .collect::<BTreeSet<_>>();
    if unique_codes.len() != command.slot_codes.len() {
        return Err(AppError::bad_request("pick cart slot codes must be unique"));
    }
    let prepared = PreparedCommand::new_v1(context, CREATE_PICK_CART_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        "wms_supervisor",
    )
    .await?;
    if let Some(result) = prepared.replayed::<PickCartReadModel>(&mut tx).await? {
        require_facility(&scope, result.facility_id, "pick cart")?;
        tx.commit().await?;
        return Ok(result);
    }
    require_facility(&scope, command.facility_id, "pick cart")?;
    require_active_facility_tx(&mut tx, access.tenant_id, command.facility_id).await?;
    let created_at = now_iso();
    let cart_id = PickCartId::new(
        sqlx::query_scalar(
            r#"INSERT INTO pick_carts(
              tenant_id,facility_id,barcode,name,status,revision,created_by_user_id,created_at)
            VALUES($1,$2,$3,$4,'active',1,$5,$6) RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.facility_id.get())
        .bind(command.barcode.as_str())
        .bind(command.name.as_str())
        .bind(context.actor_id.get())
        .bind(created_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(internal)?;
    for (index, code) in command.slot_codes.iter().enumerate() {
        let sequence = i64::try_from(index + 1).map_err(internal)?;
        sqlx::query(
            r#"INSERT INTO pick_cart_slots(
              tenant_id,facility_id,cart_id,code,sequence,created_at)
            VALUES($1,$2,$3,$4,$5,$6)"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.facility_id.get())
        .bind(cart_id.get())
        .bind(code.as_str())
        .bind(sequence)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
    }
    let result = read_cart_tx(&mut tx, access.tenant_id, cart_id).await?;
    enqueue_cart_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "outbound.pick_cart.created",
        "created",
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn change_cart_status(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ChangePickCartStatusCommand,
) -> AppResult<PickCartReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.expected_revision <= 0 {
        return Err(AppError::bad_request("expected revision must be positive"));
    }
    let prepared = PreparedCommand::new_v1(context, CHANGE_PICK_CART_STATUS_OPERATION, &command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        "wms_supervisor",
    )
    .await?;
    if let Some(result) = prepared.replayed::<PickCartReadModel>(&mut tx).await? {
        require_facility(&scope, result.facility_id, "pick cart")?;
        tx.commit().await?;
        return Ok(result);
    }
    let row = sqlx::query(
        "SELECT facility_id,status,revision FROM pick_carts WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(access.tenant_id.get())
    .bind(command.cart_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick cart"))?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?).map_err(internal)?;
    require_facility(&scope, facility_id, "pick cart")?;
    let revision: i64 = row.try_get("revision")?;
    if revision != command.expected_revision {
        return Err(AppError::conflict("pick cart revision is stale"));
    }
    let current = cart_status(&row.try_get::<String, _>("status")?)?;
    current
        .transition(command.status)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    if command.status != PickCartStatus::Active {
        let has_active_route: bool = sqlx::query_scalar(
            r#"SELECT EXISTS(SELECT 1 FROM pick_clusters
            WHERE tenant_id=$1 AND cart_id=$2 AND status IN('planned','in_progress'))"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.cart_id.get())
        .fetch_one(&mut *tx)
        .await?;
        if has_active_route {
            return Err(AppError::conflict(
                "cancel or complete the active pick cluster before changing cart status",
            ));
        }
    }
    let changed_at = now_iso();
    let updated = sqlx::query(
        r#"UPDATE pick_carts SET status=$1,revision=$2,
          status_changed_by_user_id=$3,status_changed_at=$4
        WHERE tenant_id=$5 AND id=$6 AND revision=$7"#,
    )
    .bind(cart_status_text(command.status))
    .bind(revision + 1)
    .bind(context.actor_id.get())
    .bind(changed_at)
    .bind(access.tenant_id.get())
    .bind(command.cart_id.get())
    .bind(revision)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("pick cart changed concurrently"));
    }
    let result = read_cart_tx(&mut tx, access.tenant_id, command.cart_id).await?;
    enqueue_cart_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "outbound.pick_cart.status_changed",
        cart_status_text(result.status),
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn plan(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &PlanPickClusterCommand,
) -> AppResult<PickClusterReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if !(2..=wareboxes_domain::MAX_PICK_CLUSTER_TASKS).contains(&command.assignments.len()) {
        return Err(AppError::bad_request(
            "a pick cluster must contain between 2 and 200 tasks",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, PLAN_PICK_CLUSTER_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        "wms_supervisor",
    )
    .await?;
    if let Some(result) = prepared.replayed::<PickClusterReadModel>(&mut tx).await? {
        require_cluster_scope(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    require_owner_facility(&scope, command.inventory_owner_id, command.facility_id)?;
    require_owner_facility_pair_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id,
    )
    .await?;
    lock_active_cart_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id,
        command.cart_id,
    )
    .await?;

    let mut task_to_slot = BTreeMap::new();
    for assignment in &command.assignments {
        if task_to_slot
            .insert(assignment.task_id.get(), assignment.slot_id)
            .is_some()
        {
            return Err(AppError::bad_request(
                "a pick cluster cannot contain the same task twice",
            ));
        }
    }
    require_cart_slots_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id,
        command.cart_id,
        &task_to_slot.values().copied().collect::<Vec<_>>(),
    )
    .await?;
    let tasks = lock_plan_tasks_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id,
        &task_to_slot.keys().copied().collect::<Vec<_>>(),
    )
    .await?;
    if tasks.len() != task_to_slot.len() {
        return Err(AppError::conflict(
            "one or more pick tasks are no longer eligible for cluster planning",
        ));
    }
    let plan_lines = tasks
        .iter()
        .map(|task| {
            Ok(PickClusterPlanLine {
                task_id: task.task_id,
                order_id: task.order_id,
                slot_id: *task_to_slot
                    .get(&task.task_id.get())
                    .ok_or_else(|| AppError::internal("pick cluster slot mapping is missing"))?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    validate_pick_cluster_plan(&plan_lines)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let planned_at = now_iso();
    let order_count = plan_lines
        .iter()
        .map(|line| line.order_id.get())
        .collect::<BTreeSet<_>>()
        .len();
    let cluster_id = PickClusterId::new(
        sqlx::query_scalar(
            r#"INSERT INTO pick_clusters(
              tenant_id,inventory_owner_id,facility_id,cart_id,status,revision,
              task_count,order_count,planned_by_user_id,planned_at)
            VALUES($1,$2,$3,$4,'planned',1,$5,$6,$7,$8) RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(command.cart_id.get())
        .bind(i64::try_from(plan_lines.len()).map_err(internal)?)
        .bind(i64::try_from(order_count).map_err(internal)?)
        .bind(context.actor_id.get())
        .bind(planned_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(internal)?;
    let mut order_slots = BTreeMap::new();
    for line in &plan_lines {
        order_slots
            .entry(line.order_id.get())
            .or_insert(line.slot_id);
    }
    for (order_id, slot_id) in order_slots {
        sqlx::query(
            r#"INSERT INTO pick_cluster_orders(
              tenant_id,inventory_owner_id,facility_id,cluster_id,cart_id,order_id,slot_id)
            VALUES($1,$2,$3,$4,$5,$6,$7)"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(cluster_id.get())
        .bind(command.cart_id.get())
        .bind(order_id)
        .bind(slot_id.get())
        .execute(&mut *tx)
        .await?;
    }
    for (index, task) in tasks.iter().enumerate() {
        let slot_id = task_to_slot[&task.task_id.get()];
        sqlx::query(
            r#"INSERT INTO pick_cluster_members(
              tenant_id,inventory_owner_id,facility_id,cluster_id,cart_id,
              order_id,slot_id,task_id,sequence,created_at)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(cluster_id.get())
        .bind(command.cart_id.get())
        .bind(task.order_id.get())
        .bind(slot_id.get())
        .bind(task.task_id.get())
        .bind(i64::try_from(index + 1).map_err(internal)?)
        .bind(planned_at)
        .execute(&mut *tx)
        .await?;
    }
    let result = read_cluster_tx(&mut tx, access.tenant_id, cluster_id).await?;
    enqueue_cluster_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "outbound.pick_cluster.planned",
        "planned",
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn claim_next(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ClaimNextClusterPickCommand,
) -> AppResult<Option<PickClaim>> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CLAIM_NEXT_CLUSTER_PICK_OPERATION, &command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared.replayed::<Option<PickClaim>>(&mut tx).await? {
        require_cluster_visible_tx(&mut tx, access.tenant_id, command.cluster_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }
    release_expired_claims_tx(&mut tx, access.tenant_id, &scope).await?;
    release_inaccessible_claim_tx(&mut tx, access.tenant_id, context.actor_id.get(), &scope)
        .await?;
    let row = sqlx::query(
        r#"SELECT cluster.inventory_owner_id,cluster.facility_id,cluster.cart_id,
          cluster.status,cluster.assigned_user_id,cart.status AS cart_status
        FROM pick_clusters cluster
        JOIN pick_carts cart ON cart.tenant_id=cluster.tenant_id
          AND cart.facility_id=cluster.facility_id AND cart.id=cluster.cart_id
        WHERE cluster.tenant_id=$1 AND cluster.id=$2 FOR UPDATE OF cluster,cart"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.cluster_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick cluster"))?;
    let owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?).map_err(internal)?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?).map_err(internal)?;
    require_owner_facility(&scope, owner_id, facility_id)?;
    if row.try_get::<String, _>("cart_status")? != "active" {
        return Err(AppError::conflict("pick cluster cart is not active"));
    }
    let status: String = row.try_get("status")?;
    let assigned_user_id: Option<i64> = row.try_get("assigned_user_id")?;
    if !matches!(status.as_str(), "planned" | "in_progress") {
        return Err(AppError::conflict("pick cluster is not executable"));
    }
    if assigned_user_id.is_some_and(|user_id| user_id != context.actor_id.get()) {
        return Err(AppError::conflict(
            "pick cluster is assigned to another operator",
        ));
    }
    if let Some(active_task_id) =
        active_task_for_user_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?
    {
        let belongs: bool = sqlx::query_scalar(
            r#"SELECT EXISTS(SELECT 1 FROM pick_cluster_members
            WHERE tenant_id=$1 AND cluster_id=$2 AND task_id=$3)"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.cluster_id.get())
        .bind(active_task_id)
        .fetch_one(&mut *tx)
        .await?;
        if !belongs {
            return Err(AppError::conflict(
                "operator already has active pick work outside this cluster",
            ));
        }
        let claim = load_claim_tx(
            &mut tx,
            access.tenant_id,
            PickTaskId::new(active_task_id).map_err(internal)?,
            context.actor_id.get(),
        )
        .await?;
        return Ok(prepared.commit(tx, Some(claim)).await?);
    }
    let started_at = now_iso();
    if status == "planned" {
        sqlx::query(
            r#"UPDATE pick_clusters SET status='in_progress',revision=2,
              assigned_user_id=$1,started_at=$2 WHERE tenant_id=$3 AND id=$4
              AND status='planned'"#,
        )
        .bind(context.actor_id.get())
        .bind(started_at)
        .bind(access.tenant_id.get())
        .bind(command.cluster_id.get())
        .execute(&mut *tx)
        .await?;
    }
    let task_id: Option<i64> = sqlx::query_scalar(
        r#"SELECT member.task_id FROM pick_cluster_members member
        JOIN pick_tasks task ON task.tenant_id=member.tenant_id AND task.id=member.task_id
        WHERE member.tenant_id=$1 AND member.cluster_id=$2 AND task.status='open'
          AND task.assigned_user_id IS NULL ORDER BY member.sequence
        FOR UPDATE OF task SKIP LOCKED LIMIT 1"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.cluster_id.get())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(task_id) = task_id else {
        return Err(AppError::conflict(
            "pick cluster has no claimable task; refresh its execution state",
        ));
    };
    claim_open_task_tx(
        &mut tx,
        access.tenant_id,
        task_id,
        owner_id,
        facility_id,
        context.actor_id.get(),
        started_at,
    )
    .await?;
    let claim = load_claim_tx(
        &mut tx,
        access.tenant_id,
        PickTaskId::new(task_id).map_err(internal)?,
        context.actor_id.get(),
    )
    .await?;
    if status == "planned" {
        let cluster = read_cluster_tx(&mut tx, access.tenant_id, command.cluster_id).await?;
        enqueue_cluster_event_tx(
            &mut tx,
            access.tenant_id,
            context.actor_id,
            &cluster,
            "outbound.pick_cluster.started",
            "started",
        )
        .await?;
    }
    Ok(prepared.commit(tx, Some(claim)).await?)
}

pub async fn cancel(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelPickClusterCommand,
) -> AppResult<PickClusterReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let note = command.note.trim();
    if command.expected_revision <= 0
        || note.is_empty()
        || note.chars().count() > MAX_PICK_CLUSTER_CANCEL_NOTE_LENGTH
    {
        return Err(AppError::bad_request(
            "cluster cancellation requires a positive revision and a note of at most 500 characters",
        ));
    }
    let prepared = PreparedCommand::new_v1(context, CANCEL_PICK_CLUSTER_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        "wms_supervisor",
    )
    .await?;
    if let Some(result) = prepared.replayed::<PickClusterReadModel>(&mut tx).await? {
        require_cluster_scope(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    let row = sqlx::query(
        r#"SELECT inventory_owner_id,facility_id,status,revision FROM pick_clusters
        WHERE tenant_id=$1 AND id=$2 FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.cluster_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick cluster"))?;
    let owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?).map_err(internal)?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?).map_err(internal)?;
    require_owner_facility(&scope, owner_id, facility_id)?;
    let revision: i64 = row.try_get("revision")?;
    if revision != command.expected_revision {
        return Err(AppError::conflict("pick cluster revision is stale"));
    }
    let status = match row.try_get::<String, _>("status")?.as_str() {
        "planned" => PickClusterStatus::Planned,
        "in_progress" => PickClusterStatus::InProgress,
        _ => return Err(AppError::conflict("pick cluster cannot be cancelled")),
    };
    status
        .cancel()
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let active_task: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM pick_cluster_members member
        JOIN pick_tasks task ON task.tenant_id=member.tenant_id AND task.id=member.task_id
        WHERE member.tenant_id=$1 AND member.cluster_id=$2 AND task.status='in_progress')"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.cluster_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if active_task {
        return Err(AppError::conflict(
            "release the active pick claim before cancelling its cluster",
        ));
    }
    let cancelled_at = now_iso();
    sqlx::query(
        r#"UPDATE pick_clusters SET status='cancelled',revision=$1,
          cancelled_by_user_id=$2,cancelled_at=$3,cancellation_note=$4
        WHERE tenant_id=$5 AND id=$6 AND revision=$7"#,
    )
    .bind(revision + 1)
    .bind(context.actor_id.get())
    .bind(cancelled_at)
    .bind(note)
    .bind(access.tenant_id.get())
    .bind(command.cluster_id.get())
    .bind(revision)
    .execute(&mut *tx)
    .await?;
    let result = read_cluster_tx(&mut tx, access.tenant_id, command.cluster_id).await?;
    enqueue_cluster_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "outbound.pick_cluster.cancelled",
        "cancelled",
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub(in crate::repo) async fn enqueue_terminal_event_for_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: UserId,
    task_id: PickTaskId,
) -> AppResult<()> {
    let cluster_id: Option<i64> = sqlx::query_scalar(
        r#"SELECT cluster.id FROM pick_cluster_members member
        JOIN pick_clusters cluster ON cluster.tenant_id=member.tenant_id
          AND cluster.id=member.cluster_id
        WHERE member.tenant_id=$1 AND member.task_id=$2 AND cluster.status='completed'
        ORDER BY cluster.id DESC LIMIT 1"#,
    )
    .bind(tenant_id.get())
    .bind(task_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(cluster_id) = cluster_id else {
        return Ok(());
    };
    let cluster_id = PickClusterId::new(cluster_id).map_err(internal)?;
    let event_key = format!("pick-cluster:{}:completed:3", cluster_id.get());
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM outbox_event_keys WHERE tenant_id=$1 AND event_key=$2)",
    )
    .bind(tenant_id.get())
    .bind(&event_key)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        return Ok(());
    }
    let cluster = read_cluster_tx(tx, tenant_id, cluster_id).await?;
    enqueue_cluster_event_tx(
        tx,
        tenant_id,
        actor_user_id,
        &cluster,
        "outbound.pick_cluster.completed",
        "completed",
    )
    .await
}

async fn lock_plan_tasks_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    task_ids: &[i64],
) -> AppResult<Vec<PlanTask>> {
    let locked_ids: Vec<i64> = sqlx::query_scalar(
        r#"SELECT task.id FROM pick_tasks task
        WHERE task.tenant_id=$1 AND task.inventory_owner_id=$2 AND task.facility_id=$3
          AND task.id=ANY($4) ORDER BY task.id FOR UPDATE"#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id.get())
    .bind(task_ids)
    .fetch_all(&mut **tx)
    .await?;
    if locked_ids.len() != task_ids.len() {
        return Err(AppError::not_found("pick cluster task"));
    }
    let rows = sqlx::query(
        r#"SELECT task.id AS task_id,task.order_id,location.barcode AS source_barcode
        FROM pick_tasks task
        JOIN pick_task_contents content ON content.tenant_id=task.tenant_id
          AND content.task_id=task.id AND content.state='pending'
        JOIN locations location ON location.tenant_id=content.tenant_id
          AND location.facility_id=content.facility_id AND location.id=content.source_location_id
          AND location.deleted IS NULL AND location.active AND location.pickable
        JOIN orders ON orders.tenant_id=task.tenant_id AND orders.id=task.order_id
          AND orders.inventory_owner_id=task.inventory_owner_id
          AND orders.status='processing' AND orders.deleted IS NULL
        WHERE task.tenant_id=$1 AND task.inventory_owner_id=$2 AND task.facility_id=$3
          AND task.id=ANY($4) AND task.status='open' AND task.assigned_user_id IS NULL
          AND NOT EXISTS(
            SELECT 1 FROM pick_cluster_members member
            JOIN pick_clusters cluster ON cluster.tenant_id=member.tenant_id
              AND cluster.id=member.cluster_id
            WHERE member.tenant_id=task.tenant_id AND member.task_id=task.id
              AND cluster.status IN('planned','in_progress'))
        ORDER BY lower(location.barcode),task.id"#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id.get())
    .bind(task_ids)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(PlanTask {
                task_id: PickTaskId::new(row.try_get("task_id")?).map_err(internal)?,
                order_id: OrderId::new(row.try_get("order_id")?).map_err(internal)?,
            })
        })
        .collect()
}

async fn lock_active_cart_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    cart_id: PickCartId,
) -> AppResult<()> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM pick_carts WHERE tenant_id=$1 AND facility_id=$2 AND id=$3 FOR UPDATE",
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(cart_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    match status.as_deref() {
        Some("active") => Ok(()),
        Some(_) => Err(AppError::conflict("pick cart is not active")),
        None => Err(AppError::not_found("pick cart")),
    }
}

async fn require_cart_slots_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    cart_id: PickCartId,
    slot_ids: &[PickCartSlotId],
) -> AppResult<()> {
    let unique = slot_ids
        .iter()
        .map(|slot| slot.get())
        .collect::<BTreeSet<_>>();
    let count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM pick_cart_slots
        WHERE tenant_id=$1 AND facility_id=$2 AND cart_id=$3 AND id=ANY($4)"#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(cart_id.get())
    .bind(unique.iter().copied().collect::<Vec<_>>())
    .fetch_one(&mut **tx)
    .await?;
    if usize::try_from(count).map_err(internal)? != unique.len() {
        Err(AppError::not_found("pick cart slot"))
    } else {
        Ok(())
    }
}

async fn require_active_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM facilities WHERE tenant_id=$1 AND id=$2
        AND deleted IS NULL)"#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::not_found("pick cart"))
    }
}

async fn require_owner_facility_pair_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM inventory_owner_facilities
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND deleted IS NULL)"#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::not_found("pick cluster"))
    }
}

async fn require_cluster_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    cluster_id: PickClusterId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id FROM pick_clusters WHERE tenant_id=$1 AND id=$2",
    )
    .bind(tenant_id.get())
    .bind(cluster_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick cluster"))?;
    require_owner_facility(
        scope,
        InventoryOwnerId::new(row.try_get("inventory_owner_id")?).map_err(internal)?,
        FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
    )
}

fn require_cluster_scope(scope: &ScopeBindings, cluster: &PickClusterReadModel) -> AppResult<()> {
    require_owner_facility(scope, cluster.inventory_owner_id, cluster.facility_id)
}

fn require_owner_facility(
    scope: &ScopeBindings,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    if scope.includes_inventory_owner(owner_id.get()) && scope.includes_facility(facility_id.get())
    {
        Ok(())
    } else {
        Err(AppError::not_found("pick cluster"))
    }
}

fn require_facility(
    scope: &ScopeBindings,
    facility_id: FacilityId,
    resource: &str,
) -> AppResult<()> {
    if scope.includes_facility(facility_id.get()) {
        Ok(())
    } else {
        Err(AppError::not_found(resource))
    }
}

async fn enqueue_cart_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: UserId,
    cart: &PickCartReadModel,
    event_type: &str,
    transition: &str,
) -> AppResult<()> {
    let aggregate_id = cart.cart_id.to_string();
    let ordering_key = format!("pick-cart:{}", cart.cart_id.get());
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: None,
            facility_id: Some(cart.facility_id),
            actor_user_id: Some(actor_user_id.get()),
            event_key: &format!(
                "pick-cart:{}:{}:{}",
                cart.cart_id.get(),
                transition,
                cart.revision
            ),
            aggregate_type: "pick_cart",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type,
            schema_version: 1,
            payload: &json!({
                "cart_id": cart.cart_id,
                "facility_id": cart.facility_id,
                "barcode": cart.barcode,
                "name": cart.name,
                "status": cart.status,
                "revision": cart.revision,
                "slot_count": cart.slots.len(),
            }),
            occurred_at: cart.status_changed_at.unwrap_or(cart.created_at),
        },
    )
    .await?;
    Ok(())
}

async fn enqueue_cluster_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: UserId,
    cluster: &PickClusterReadModel,
    event_type: &str,
    transition: &str,
) -> AppResult<()> {
    let aggregate_id = cluster.cluster_id.to_string();
    let ordering_key = format!("pick-cluster:{}", cluster.cluster_id.get());
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(cluster.inventory_owner_id),
            facility_id: Some(cluster.facility_id),
            actor_user_id: Some(actor_user_id.get()),
            event_key: &format!(
                "pick-cluster:{}:{}:{}",
                cluster.cluster_id.get(),
                transition,
                cluster.revision
            ),
            aggregate_type: "pick_cluster",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type,
            schema_version: 1,
            payload: &json!({
                "cluster_id": cluster.cluster_id,
                "inventory_owner_id": cluster.inventory_owner_id,
                "facility_id": cluster.facility_id,
                "cart_id": cluster.cart_id,
                "cart_barcode": cluster.cart_barcode,
                "status": cluster.status,
                "revision": cluster.revision,
                "task_count": cluster.task_count,
                "order_count": cluster.order_count,
                "completed_task_count": cluster.completed_task_count,
                "assigned_user_id": cluster.assigned_user_id,
                "members": cluster.members,
            }),
            occurred_at: cluster
                .completed_at
                .or(cluster.cancelled_at)
                .or(cluster.started_at)
                .unwrap_or(cluster.planned_at),
        },
    )
    .await?;
    Ok(())
}
