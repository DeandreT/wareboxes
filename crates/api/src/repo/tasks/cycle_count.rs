use sqlx::Row;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{
    InventoryStatus, InventoryTransactionType, ItemLocationCycleCountConfirmation, TenantAccess,
};
use wareboxes_domain::{FacilityId, InventoryOwnerId};

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::lock_current_scope_tx;
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::inventory_locking::{balance_license_plate_hint, lock_license_plate};
use wareboxes_application::count_decision_policy::{
    CountDecisionPolicyReadModel, CountDecisionPolicySource,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_core::models::{
    CycleCountDecisionPolicySnapshot, CycleCountDecisionPolicySource as CoreCountPolicySource,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use super::{insert_progress_tx, require_replayed_task_visible_tx, TaskDimensions};

const OPERATION: &str = "task.confirm_item_location_cycle_count.v1";

#[derive(serde::Serialize)]
struct CountScans<'a> {
    location_barcode: &'a str,
    item_barcode: &'a str,
    license_plate_barcode: Option<&'a str>,
}

pub(super) struct CountTarget {
    pub(super) task_id: i64,
    pub(super) inventory_owner_id: i64,
    pub(super) facility_id: i64,
    pub(super) location_id: i64,
    pub(super) item_id: i64,
    pub(super) inventory_balance_id: i64,
    pub(super) variance_id: Option<wareboxes_domain::CycleCountVarianceId>,
    pub(super) attempt_sequence: u16,
}

pub(super) struct LockedBalance {
    pub(super) item_batch_id: i64,
    pub(super) license_plate_id: Option<i64>,
    pub(super) uom: String,
    pub(super) lot: Option<String>,
    pub(super) expiration: Option<wareboxes_core::models::Timestamp>,
    pub(super) serial: Option<String>,
    pub(super) status: InventoryStatus,
    pub(super) qty_on_hand: i64,
    pub(super) qty_reserved: i64,
    pub(super) qty_held: i64,
}

fn validated_note(note: Option<&str>) -> AppResult<Option<&str>> {
    let Some(note) = note else {
        return Ok(None);
    };
    if note.trim() != note || note.is_empty() {
        return Err(AppError::bad_request(
            "cycle count note must be trimmed and nonempty",
        ));
    }
    if note.chars().count() > 1000 {
        return Err(AppError::bad_request(
            "cycle count note cannot exceed 1000 characters",
        ));
    }
    Ok(Some(note))
}

fn parse_inventory_status(value: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {value}")))
}

async fn lock_task_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    task_id: i64,
    actor_user_id: i64,
    scope: &crate::repo::access::ScopeBindings,
) -> AppResult<CountTarget> {
    let row = sqlx::query(
        r#"
        SELECT task.status,
               task.assigned_user_id,
               task.lease_expires_at > statement_timestamp() AS lease_is_current,
               detail.inventory_owner_id,
               detail.facility_id,
               detail.location_id,
               detail.item_id,
               detail.inventory_balance_id,
               detail.variance_case_id,
               detail.attempt_sequence
        FROM work_tasks task
        INNER JOIN cycle_count_item_location_tasks detail
          ON detail.tenant_id = task.tenant_id
         AND detail.task_id = task.id
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.deleted IS NULL
          AND task.task_type = 'cycle_count_item_location'
        FOR UPDATE OF task, detail
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("cycle count task"))?;

    let target = CountTarget {
        task_id,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        item_id: row.try_get("item_id")?,
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        variance_id: row
            .try_get::<Option<i64>, _>("variance_case_id")?
            .map(wareboxes_domain::CycleCountVarianceId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        attempt_sequence: u16::try_from(row.try_get::<i16, _>("attempt_sequence")?)
            .map_err(|_| AppError::internal("cycle count attempt sequence is invalid"))?,
    };
    let dimensions = TaskDimensions {
        facility_id: Some(target.facility_id),
        inventory_owner_id: Some(target.inventory_owner_id),
    };
    if !dimensions.is_allowed_by(scope) {
        return Err(AppError::not_found("cycle count task"));
    }

    let status: String = row.try_get("status")?;
    let assigned_user_id: Option<i64> = row.try_get("assigned_user_id")?;
    let lease_is_current: Option<bool> = row.try_get("lease_is_current")?;
    if status != "in_progress"
        || assigned_user_id != Some(actor_user_id)
        || lease_is_current != Some(true)
    {
        return Err(AppError::conflict(
            "cycle count task does not have an active claim for this operator",
        ));
    }
    Ok(target)
}

async fn lock_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    target: &CountTarget,
) -> AppResult<LockedBalance> {
    let row = sqlx::query(
        r#"
        SELECT balance.inventory_owner_id,
               balance.facility_id,
               balance.location_id,
               balance.item_id,
               balance.item_batch_id,
               balance.license_plate_id,
               balance.uom,
               batch.lot,
               batch.expiration,
               batch.serial,
               balance.status,
               balance.qty_on_hand,
               balance.qty_reserved,
               balance.qty_held
        FROM inventory_balances balance
        INNER JOIN item_batches batch
          ON batch.tenant_id = balance.tenant_id
         AND batch.inventory_owner_id = balance.inventory_owner_id
         AND batch.id = balance.item_batch_id
        WHERE balance.tenant_id = $1
          AND balance.id = $2
          AND balance.deleted IS NULL
          AND batch.deleted IS NULL
        FOR UPDATE OF balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_balance_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("cycle count inventory balance is no longer active"))?;

    if row.try_get::<i64, _>("inventory_owner_id")? != target.inventory_owner_id
        || row.try_get::<i64, _>("facility_id")? != target.facility_id
        || row.try_get::<i64, _>("location_id")? != target.location_id
        || row.try_get::<i64, _>("item_id")? != target.item_id
    {
        return Err(AppError::conflict(
            "cycle count inventory balance no longer matches the task target",
        ));
    }

    Ok(LockedBalance {
        item_batch_id: row.try_get("item_batch_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        expiration: row.try_get("expiration")?,
        serial: row.try_get("serial")?,
        status: parse_inventory_status(&row.try_get::<String, _>("status")?)?,
        qty_on_hand: row.try_get("qty_on_hand")?,
        qty_reserved: row.try_get("qty_reserved")?,
        qty_held: row.try_get("qty_held")?,
    })
}

pub async fn confirm_item_location_cycle_count_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    counted_quantity: i64,
    note: Option<&str>,
) -> AppResult<ItemLocationCycleCountConfirmation> {
    confirm_item_location_cycle_count_with_scans_in_scope(
        db,
        access,
        command,
        task_id,
        counted_quantity,
        note,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn confirm_scanned_item_location_cycle_count_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    location_barcode: &str,
    item_barcode: &str,
    license_plate_barcode: Option<&str>,
    counted_quantity: i64,
    note: Option<&str>,
) -> AppResult<ItemLocationCycleCountConfirmation> {
    confirm_item_location_cycle_count_with_scans_in_scope(
        db,
        access,
        command,
        task_id,
        counted_quantity,
        note,
        Some(CountScans {
            location_barcode,
            item_barcode,
            license_plate_barcode,
        }),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn confirm_item_location_cycle_count_with_scans_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    counted_quantity: i64,
    note: Option<&str>,
    scans: Option<CountScans<'_>>,
) -> AppResult<ItemLocationCycleCountConfirmation> {
    command.require_actor(access.tenant_id, access.user_id)?;
    if task_id <= 0 {
        return Err(AppError::bad_request("task ID must be positive"));
    }
    if counted_quantity < 0 {
        return Err(AppError::bad_request("counted quantity cannot be negative"));
    }
    let note = validated_note(note)?;
    let prepared = PreparedCommand::new_v1(
        command,
        OPERATION,
        &(task_id, counted_quantity, note, &scans),
    )?;

    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;

    if let Some(result) = prepared
        .replayed::<ItemLocationCycleCountConfirmation>(&mut tx)
        .await?
    {
        require_replayed_task_visible_tx(&mut tx, access.tenant_id, task_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let target = lock_task_target(
        &mut tx,
        access.tenant_id,
        task_id,
        command.actor_id.get(),
        &scope,
    )
    .await?;
    let license_plate_id =
        balance_license_plate_hint(&mut tx, access.tenant_id, target.inventory_balance_id).await?;
    lock_license_plate(&mut tx, access.tenant_id, license_plate_id).await?;
    let balance = lock_balance(&mut tx, access.tenant_id, &target).await?;
    if let Some(scans) = scans {
        validate_scans_tx(&mut tx, access.tenant_id, &target, &balance, scans).await?;
    }
    let committed_quantity = balance
        .qty_reserved
        .checked_add(balance.qty_held)
        .ok_or_else(|| AppError::internal("inventory commitments are out of range"))?;
    if counted_quantity < committed_quantity {
        return Err(AppError::conflict(
            "counted quantity cannot be lower than reserved and held quantity",
        ));
    }
    let variance_quantity = counted_quantity
        .checked_sub(balance.qty_on_hand)
        .ok_or_else(|| AppError::bad_request("cycle count variance is out of range"))?;
    let confirmed_at = now_iso();
    let control = super::prepare_count_control_tx(
        &mut tx,
        access.tenant_id,
        command.actor_id.get(),
        task_id,
        &target,
        &balance,
        counted_quantity,
        variance_quantity,
        confirmed_at,
    )
    .await?;

    let inventory_transaction_id = if variance_quantity == 0
        || control.disposition != wareboxes_domain::CycleCountDisposition::Posted
    {
        None
    } else {
        let owner_facility =
            inventory_journal::owner_facility_scope(target.inventory_owner_id, target.facility_id)?;
        let transaction_id = inventory_journal::begin_transaction(
            &mut tx,
            &JournalCommand {
                tenant_id: access.tenant_id,
                owner_facility,
                actor_user_id: command.actor_id.get(),
                transaction_type: InventoryTransactionType::Adjust,
                reason: Some("cycle count confirmation"),
                reference_type: Some("cycle_count_item_location_task"),
                reference_id: Some(task_id),
                correlation_id: Some(&command.request_id),
                operation: OPERATION,
                idempotency_key: Some(prepared.idempotency_key()),
                request_hash: prepared.request_hash(),
            },
        )
        .await?;

        inventory_journal::append_entry(
            &mut tx,
            access.tenant_id,
            owner_facility,
            transaction_id,
            &JournalEntry {
                location_id: target.location_id,
                license_plate_id: balance.license_plate_id,
                item_batch_id: balance.item_batch_id,
                status: balance.status,
                quantity_delta: variance_quantity,
            },
        )
        .await?;

        let updated = sqlx::query(
            r#"
            UPDATE inventory_balances
            SET qty_on_hand = $1,
                modified = $2
            WHERE tenant_id = $3
              AND inventory_owner_id = $4
              AND id = $5
              AND deleted IS NULL
              AND qty_on_hand = $6
              AND qty_reserved = $7
              AND qty_held = $8
            "#,
        )
        .bind(counted_quantity)
        .bind(confirmed_at)
        .bind(access.tenant_id.get())
        .bind(target.inventory_owner_id)
        .bind(target.inventory_balance_id)
        .bind(balance.qty_on_hand)
        .bind(balance.qty_reserved)
        .bind(balance.qty_held)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict(
                "cycle count inventory balance changed during confirmation",
            ));
        }
        Some(transaction_id)
    };

    let mut confirmation = ItemLocationCycleCountConfirmation {
        tenant_id: access.tenant_id,
        task_id,
        inventory_owner_id: InventoryOwnerId::new(target.inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: target.facility_id,
        location_id: target.location_id,
        inventory_balance_id: target.inventory_balance_id,
        license_plate_id: balance.license_plate_id,
        item_batch_id: balance.item_batch_id,
        item_id: target.item_id,
        uom: balance.uom.clone(),
        lot: balance.lot.clone(),
        expiration: balance.expiration,
        serial: balance.serial.clone(),
        inventory_status: balance.status,
        previous_on_hand_quantity: balance.qty_on_hand,
        reserved_quantity: balance.qty_reserved,
        held_quantity: balance.qty_held,
        counted_quantity,
        variance_quantity,
        inventory_transaction_id,
        disposition: control.disposition,
        decision_policy: control
            .decision_policy
            .as_ref()
            .map(core_decision_policy_snapshot),
        variance_id: control.variance_id,
        variance_revision: control.variance_revision,
        next_recount_task_id: None,
        confirmed_by: command.actor_id.get(),
        confirmed_at,
        note: note.map(str::to_owned),
    };

    sqlx::query(
        r#"
        INSERT INTO cycle_count_item_location_results (
            tenant_id, task_id, inventory_owner_id, facility_id, location_id,
            item_id, inventory_balance_id, item_batch_id, license_plate_id, uom,
            lot, expiration, serial, status, system_qty_on_hand,
            system_qty_reserved, system_qty_held, counted_qty, variance_qty,
            inventory_transaction_id, confirmed_by, confirmed_at, note,
            disposition, variance_case_id, attempt_sequence, policy_id,
            policy_revision, absolute_tolerance_qty, percentage_tolerance_bps,
            automatic_recount_limit, allowed_variance_qty
            , count_policy_source, count_configuration_id,
            count_configuration_revision, count_scope_level,
            count_inventory_owner_id, count_facility_id,
            count_absolute_tolerance_qty, count_percentage_tolerance_bps,
            count_approval_threshold_qty, count_policy_hash
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
            $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26,
            $27, $28, $29, $30, $31, $32, $33, $34, $35, $36, $37,
            $38, $39, $40, $41, $42
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(target.location_id)
    .bind(target.item_id)
    .bind(target.inventory_balance_id)
    .bind(balance.item_batch_id)
    .bind(balance.license_plate_id)
    .bind(&confirmation.uom)
    .bind(&confirmation.lot)
    .bind(confirmation.expiration)
    .bind(&confirmation.serial)
    .bind(confirmation.inventory_status.as_str())
    .bind(confirmation.previous_on_hand_quantity)
    .bind(confirmation.reserved_quantity)
    .bind(confirmation.held_quantity)
    .bind(confirmation.counted_quantity)
    .bind(confirmation.variance_quantity)
    .bind(confirmation.inventory_transaction_id)
    .bind(confirmation.confirmed_by)
    .bind(confirmation.confirmed_at)
    .bind(&confirmation.note)
    .bind(match control.disposition {
        wareboxes_domain::CycleCountDisposition::Posted => "posted",
        wareboxes_domain::CycleCountDisposition::RecountRequired => "recount_required",
        wareboxes_domain::CycleCountDisposition::ApprovalRequired => "approval_required",
    })
    .bind(control.variance_id.map(|id| id.get()))
    .bind(
        i16::try_from(control.attempt_sequence)
            .map_err(|_| AppError::internal("cycle count attempt is out of database range"))?,
    )
    .bind(control.policy.map(|policy| policy.id.get()))
    .bind(control.policy.map(|policy| policy.revision.get()))
    .bind(
        control
            .decision_policy
            .as_ref()
            .map(|policy| policy.absolute_tolerance_quantity),
    )
    .bind(
        control
            .decision_policy
            .as_ref()
            .map(|policy| {
                i32::try_from(policy.percentage_tolerance_basis_points).map_err(|_| {
                    AppError::internal("cycle count percentage is out of database range")
                })
            })
            .transpose()?,
    )
    .bind(
        control
            .policy
            .map(|policy| {
                i16::try_from(policy.policy.automatic_recount_limit()).map_err(|_| {
                    AppError::internal("cycle count recount limit is out of database range")
                })
            })
            .transpose()?,
    )
    .bind(control.allowed_variance_quantity)
    .bind(
        control
            .decision_policy
            .as_ref()
            .map(|policy| policy.source.as_str()),
    )
    .bind(
        control
            .decision_policy
            .as_ref()
            .and_then(|policy| policy.configuration_id)
            .map(wareboxes_domain::ConfigurationVersionId::get),
    )
    .bind(
        control
            .decision_policy
            .as_ref()
            .and_then(|policy| policy.configuration_revision),
    )
    .bind(
        control
            .decision_policy
            .as_ref()
            .and_then(|policy| policy.configuration_scope)
            .map(configuration_scope_level),
    )
    .bind(
        control
            .decision_policy
            .as_ref()
            .and_then(|policy| policy.configuration_scope)
            .and_then(configuration_scope_owner),
    )
    .bind(
        control
            .decision_policy
            .as_ref()
            .and_then(|policy| policy.configuration_scope)
            .and_then(configuration_scope_facility),
    )
    .bind(
        control
            .decision_policy
            .as_ref()
            .map(|policy| policy.absolute_tolerance_quantity),
    )
    .bind(
        control
            .decision_policy
            .as_ref()
            .map(|policy| i32::try_from(policy.percentage_tolerance_basis_points))
            .transpose()
            .map_err(|_| AppError::internal("Count percentage is out of database range"))?,
    )
    .bind(
        control
            .decision_policy
            .as_ref()
            .and_then(|policy| policy.approval_threshold_quantity),
    )
    .bind(
        control
            .decision_policy
            .as_ref()
            .map(|policy| policy.policy_hash.as_str()),
    )
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
            "cycle count task claim expired during confirmation",
        ));
    }

    let advanced = super::advance_count_control_after_confirmation_tx(
        &mut tx,
        access.tenant_id,
        command.actor_id.get(),
        &target,
        &balance,
        counted_quantity,
        variance_quantity,
        inventory_transaction_id,
        control,
        confirmed_at,
    )
    .await?;
    confirmation.variance_revision = advanced.variance_revision;
    confirmation.next_recount_task_id = advanced.next_recount_task_id;

    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        task_id,
        None,
        Some(command.actor_id.get()),
        "cycle_count_confirmed",
        None,
        None,
        None,
        note,
        None,
    )
    .await?;

    let inventory_owner_id = confirmation.inventory_owner_id;
    let facility_id = FacilityId::new(target.facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let event_key = format!("cycle-count-confirmation:{task_id}");
    let aggregate_id = task_id.to_string();
    let payload = serde_json::json!({
        "task_id": task_id,
        "inventory_owner_id": target.inventory_owner_id,
        "facility_id": target.facility_id,
        "location_id": target.location_id,
        "inventory_balance_id": target.inventory_balance_id,
        "item_batch_id": balance.item_batch_id,
        "item_id": target.item_id,
        "license_plate_id": balance.license_plate_id,
        "status": balance.status.as_str(),
        "previous_on_hand_quantity": balance.qty_on_hand,
        "reserved_quantity": balance.qty_reserved,
        "held_quantity": balance.qty_held,
        "counted_quantity": counted_quantity,
        "variance_quantity": variance_quantity,
        "inventory_transaction_id": inventory_transaction_id,
        "disposition": match confirmation.disposition {
            wareboxes_domain::CycleCountDisposition::Posted => "posted",
            wareboxes_domain::CycleCountDisposition::RecountRequired => "recount_required",
            wareboxes_domain::CycleCountDisposition::ApprovalRequired => "approval_required",
        },
        "decision_policy": confirmation.decision_policy,
        "variance_id": confirmation.variance_id.map(|id| id.get()),
        "variance_revision": confirmation.variance_revision.map(|revision| revision.get()),
        "next_recount_task_id": confirmation.next_recount_task_id,
    });
    outbox::enqueue(
        &mut tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(command.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "cycle_count_confirmation",
            aggregate_id: &aggregate_id,
            ordering_key: &event_key,
            aggregate_sequence: 1,
            event_type: "inventory.cycle_count.confirmed",
            schema_version: 1,
            payload: &payload,
            occurred_at: confirmed_at,
        },
    )
    .await?;

    if let (Some(variance_id), Some(variance_revision)) =
        (confirmation.variance_id, confirmation.variance_revision)
    {
        let variance_event_key = format!("cycle-count-variance-confirmation:{task_id}");
        let variance_aggregate_id = variance_id.to_string();
        let variance_ordering_key = format!("cycle-count-variance:{variance_id}");
        let aggregate_sequence = variance_revision
            .get()
            .checked_sub(1)
            .filter(|sequence| *sequence > 0)
            .ok_or_else(|| AppError::internal("cycle count variance event sequence is invalid"))?;
        outbox::enqueue(
            &mut tx,
            &NewOutboxEvent {
                tenant_id: access.tenant_id,
                inventory_owner_id: Some(inventory_owner_id),
                facility_id: Some(facility_id),
                actor_user_id: Some(command.actor_id.get()),
                event_key: &variance_event_key,
                aggregate_type: "cycle_count_variance",
                aggregate_id: &variance_aggregate_id,
                ordering_key: &variance_ordering_key,
                aggregate_sequence,
                event_type: match confirmation.disposition {
                    wareboxes_domain::CycleCountDisposition::Posted => {
                        "inventory.cycle_count_variance.posted"
                    }
                    wareboxes_domain::CycleCountDisposition::RecountRequired => {
                        "inventory.cycle_count_variance.recount_required"
                    }
                    wareboxes_domain::CycleCountDisposition::ApprovalRequired => {
                        "inventory.cycle_count_variance.approval_required"
                    }
                },
                schema_version: 1,
                payload: &payload,
                occurred_at: confirmed_at,
            },
        )
        .await?;
    }

    Ok(prepared
        .commit_with_inventory_transaction(tx, confirmation, inventory_transaction_id)
        .await?)
}

fn core_decision_policy_snapshot(
    policy: &CountDecisionPolicyReadModel,
) -> CycleCountDecisionPolicySnapshot {
    CycleCountDecisionPolicySnapshot {
        source: match policy.source {
            CountDecisionPolicySource::ProductDefault => CoreCountPolicySource::ProductDefault,
            CountDecisionPolicySource::Configuration => CoreCountPolicySource::Configuration,
        },
        configuration_id: policy.configuration_id,
        configuration_revision: policy.configuration_revision,
        configuration_scope: policy.configuration_scope,
        absolute_tolerance_quantity: policy.absolute_tolerance_quantity,
        percentage_tolerance_basis_points: policy.percentage_tolerance_basis_points,
        approval_threshold_quantity: policy.approval_threshold_quantity,
        policy_hash: policy.policy_hash.clone(),
    }
}

const fn configuration_scope_level(scope: wareboxes_domain::ConfigurationScope) -> &'static str {
    match scope {
        wareboxes_domain::ConfigurationScope::Tenant => "tenant",
        wareboxes_domain::ConfigurationScope::InventoryOwner { .. } => "inventory_owner",
        wareboxes_domain::ConfigurationScope::Facility { .. } => "facility",
        wareboxes_domain::ConfigurationScope::OwnerFacility { .. } => "owner_facility",
    }
}

const fn configuration_scope_owner(scope: wareboxes_domain::ConfigurationScope) -> Option<i64> {
    match scope {
        wareboxes_domain::ConfigurationScope::InventoryOwner { inventory_owner_id }
        | wareboxes_domain::ConfigurationScope::OwnerFacility {
            inventory_owner_id, ..
        } => Some(inventory_owner_id.get()),
        wareboxes_domain::ConfigurationScope::Tenant
        | wareboxes_domain::ConfigurationScope::Facility { .. } => None,
    }
}

const fn configuration_scope_facility(scope: wareboxes_domain::ConfigurationScope) -> Option<i64> {
    match scope {
        wareboxes_domain::ConfigurationScope::Facility { facility_id }
        | wareboxes_domain::ConfigurationScope::OwnerFacility { facility_id, .. } => {
            Some(facility_id.get())
        }
        wareboxes_domain::ConfigurationScope::Tenant
        | wareboxes_domain::ConfigurationScope::InventoryOwner { .. } => None,
    }
}

async fn validate_scans_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    target: &CountTarget,
    balance: &LockedBalance,
    scans: CountScans<'_>,
) -> AppResult<()> {
    let location_matches: bool = sqlx::query_scalar(
        r#"
        SELECT barcode = $3
        FROM locations
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
          AND active
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.location_id)
    .bind(scans.location_barcode)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or(false);
    if !location_matches {
        return Err(AppError::conflict(
            "scanned location does not match the cycle count task",
        ));
    }

    let item_matches: bool = sqlx::query_scalar(
        r#"
        SELECT TRUE
        FROM barcodes
        WHERE tenant_id = $1
          AND item_id = $2
          AND name = $3
          AND deleted IS NULL
        ORDER BY id
        LIMIT 1
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.item_id)
    .bind(scans.item_barcode)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or(false);
    if !item_matches {
        return Err(AppError::conflict(
            "scanned item does not match the cycle count task",
        ));
    }

    match (balance.license_plate_id, scans.license_plate_barcode) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err(AppError::conflict(
            "cycle count stock is not on a license plate",
        )),
        (Some(_), None) => Err(AppError::conflict(
            "license plate scan is required for this cycle count",
        )),
        (Some(license_plate_id), Some(scanned_barcode)) => {
            let plate_matches: bool = sqlx::query_scalar(
                r#"
                SELECT barcode = $4
                FROM license_plates
                WHERE tenant_id = $1
                  AND inventory_owner_id = $2
                  AND id = $3
                  AND deleted IS NULL
                FOR SHARE
                "#,
            )
            .bind(tenant_id.get())
            .bind(target.inventory_owner_id)
            .bind(license_plate_id)
            .bind(scanned_barcode)
            .fetch_optional(&mut **tx)
            .await?
            .unwrap_or(false);
            if plate_matches {
                Ok(())
            } else {
                Err(AppError::conflict(
                    "scanned license plate does not match the cycle count task",
                ))
            }
        }
    }
}
