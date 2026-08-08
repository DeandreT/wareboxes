use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::replenishment::{
    ConfigureReplenishmentPolicyCommand, ConfigureReplenishmentPolicyResult,
    RetireReplenishmentPolicyCommand, RetireReplenishmentPolicyResult,
    CONFIGURE_REPLENISHMENT_POLICY_OPERATION, RETIRE_REPLENISHMENT_POLICY_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    ReplenishmentPolicyId, ReplenishmentPolicyRevision, ReplenishmentPolicyStatus, TenantId,
    Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

use super::{
    enqueue_event_tx, policy_from_row, policy_sources_tx, require_scope,
    require_stored_policy_visible_before_replay_tx, PolicyRow,
};

pub async fn configure_policy(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureReplenishmentPolicyCommand,
) -> AppResult<ConfigureReplenishmentPolicyResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.definition.scope().tenant_id != access.tenant_id {
        return Err(AppError::not_found("replenishment policy"));
    }
    let prepared =
        PreparedCommand::new_v1(context, CONFIGURE_REPLENISHMENT_POLICY_OPERATION, command)?;
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

    require_stored_policy_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;

    if let Some(result) = prepared
        .replayed::<ConfigureReplenishmentPolicyResult>(&mut tx)
        .await?
    {
        require_replayed_policy_visible_tx(&mut tx, access.tenant_id, result.policy_id, &scope)
            .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let natural = command.definition.scope();
    require_scope(
        &scope,
        natural.inventory_owner_id.get(),
        natural.facility_id.get(),
    )?;
    lock_natural_key_tx(&mut tx, natural).await?;
    validate_references_tx(&mut tx, command).await?;
    let predecessor = latest_policy_tx(&mut tx, access.tenant_id, command).await?;
    validate_expected_revision(command.expected_revision, predecessor.as_ref())?;
    if let Some(active) = predecessor
        .as_ref()
        .filter(|row| row.effective_to.is_none())
    {
        ensure_no_active_work_tx(&mut tx, access.tenant_id, active.id).await?;
    }

    let configured_at = now_iso();
    if let Some(predecessor) = predecessor.as_ref() {
        if predecessor.effective_to.is_none() {
            retire_row_tx(
                &mut tx,
                access.tenant_id,
                predecessor.id,
                context.actor_id.get(),
                configured_at,
            )
            .await?;
        }
    }
    let revision = predecessor.as_ref().map_or(
        Ok(ReplenishmentPolicyRevision::new(1)
            .map_err(|error| AppError::internal(error.to_string()))?),
        |row| {
            row.revision
                .checked_next()
                .ok_or_else(|| AppError::internal("replenishment policy revision overflow"))
        },
    )?;
    let policy_id = insert_policy_tx(
        &mut tx,
        command,
        revision,
        predecessor.as_ref().map(|row| row.id),
        context.actor_id.get(),
        configured_at,
    )
    .await?;
    insert_sources_tx(&mut tx, access.tenant_id, policy_id, command).await?;

    let result = ConfigureReplenishmentPolicyResult {
        policy_id,
        definition: command.definition.clone(),
        status: ReplenishmentPolicyStatus::Active,
        previous_revision: predecessor.as_ref().map(|row| row.revision),
        revision,
        configured_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        configured_at,
    };
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        natural.inventory_owner_id,
        natural.facility_id,
        context.actor_id.get(),
        "replenishment_policy",
        policy_id.get(),
        "inventory.replenishment_policy.configured",
        &format!("configured:{}", revision.get()),
        &serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?,
        configured_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn retire_policy(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RetireReplenishmentPolicyCommand,
) -> AppResult<RetireReplenishmentPolicyResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, RETIRE_REPLENISHMENT_POLICY_OPERATION, command)?;
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
    require_stored_policy_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<RetireReplenishmentPolicyResult>(&mut tx)
        .await?
    {
        require_replayed_policy_visible_tx(&mut tx, access.tenant_id, result.policy_id, &scope)
            .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let hint_row = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, facility_id,
               pick_face_location_id, item_id, uom, minimum_qty, target_qty,
               revision, effective_from, effective_to
        FROM replenishment_policies
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.policy_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment policy"))?;
    let hint_sources = policy_sources_tx(&mut tx, access.tenant_id, command.policy_id).await?;
    let hint = policy_from_row(&hint_row, hint_sources)?;
    require_scope(
        &scope,
        hint.scope().inventory_owner_id.get(),
        hint.scope().facility_id.get(),
    )?;
    lock_natural_key_tx(&mut tx, hint.scope()).await?;

    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, facility_id,
               pick_face_location_id, item_id, uom, minimum_qty, target_qty,
               revision, effective_from, effective_to
        FROM replenishment_policies
        WHERE tenant_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.policy_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment policy"))?;
    let sources = policy_sources_tx(&mut tx, access.tenant_id, command.policy_id).await?;
    let policy = policy_from_row(&row, sources)?;
    require_scope(
        &scope,
        policy.scope().inventory_owner_id.get(),
        policy.scope().facility_id.get(),
    )?;
    if policy.effective_to.is_some() {
        return Err(AppError::conflict(
            "replenishment policy is already retired",
        ));
    }
    if policy.revision != command.expected_revision {
        return Err(AppError::conflict(
            "replenishment policy revision does not match expected revision",
        ));
    }
    ensure_no_active_work_tx(&mut tx, access.tenant_id, policy.id).await?;
    let retired_at = now_iso();
    retire_row_tx(
        &mut tx,
        access.tenant_id,
        policy.id,
        context.actor_id.get(),
        retired_at,
    )
    .await?;
    let result = RetireReplenishmentPolicyResult {
        policy_id: policy.id,
        scope: policy.scope().clone(),
        revision: policy.revision,
        status: ReplenishmentPolicyStatus::Retired,
        retired_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        retired_at,
    };
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        policy.scope().inventory_owner_id,
        policy.scope().facility_id,
        context.actor_id.get(),
        "replenishment_policy",
        policy.id.get(),
        "inventory.replenishment_policy.retired",
        &format!("retired:{}", policy.revision.get()),
        &serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?,
        retired_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub(super) async fn lock_natural_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &wareboxes_domain::ReplenishmentPolicyScope,
) -> AppResult<()> {
    sqlx::query(
        r#"SELECT pg_advisory_xact_lock(hashtextextended(
            concat_ws(':', 'replenishment_policy', $1, $2, $3, $4, $5, $6), 0
        ))"#,
    )
    .bind(scope.tenant_id.get())
    .bind(scope.inventory_owner_id.get())
    .bind(scope.facility_id.get())
    .bind(scope.pick_face_location_id.get())
    .bind(scope.item_id.get())
    .bind(scope.uom.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn validate_references_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &ConfigureReplenishmentPolicyCommand,
) -> AppResult<()> {
    let scope = command.definition.scope();
    let valid_assignment: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM inventory_owner_facilities assignment
            JOIN inventory_owner_items owner_item
              ON owner_item.tenant_id = assignment.tenant_id
             AND owner_item.inventory_owner_id = assignment.inventory_owner_id
             AND owner_item.item_id = $4
             AND owner_item.deleted IS NULL
            JOIN items item ON item.tenant_id=owner_item.tenant_id
             AND item.id=owner_item.item_id AND item.deleted IS NULL
            WHERE assignment.tenant_id = $1
              AND assignment.inventory_owner_id = $2
              AND assignment.facility_id = $3
              AND assignment.deleted IS NULL
              AND EXISTS (SELECT 1 FROM barcodes barcode
                WHERE barcode.tenant_id=item.tenant_id AND barcode.item_id=item.id
                  AND barcode.deleted IS NULL AND NULLIF(btrim(barcode.name),'') IS NOT NULL)
        )
        "#,
    )
    .bind(scope.tenant_id.get())
    .bind(scope.inventory_owner_id.get())
    .bind(scope.facility_id.get())
    .bind(scope.item_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if !valid_assignment {
        return Err(AppError::not_found("replenishment policy references"));
    }
    let mut location_ids = command
        .definition
        .reserve_source_location_ids()
        .as_slice()
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    location_ids.push(scope.pick_face_location_id.get());
    location_ids.sort_unstable();
    let rows = sqlx::query(
        r#"
        SELECT id, active, pickable, receivable,
               NULLIF(btrim(barcode), '') IS NOT NULL AS scannable
        FROM locations
        WHERE tenant_id = $1 AND facility_id = $2 AND id = ANY($3) AND deleted IS NULL
        ORDER BY id
        "#,
    )
    .bind(scope.tenant_id.get())
    .bind(scope.facility_id.get())
    .bind(&location_ids)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != location_ids.len() {
        return Err(AppError::not_found("replenishment policy location"));
    }
    for row in rows {
        let id: i64 = row.try_get("id")?;
        let active: bool = row.try_get("active")?;
        let pickable: bool = row.try_get("pickable")?;
        let receivable: bool = row.try_get("receivable")?;
        let scannable: bool = row.try_get("scannable")?;
        let eligible = if id == scope.pick_face_location_id.get() {
            active && scannable && pickable && !receivable
        } else {
            active && scannable && !pickable && !receivable
        };
        if !eligible {
            return Err(AppError::conflict(
                "replenishment policy location is not executable",
            ));
        }
    }
    Ok(())
}

fn validate_expected_revision(
    expected: Option<ReplenishmentPolicyRevision>,
    predecessor: Option<&PolicyRow>,
) -> AppResult<()> {
    match (expected, predecessor) {
        (None, None) => Ok(()),
        (Some(expected), Some(row)) if expected == row.revision => Ok(()),
        (None, Some(row)) if row.effective_to.is_some() => Ok(()),
        (None, Some(_)) => Err(AppError::conflict(
            "an active replenishment policy already exists",
        )),
        (Some(_), None) => Err(AppError::conflict(
            "replenishment policy expected revision does not exist",
        )),
        (Some(_), Some(_)) => Err(AppError::conflict(
            "replenishment policy revision does not match expected revision",
        )),
    }
}

async fn latest_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &ConfigureReplenishmentPolicyCommand,
) -> AppResult<Option<PolicyRow>> {
    let scope = command.definition.scope();
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, inventory_owner_id, facility_id,
               pick_face_location_id, item_id, uom, minimum_qty, target_qty,
               revision, effective_from, effective_to
        FROM replenishment_policies
        WHERE tenant_id = $1 AND inventory_owner_id = $2 AND facility_id = $3
          AND pick_face_location_id = $4 AND item_id = $5 AND uom = $6
        ORDER BY revision DESC
        LIMIT 1
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(scope.inventory_owner_id.get())
    .bind(scope.facility_id.get())
    .bind(scope.pick_face_location_id.get())
    .bind(scope.item_id.get())
    .bind(scope.uom.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let id = ReplenishmentPolicyId::new(row.try_get("id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let sources = policy_sources_tx(tx, tenant_id, id).await?;
    Ok(Some(policy_from_row(&row, sources)?))
}

async fn ensure_no_active_work_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ReplenishmentPolicyId,
) -> AppResult<()> {
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM replenishment_tasks WHERE tenant_id=$1 AND policy_id=$2 AND closed_at IS NULL)",
    )
    .bind(tenant_id.get())
    .bind(policy_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if active {
        Err(AppError::conflict(
            "replenishment policy has active inbound work",
        ))
    } else {
        Ok(())
    }
}

async fn retire_row_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ReplenishmentPolicyId,
    actor_id: i64,
    retired_at: Timestamp,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE replenishment_policies
        SET effective_to=$1, retired_by_user_id=$2
        WHERE tenant_id=$3 AND id=$4 AND effective_to IS NULL
        "#,
    )
    .bind(retired_at)
    .bind(actor_id)
    .bind(tenant_id.get())
    .bind(policy_id.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "replenishment policy is no longer active",
        ));
    }
    Ok(())
}

async fn insert_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &ConfigureReplenishmentPolicyCommand,
    revision: ReplenishmentPolicyRevision,
    predecessor: Option<ReplenishmentPolicyId>,
    actor_id: i64,
    configured_at: Timestamp,
) -> AppResult<ReplenishmentPolicyId> {
    let scope = command.definition.scope();
    let thresholds = command.definition.thresholds();
    let source_count = i64::try_from(
        command
            .definition
            .reserve_source_location_ids()
            .as_slice()
            .len(),
    )
    .map_err(|_| AppError::bad_request("too many replenishment source locations"))?;
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO replenishment_policies (
            tenant_id, inventory_owner_id, facility_id, pick_face_location_id,
            item_id, uom, minimum_qty, target_qty, revision, supersedes_policy_id,
            source_location_count, effective_from, configured_by_user_id, configured_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$12)
        RETURNING id
        "#,
    )
    .bind(scope.tenant_id.get())
    .bind(scope.inventory_owner_id.get())
    .bind(scope.facility_id.get())
    .bind(scope.pick_face_location_id.get())
    .bind(scope.item_id.get())
    .bind(scope.uom.as_str())
    .bind(thresholds.minimum().get())
    .bind(thresholds.target().get())
    .bind(revision.get())
    .bind(predecessor.map(|id| id.get()))
    .bind(source_count)
    .bind(configured_at)
    .bind(actor_id)
    .fetch_one(&mut **tx)
    .await?;
    ReplenishmentPolicyId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn insert_sources_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ReplenishmentPolicyId,
    command: &ConfigureReplenishmentPolicyCommand,
) -> AppResult<()> {
    let scope = command.definition.scope();
    for (index, location_id) in command
        .definition
        .reserve_source_location_ids()
        .as_slice()
        .iter()
        .enumerate()
    {
        sqlx::query(
            r#"
            INSERT INTO replenishment_policy_sources (
                tenant_id, inventory_owner_id, facility_id, policy_id,
                source_location_id, source_sequence
            ) VALUES ($1,$2,$3,$4,$5,$6)
            "#,
        )
        .bind(tenant_id.get())
        .bind(scope.inventory_owner_id.get())
        .bind(scope.facility_id.get())
        .bind(policy_id.get())
        .bind(location_id.get())
        .bind(
            index
                .checked_add(1)
                .and_then(|sequence| i64::try_from(sequence).ok())
                .ok_or_else(|| AppError::internal("source sequence overflow"))?,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn require_replayed_policy_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ReplenishmentPolicyId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        "SELECT inventory_owner_id, facility_id FROM replenishment_policies WHERE tenant_id=$1 AND id=$2",
    )
    .bind(tenant_id.get())
    .bind(policy_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("replenishment policy"))?;
    require_scope(
        scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )
}
