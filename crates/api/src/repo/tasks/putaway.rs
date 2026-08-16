use serde::{Deserialize, Serialize};
use sqlx::Row;
use wareboxes_application::{
    putaway::PutawayTaskCreation,
    putaway_policy::{PutawayPolicyExpectation, PutawayPolicyReadModel},
    CommandContext,
};
use wareboxes_core::models::{
    InventoryStatus, InventoryTransactionType, PutawayConfirmation, TenantAccess, Timestamp,
    WorkTaskType,
};
use wareboxes_domain::{FacilityId, InventoryOwnerId};

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::putaway_policy::{self, PutawayContent};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use super::{
    insert_progress_tx, insert_task_tx, lock_current_task_scope_tx,
    require_replayed_task_visible_tx, task_permission, task_timeout_seconds, NewWorkTask,
    TaskDimensions,
};

const CREATE_OPERATION: &str = "task.create_putaway.v2";
const CONFIRM_OPERATION: &str = "task.confirm_putaway.v2";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmPutawayOutcome {
    pub confirmation: PutawayConfirmation,
    pub putaway_policy: PutawayPolicyReadModel,
}

struct PutawaySource {
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    status: InventoryStatus,
    qty_on_hand: i64,
    qty_reserved: i64,
    qty_held: i64,
}

struct PutawayTarget {
    inventory_owner_id: i64,
    facility_id: i64,
    source_inventory_balance_id: i64,
    source_location_id: i64,
    destination_location_id: i64,
    item_batch_id: i64,
    item_id: i64,
    status: InventoryStatus,
    quantity: i64,
    putaway_policy: PutawayPolicyReadModel,
}

struct LockedBalance {
    id: i64,
    location_id: i64,
    uom: String,
    qty_on_hand: i64,
    qty_reserved: i64,
    qty_held: i64,
    active: bool,
}

fn parse_inventory_status(value: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {value}")))
}

fn validate_creation(
    source_inventory_balance_id: i64,
    destination_location_id: i64,
    quantity: i64,
    priority: i64,
    instructions: Option<&str>,
) -> AppResult<()> {
    if source_inventory_balance_id <= 0 {
        return Err(AppError::bad_request(
            "source inventory balance ID must be positive",
        ));
    }
    if destination_location_id <= 0 {
        return Err(AppError::bad_request(
            "destination location ID must be positive",
        ));
    }
    if quantity <= 0 {
        return Err(AppError::bad_request("putaway quantity must be positive"));
    }
    if priority < 0 {
        return Err(AppError::bad_request("putaway priority cannot be negative"));
    }
    if let Some(instructions) = instructions {
        if instructions.trim() != instructions || instructions.is_empty() {
            return Err(AppError::bad_request(
                "putaway instructions must be trimmed and nonempty",
            ));
        }
        if instructions.chars().count() > 1_000 {
            return Err(AppError::bad_request(
                "putaway instructions cannot exceed 1000 characters",
            ));
        }
    }
    Ok(())
}

fn validate_scanned_barcode(value: &str) -> AppResult<()> {
    if value.trim() != value || value.is_empty() {
        return Err(AppError::bad_request(
            "destination location barcode must be trimmed and nonempty",
        ));
    }
    if value.chars().count() > 200 {
        return Err(AppError::bad_request(
            "destination location barcode cannot exceed 200 characters",
        ));
    }
    Ok(())
}

async fn lock_source_for_creation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    source_inventory_balance_id: i64,
) -> AppResult<PutawaySource> {
    let row = sqlx::query(
        r#"
        SELECT balance.inventory_owner_id,
               balance.facility_id,
               balance.location_id,
               balance.item_batch_id,
               balance.item_id,
               balance.uom,
               balance.status,
               balance.qty_on_hand,
               balance.qty_reserved,
               balance.qty_held,
               balance.license_plate_id,
               source_location.receivable AS source_is_receivable
        FROM inventory_balances balance
        INNER JOIN locations source_location
          ON source_location.tenant_id = balance.tenant_id
         AND source_location.facility_id = balance.facility_id
         AND source_location.id = balance.location_id
        INNER JOIN item_batches batch
          ON batch.tenant_id = balance.tenant_id
         AND batch.inventory_owner_id = balance.inventory_owner_id
         AND batch.id = balance.item_batch_id
        WHERE balance.tenant_id = $1
          AND balance.id = $2
          AND balance.deleted IS NULL
          AND source_location.deleted IS NULL
          AND source_location.active
          AND batch.deleted IS NULL
        FOR UPDATE OF balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(source_inventory_balance_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("putaway source inventory"))?;

    if row.try_get::<Option<i64>, _>("license_plate_id")?.is_some() {
        return Err(AppError::conflict(
            "license plate inventory requires a container putaway workflow",
        ));
    }
    if !row.try_get::<bool, _>("source_is_receivable")? {
        return Err(AppError::conflict(
            "putaway source inventory must be in a receiving location",
        ));
    }
    let status = parse_inventory_status(&row.try_get::<String, _>("status")?)?;
    if status != InventoryStatus::Available {
        return Err(AppError::conflict(
            "only available inventory can enter putaway",
        ));
    }
    Ok(PutawaySource {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        status,
        qty_on_hand: row.try_get("qty_on_hand")?,
        qty_reserved: row.try_get("qty_reserved")?,
        qty_held: row.try_get("qty_held")?,
    })
}

async fn lock_destination(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    destination_location_id: i64,
    facility_id: i64,
) -> AppResult<String> {
    let destination: Option<String> = sqlx::query_scalar(
        r#"
        SELECT barcode
        FROM locations
        WHERE tenant_id = $1
          AND id = $2
          AND facility_id = $3
          AND deleted IS NULL
          AND active
          AND NOT receivable
          AND barcode IS NOT NULL
          AND btrim(barcode) <> ''
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(destination_location_id)
    .bind(facility_id)
    .fetch_optional(&mut **tx)
    .await?;
    destination.ok_or_else(|| {
        AppError::conflict(
            "putaway destination must be an active storage location in the source facility",
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn create_putaway_task_with_policy_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    source_inventory_balance_id: i64,
    destination_location_id: i64,
    quantity: i64,
    priority: i64,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<&str>,
    expected_policy: &PutawayPolicyExpectation,
) -> AppResult<PutawayTaskCreation> {
    command.require_actor(access.tenant_id, access.user_id)?;
    validate_creation(
        source_inventory_balance_id,
        destination_location_id,
        quantity,
        priority,
        instructions,
    )?;
    let prepared = PreparedCommand::new_v1(
        command,
        CREATE_OPERATION,
        &(
            source_inventory_balance_id,
            destination_location_id,
            quantity,
            priority,
            assigned_user_id,
            scheduled_for,
            due_at,
            instructions,
            expected_policy,
        ),
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_task_scope_tx(
        &mut tx,
        access.tenant_id,
        command.actor_id.get(),
        assigned_user_id,
    )
    .await?;

    if let Some(result) = prepared.replayed::<PutawayTaskCreation>(&mut tx).await? {
        require_replayed_task_visible_tx(&mut tx, access.tenant_id, result.task_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let source =
        lock_source_for_creation(&mut tx, access.tenant_id, source_inventory_balance_id).await?;
    let dimensions = TaskDimensions {
        facility_id: Some(source.facility_id),
        inventory_owner_id: Some(source.inventory_owner_id),
    };
    if !dimensions.is_allowed_by(&scope) {
        return Err(AppError::not_found("putaway source inventory"));
    }
    if source.location_id == destination_location_id {
        return Err(AppError::bad_request(
            "putaway source and destination locations must differ",
        ));
    }
    lock_destination(
        &mut tx,
        access.tenant_id,
        destination_location_id,
        source.facility_id,
    )
    .await?;
    inventory_journal::lock_active_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        inventory_journal::owner_facility_scope(source.inventory_owner_id, source.facility_id)?,
    )
    .await?;
    let owner_id = InventoryOwnerId::new(source.inventory_owner_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let facility_id = FacilityId::new(source.facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let putaway_policy = putaway_policy::resolve_putaway_policy_tx(
        &mut tx,
        access.tenant_id,
        owner_id,
        facility_id,
        now_iso(),
        true,
    )
    .await?;
    putaway_policy::require_expected_policy(&putaway_policy, expected_policy)?;
    let available = source
        .qty_on_hand
        .checked_sub(source.qty_reserved)
        .and_then(|quantity| quantity.checked_sub(source.qty_held))
        .ok_or_else(|| AppError::internal("inventory commitments are out of range"))?;
    if available < quantity {
        return Err(AppError::conflict(
            "insufficient uncommitted inventory for putaway",
        ));
    }
    putaway_policy::validate_destination_tx(
        &mut tx,
        access.tenant_id,
        owner_id,
        facility_id,
        destination_location_id,
        &[PutawayContent {
            item_id: source.item_id,
            item_batch_id: source.item_batch_id,
            uom: source.uom.clone(),
            quantity,
        }],
        &putaway_policy,
    )
    .await?;

    let existing_task: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT task_id
        FROM putaway_tasks
        WHERE tenant_id = $1
          AND source_inventory_balance_id = $2
          AND closed_at IS NULL
        LIMIT 1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(source_inventory_balance_id)
    .fetch_optional(&mut *tx)
    .await?;
    if existing_task.is_some() {
        return Err(AppError::conflict(
            "source inventory already has active putaway work",
        ));
    }

    let task_id = insert_task_tx(
        &mut tx,
        access.tenant_id,
        NewWorkTask {
            facility_id: Some(source.facility_id),
            inventory_owner_id: Some(source.inventory_owner_id),
            task_type: WorkTaskType::Putaway,
            title: "Put away received inventory".to_owned(),
            instructions: instructions.map(str::to_owned),
            required_permission: task_permission(WorkTaskType::Putaway).to_owned(),
            priority,
            task_timeout_seconds: task_timeout_seconds(WorkTaskType::Putaway),
            assigned_user_id,
            created_by: Some(command.actor_id.get()),
            scheduled_for,
            due_at,
            metadata_json: None,
        },
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO putaway_tasks (
            tenant_id,
            task_id,
            inventory_owner_id,
            facility_id,
            source_inventory_balance_id,
            source_location_id,
            destination_location_id,
            item_batch_id,
            item_id,
            inventory_status,
            planned_quantity,
            putaway_policy_source,
            putaway_policy_configuration_id,
            putaway_policy_configuration_revision,
            putaway_policy_scope_level,
            putaway_policy_scope_owner_id,
            putaway_policy_scope_facility_id,
            putaway_policy_definition,
            putaway_policy_hash
        )
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(source.inventory_owner_id)
    .bind(source.facility_id)
    .bind(source_inventory_balance_id)
    .bind(source.location_id)
    .bind(destination_location_id)
    .bind(source.item_batch_id)
    .bind(source.item_id)
    .bind(source.status.as_str())
    .bind(quantity)
    .bind(putaway_policy::source_text(putaway_policy.source))
    .bind(putaway_policy.configuration_id.map(|id| id.get()))
    .bind(putaway_policy.configuration_revision)
    .bind(putaway_policy::scope_values(putaway_policy.configuration_scope).0)
    .bind(putaway_policy::scope_values(putaway_policy.configuration_scope).1)
    .bind(putaway_policy::scope_values(putaway_policy.configuration_scope).2)
    .bind(putaway_policy::definition_json(&putaway_policy))
    .bind(&putaway_policy.policy_hash)
    .execute(&mut *tx)
    .await?;

    let result = PutawayTaskCreation {
        task_id,
        putaway_policy,
    };
    Ok(prepared.commit(tx, result).await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_putaway_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    source_inventory_balance_id: i64,
    destination_location_id: i64,
    quantity: i64,
    priority: i64,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<&str>,
) -> AppResult<i64> {
    let expected = PutawayPolicyReadModel::product_default().expectation();
    Ok(create_putaway_task_with_policy_in_scope(
        db,
        access,
        command,
        source_inventory_balance_id,
        destination_location_id,
        quantity,
        priority,
        assigned_user_id,
        scheduled_for,
        due_at,
        instructions,
        &expected,
    )
    .await?
    .task_id)
}

async fn lock_putaway_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    actor_user_id: i64,
    scope: &ScopeBindings,
) -> AppResult<PutawayTarget> {
    let row = sqlx::query(
        r#"
        SELECT task.status,
               task.assigned_user_id,
               task.lease_expires_at > statement_timestamp() AS lease_is_current,
               detail.inventory_owner_id,
               detail.facility_id,
               detail.source_inventory_balance_id,
               detail.source_location_id,
               detail.destination_location_id,
               detail.item_batch_id,
               detail.item_id,
               detail.inventory_status,
               detail.planned_quantity,
               detail.putaway_policy_source,
               detail.putaway_policy_configuration_id,
               detail.putaway_policy_configuration_revision,
               detail.putaway_policy_scope_level,
               detail.putaway_policy_scope_owner_id,
               detail.putaway_policy_scope_facility_id,
               detail.putaway_policy_definition,
               detail.putaway_policy_hash,
               detail.closed_at
        FROM work_tasks task
        INNER JOIN putaway_tasks detail
          ON detail.tenant_id = task.tenant_id
         AND detail.task_id = task.id
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.deleted IS NULL
          AND task.task_type = 'putaway'
        FOR UPDATE OF task
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("putaway task"))?;
    let putaway_policy = putaway_policy::frozen_policy(&row)?;
    let target = PutawayTarget {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        source_inventory_balance_id: row.try_get("source_inventory_balance_id")?,
        source_location_id: row.try_get("source_location_id")?,
        destination_location_id: row.try_get("destination_location_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        status: parse_inventory_status(&row.try_get::<String, _>("inventory_status")?)?,
        quantity: row.try_get("planned_quantity")?,
        putaway_policy,
    };
    let dimensions = TaskDimensions {
        facility_id: Some(target.facility_id),
        inventory_owner_id: Some(target.inventory_owner_id),
    };
    if !dimensions.is_allowed_by(scope) {
        return Err(AppError::not_found("putaway task"));
    }
    let status: String = row.try_get("status")?;
    let assigned_user_id: Option<i64> = row.try_get("assigned_user_id")?;
    let lease_is_current: Option<bool> = row.try_get("lease_is_current")?;
    let closed_at: Option<Timestamp> = row.try_get("closed_at")?;
    if status != "in_progress"
        || assigned_user_id != Some(actor_user_id)
        || lease_is_current != Some(true)
        || closed_at.is_some()
    {
        return Err(AppError::conflict(
            "putaway task does not have an active claim for this operator",
        ));
    }
    Ok(target)
}

async fn lock_putaway_balances(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    target: &PutawayTarget,
) -> AppResult<Vec<LockedBalance>> {
    let rows = sqlx::query(
        r#"
        SELECT id,
               location_id,
               uom,
               qty_on_hand,
               qty_reserved,
               qty_held,
               deleted IS NULL AS active
        FROM inventory_balances
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND facility_id = $3
          AND license_plate_id IS NULL
          AND (
              id = $4
              OR (
                  location_id = $5
                  AND item_batch_id = $6
                  AND item_id = $7
                  AND status = $8
              )
          )
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(target.source_inventory_balance_id)
    .bind(target.destination_location_id)
    .bind(target.item_batch_id)
    .bind(target.item_id)
    .bind(target.status.as_str())
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(LockedBalance {
                id: row.try_get("id")?,
                location_id: row.try_get("location_id")?,
                uom: row.try_get("uom")?,
                qty_on_hand: row.try_get("qty_on_hand")?,
                qty_reserved: row.try_get("qty_reserved")?,
                qty_held: row.try_get("qty_held")?,
                active: row.try_get("active")?,
            })
        })
        .collect()
}

pub async fn confirm_putaway_with_policy_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    scanned_destination_location_barcode: &str,
    expected_policy: &PutawayPolicyExpectation,
) -> AppResult<ConfirmPutawayOutcome> {
    command.require_actor(access.tenant_id, access.user_id)?;
    if task_id <= 0 {
        return Err(AppError::bad_request("putaway task ID must be positive"));
    }
    validate_scanned_barcode(scanned_destination_location_barcode)?;
    let prepared = PreparedCommand::new_v1(
        command,
        CONFIRM_OPERATION,
        &(
            task_id,
            scanned_destination_location_barcode,
            expected_policy,
        ),
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;

    if let Some(result) = prepared.replayed::<ConfirmPutawayOutcome>(&mut tx).await? {
        require_replayed_task_visible_tx(&mut tx, access.tenant_id, task_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let target =
        lock_putaway_target(&mut tx, access, task_id, command.actor_id.get(), &scope).await?;
    putaway_policy::require_expected_policy(&target.putaway_policy, expected_policy)?;
    let destination_location_barcode = lock_destination(
        &mut tx,
        access.tenant_id,
        target.destination_location_id,
        target.facility_id,
    )
    .await?;
    if destination_location_barcode != scanned_destination_location_barcode {
        return Err(AppError::conflict(
            "scanned destination does not match the directed putaway location",
        ));
    }
    let balances = lock_putaway_balances(&mut tx, access.tenant_id, &target).await?;
    let source = balances
        .iter()
        .find(|balance| balance.id == target.source_inventory_balance_id && balance.active)
        .ok_or_else(|| AppError::conflict("putaway source inventory is no longer active"))?;
    if source.location_id != target.source_location_id {
        return Err(AppError::conflict(
            "putaway source inventory no longer matches the task",
        ));
    }
    let available = source
        .qty_on_hand
        .checked_sub(source.qty_reserved)
        .and_then(|quantity| quantity.checked_sub(source.qty_held))
        .ok_or_else(|| AppError::internal("inventory commitments are out of range"))?;
    if available < target.quantity {
        return Err(AppError::conflict(
            "insufficient uncommitted inventory for putaway",
        ));
    }

    let owner_facility =
        inventory_journal::owner_facility_scope(target.inventory_owner_id, target.facility_id)?;
    let transaction_id = inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: command.actor_id.get(),
            transaction_type: InventoryTransactionType::Move,
            reason: Some("directed putaway confirmation"),
            reference_type: Some("putaway_task"),
            reference_id: Some(task_id),
            correlation_id: Some(&command.request_id),
            operation: CONFIRM_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
    )
    .await?;

    putaway_policy::validate_destination_tx(
        &mut tx,
        access.tenant_id,
        InventoryOwnerId::new(target.inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        FacilityId::new(target.facility_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        target.destination_location_id,
        &[PutawayContent {
            item_id: target.item_id,
            item_batch_id: target.item_batch_id,
            uom: source.uom.clone(),
            quantity: target.quantity,
        }],
        &target.putaway_policy,
    )
    .await?;
    let confirmed_at = now_iso();
    let source_update = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET qty_on_hand = qty_on_hand - $1,
            modified = $2
        WHERE tenant_id = $3
          AND inventory_owner_id = $4
          AND facility_id = $5
          AND id = $6
          AND location_id = $7
          AND item_batch_id = $8
          AND item_id = $9
          AND status = $10
          AND license_plate_id IS NULL
          AND deleted IS NULL
          AND qty_on_hand - qty_reserved - qty_held >= $1
        "#,
    )
    .bind(target.quantity)
    .bind(confirmed_at)
    .bind(access.tenant_id.get())
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(target.source_inventory_balance_id)
    .bind(target.source_location_id)
    .bind(target.item_batch_id)
    .bind(target.item_id)
    .bind(target.status.as_str())
    .execute(&mut *tx)
    .await?;
    if source_update.rows_affected() != 1 {
        return Err(AppError::conflict(
            "putaway source inventory changed during confirmation",
        ));
    }

    let destination_inventory_balance_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_balances (
            tenant_id,
            inventory_owner_id,
            created,
            modified,
            facility_id,
            location_id,
            license_plate_id,
            item_batch_id,
            item_id,
            uom,
            status,
            qty_on_hand,
            qty_reserved
        )
        VALUES ($1, $2, $3, $3, $4, $5, NULL, $6, $7, $8, $9, $10, 0)
        ON CONFLICT (
            tenant_id,
            inventory_owner_id,
            location_id,
            item_batch_id,
            uom,
            status
        ) WHERE license_plate_id IS NULL
        DO UPDATE SET
            qty_on_hand = inventory_balances.qty_on_hand + excluded.qty_on_hand,
            modified = excluded.modified,
            deleted = NULL
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(target.inventory_owner_id)
    .bind(confirmed_at)
    .bind(target.facility_id)
    .bind(target.destination_location_id)
    .bind(target.item_batch_id)
    .bind(target.item_id)
    .bind(&source.uom)
    .bind(target.status.as_str())
    .bind(target.quantity)
    .fetch_one(&mut *tx)
    .await?;

    for (location_id, quantity_delta) in [
        (target.source_location_id, -target.quantity),
        (target.destination_location_id, target.quantity),
    ] {
        inventory_journal::append_entry(
            &mut tx,
            access.tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id,
                license_plate_id: None,
                item_batch_id: target.item_batch_id,
                status: target.status,
                quantity_delta,
            },
        )
        .await?;
    }

    let result = PutawayConfirmation {
        tenant_id: access.tenant_id,
        task_id,
        inventory_owner_id: InventoryOwnerId::new(target.inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: target.facility_id,
        source_inventory_balance_id: target.source_inventory_balance_id,
        destination_inventory_balance_id,
        source_location_id: target.source_location_id,
        destination_location_id: target.destination_location_id,
        destination_location_barcode,
        item_batch_id: target.item_batch_id,
        item_id: target.item_id,
        inventory_status: target.status,
        quantity: target.quantity,
        inventory_transaction_id: transaction_id,
        confirmed_by: command.actor_id.get(),
        confirmed_at,
    };
    sqlx::query(
        r#"
        INSERT INTO putaway_results (
            tenant_id,
            task_id,
            inventory_owner_id,
            facility_id,
            source_inventory_balance_id,
            destination_inventory_balance_id,
            source_location_id,
            destination_location_id,
            destination_location_barcode,
            item_batch_id,
            item_id,
            inventory_status,
            quantity,
            inventory_transaction_id,
            confirmed_by,
            confirmed_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
            $14, $15, $16
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(target.source_inventory_balance_id)
    .bind(destination_inventory_balance_id)
    .bind(target.source_location_id)
    .bind(target.destination_location_id)
    .bind(&result.destination_location_barcode)
    .bind(target.item_batch_id)
    .bind(target.item_id)
    .bind(target.status.as_str())
    .bind(target.quantity)
    .bind(transaction_id)
    .bind(command.actor_id.get())
    .bind(confirmed_at)
    .execute(&mut *tx)
    .await?;

    let completed = sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'completed',
            completed_by = $1,
            completed_at = $2,
            lease_expires_at = NULL,
            modified = $2
        WHERE tenant_id = $3
          AND id = $4
          AND deleted IS NULL
          AND status = 'in_progress'
          AND assigned_user_id = $1
          AND lease_expires_at > statement_timestamp()
        "#,
    )
    .bind(command.actor_id.get())
    .bind(confirmed_at)
    .bind(access.tenant_id.get())
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    if completed.rows_affected() != 1 {
        return Err(AppError::conflict(
            "putaway task claim expired during confirmation",
        ));
    }
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        task_id,
        None,
        Some(command.actor_id.get()),
        "putaway_confirmed",
        Some(target.quantity),
        Some(target.source_location_id),
        Some(target.destination_location_id),
        None,
        None,
    )
    .await?;

    let inventory_owner_id = result.inventory_owner_id;
    let facility_id = FacilityId::new(target.facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let event_key = format!("putaway-confirmation:{task_id}");
    let aggregate_id = task_id.to_string();
    let payload = serde_json::json!({
        "task_id": task_id,
        "inventory_transaction_id": transaction_id,
        "inventory_owner_id": target.inventory_owner_id,
        "facility_id": target.facility_id,
        "source_inventory_balance_id": target.source_inventory_balance_id,
        "destination_inventory_balance_id": destination_inventory_balance_id,
        "source_location_id": target.source_location_id,
        "destination_location_id": target.destination_location_id,
        "item_batch_id": target.item_batch_id,
        "item_id": target.item_id,
        "inventory_status": target.status.as_str(),
        "quantity": target.quantity,
        "putaway_policy": &target.putaway_policy,
    });
    outbox::enqueue(
        &mut tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(command.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "putaway_confirmation",
            aggregate_id: &aggregate_id,
            ordering_key: &event_key,
            aggregate_sequence: 1,
            event_type: "inventory.putaway.confirmed",
            schema_version: 2,
            payload: &payload,
            occurred_at: confirmed_at,
        },
    )
    .await?;

    let outcome = ConfirmPutawayOutcome {
        confirmation: result,
        putaway_policy: target.putaway_policy,
    };
    Ok(prepared
        .commit_with_inventory_transaction(tx, outcome, Some(transaction_id))
        .await?)
}

pub async fn confirm_putaway_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    scanned_destination_location_barcode: &str,
) -> AppResult<PutawayConfirmation> {
    let expected = PutawayPolicyReadModel::product_default().expectation();
    Ok(confirm_putaway_with_policy_in_scope(
        db,
        access,
        command,
        task_id,
        scanned_destination_location_barcode,
        &expected,
    )
    .await?
    .confirmation)
}
