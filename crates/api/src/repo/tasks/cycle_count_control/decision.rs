use sqlx::Row;
use wareboxes_application::cycle_count_control::{
    DecideCycleCountVarianceCommand, DecideCycleCountVarianceResult,
    DECIDE_CYCLE_COUNT_VARIANCE_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryStatus, InventoryTransactionType, TenantAccess};
use wareboxes_domain::{
    CycleCountDisposition, CycleCountVarianceDecision, CycleCountVarianceDecisionId,
    CycleCountVarianceRevision, CycleCountVarianceStatus, FacilityId, InventoryOwnerId, UserId,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::inventory_locking::lock_license_plate;

use super::super::{create_item_location_cycle_count_task_tx, TaskDimensions};
use super::{attach_recount_task_tx, variance_status};

pub async fn decide_cycle_count_variance_in_scope(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &DecideCycleCountVarianceCommand,
) -> AppResult<DecideCycleCountVarianceResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, DECIDE_CYCLE_COUNT_VARIANCE_OPERATION, command)?;
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
    let hint = sqlx::query(
        "SELECT inventory_owner_id, facility_id FROM cycle_count_variance_cases WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(command.variance_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("cycle count variance"))?;
    let hint_owner: i64 = hint.try_get("inventory_owner_id")?;
    let hint_facility: i64 = hint.try_get("facility_id")?;
    if !scope.includes_inventory_owner(hint_owner) || !scope.includes_facility(hint_facility) {
        return Err(AppError::not_found("cycle count variance"));
    }
    if let Some(result) = prepared
        .replayed::<DecideCycleCountVarianceResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT variance.inventory_owner_id, variance.facility_id,
               variance.inventory_balance_id, variance.location_id,
               variance.item_id, variance.item_batch_id, variance.license_plate_id,
               variance.inventory_status, variance.latest_task_id,
               variance.latest_attempt_sequence, variance.system_qty_on_hand,
               variance.system_qty_reserved, variance.system_qty_held,
               variance.counted_qty, variance.variance_qty, variance.state,
               variance.revision
        FROM cycle_count_variance_cases variance
        WHERE variance.tenant_id=$1 AND variance.id=$2
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.variance_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("cycle count variance"))?;
    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    if inventory_owner_id != hint_owner || facility_id != hint_facility {
        return Err(AppError::not_found("cycle count variance"));
    }
    let previous_status = variance_status(&row.try_get::<String, _>("state")?)?;
    if previous_status != CycleCountVarianceStatus::AwaitingApproval {
        return Err(AppError::conflict(
            "cycle count variance is not awaiting approval",
        ));
    }
    let previous_revision = CycleCountVarianceRevision::new(row.try_get("revision")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    if previous_revision != command.expected_revision {
        return Err(AppError::conflict(
            "cycle count variance revision does not match expected revision",
        ));
    }
    let next_revision = previous_revision
        .checked_next()
        .ok_or_else(|| AppError::internal("cycle count variance revision overflow"))?;
    let decided_at = now_iso();

    let (status, disposition, next_task_id, inventory_transaction_id) = match command
        .details
        .decision
    {
        CycleCountVarianceDecision::RequestRecount => {
            let attempt = u16::try_from(row.try_get::<i16, _>("latest_attempt_sequence")?)
                .map_err(|_| AppError::internal("stored count attempt is invalid"))?
                .checked_add(1)
                .ok_or_else(|| AppError::internal("cycle count attempt overflow"))?;
            let task_id = create_item_location_cycle_count_task_tx(
                &mut tx,
                access.tenant_id,
                context.actor_id.get(),
                row.try_get("location_id")?,
                row.try_get("item_id")?,
                row.try_get("inventory_balance_id")?,
                TaskDimensions {
                    facility_id: Some(facility_id),
                    inventory_owner_id: Some(inventory_owner_id),
                },
                Some("supervisor_recount"),
                None,
                None,
                Some("Blind recount requested during variance review"),
            )
            .await?;
            attach_recount_task_tx(
                &mut tx,
                access.tenant_id,
                task_id,
                command.variance_id,
                attempt,
            )
            .await?;
            let updated = sqlx::query(
                r#"
                UPDATE cycle_count_variance_cases
                SET latest_task_id=$1, latest_attempt_sequence=$2,
                    state='awaiting_recount', revision=$3, modified_at=$4
                WHERE tenant_id=$5 AND inventory_owner_id=$6 AND facility_id=$7
                  AND id=$8 AND revision=$9 AND state='awaiting_approval'
                "#,
            )
            .bind(task_id)
            .bind(
                i16::try_from(attempt)
                    .map_err(|_| AppError::internal("count attempt is out of range"))?,
            )
            .bind(next_revision.get())
            .bind(decided_at)
            .bind(access.tenant_id.get())
            .bind(inventory_owner_id)
            .bind(facility_id)
            .bind(command.variance_id.get())
            .bind(previous_revision.get())
            .execute(&mut *tx)
            .await?;
            require_single_variance_update(updated.rows_affected())?;
            (
                CycleCountVarianceStatus::AwaitingRecount,
                CycleCountDisposition::RecountRequired,
                Some(task_id),
                None,
            )
        }
        CycleCountVarianceDecision::ApproveAdjustment => {
            let license_plate_id: Option<i64> = row.try_get("license_plate_id")?;
            lock_license_plate(&mut tx, access.tenant_id, license_plate_id).await?;
            let balance = sqlx::query(
                r#"
                SELECT item_batch_id, license_plate_id, status, qty_on_hand,
                       qty_reserved, qty_held, deleted
                FROM inventory_balances
                WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3 AND id=$4
                FOR UPDATE
                "#,
            )
            .bind(access.tenant_id.get())
            .bind(inventory_owner_id)
            .bind(facility_id)
            .bind(row.try_get::<i64, _>("inventory_balance_id")?)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                AppError::conflict("cycle count inventory balance is no longer active")
            })?;
            let counted_quantity: i64 = row.try_get("counted_qty")?;
            if balance
                .try_get::<Option<wareboxes_core::models::Timestamp>, _>("deleted")?
                .is_some()
                || balance.try_get::<i64, _>("item_batch_id")?
                    != row.try_get::<i64, _>("item_batch_id")?
                || balance.try_get::<Option<i64>, _>("license_plate_id")? != license_plate_id
                || balance.try_get::<String, _>("status")?
                    != row.try_get::<String, _>("inventory_status")?
                || balance.try_get::<i64, _>("qty_on_hand")?
                    != row.try_get::<i64, _>("system_qty_on_hand")?
                || balance.try_get::<i64, _>("qty_reserved")?
                    != row.try_get::<i64, _>("system_qty_reserved")?
                || balance.try_get::<i64, _>("qty_held")?
                    != row.try_get::<i64, _>("system_qty_held")?
            {
                return Err(AppError::conflict(
                    "cycle count inventory changed; request a new recount",
                ));
            }
            let inventory_status =
                InventoryStatus::parse(&row.try_get::<String, _>("inventory_status")?)
                    .ok_or_else(|| AppError::internal("stored cycle count status is invalid"))?;
            let owner_facility =
                inventory_journal::owner_facility_scope(inventory_owner_id, facility_id)?;
            let transaction_id = inventory_journal::begin_transaction(
                &mut tx,
                &JournalCommand {
                    tenant_id: access.tenant_id,
                    owner_facility,
                    actor_user_id: context.actor_id.get(),
                    transaction_type: InventoryTransactionType::Adjust,
                    reason: Some("approved cycle count variance"),
                    reference_type: Some("cycle_count_variance_case"),
                    reference_id: Some(command.variance_id.get()),
                    correlation_id: Some(&context.request_id),
                    operation: DECIDE_CYCLE_COUNT_VARIANCE_OPERATION,
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
                    location_id: row.try_get("location_id")?,
                    license_plate_id,
                    item_batch_id: row.try_get("item_batch_id")?,
                    status: inventory_status,
                    quantity_delta: row.try_get("variance_qty")?,
                },
            )
            .await?;
            let balance_updated = sqlx::query(
                r#"
                UPDATE inventory_balances SET qty_on_hand=$1, modified=$2
                WHERE tenant_id=$3 AND inventory_owner_id=$4 AND facility_id=$5 AND id=$6
                  AND deleted IS NULL AND qty_on_hand=$7 AND qty_reserved=$8 AND qty_held=$9
                "#,
            )
            .bind(counted_quantity)
            .bind(decided_at)
            .bind(access.tenant_id.get())
            .bind(inventory_owner_id)
            .bind(facility_id)
            .bind(row.try_get::<i64, _>("inventory_balance_id")?)
            .bind(row.try_get::<i64, _>("system_qty_on_hand")?)
            .bind(row.try_get::<i64, _>("system_qty_reserved")?)
            .bind(row.try_get::<i64, _>("system_qty_held")?)
            .execute(&mut *tx)
            .await?;
            if balance_updated.rows_affected() != 1 {
                return Err(AppError::conflict(
                    "cycle count inventory changed during approval",
                ));
            }
            let updated = sqlx::query(
                r#"
                UPDATE cycle_count_variance_cases
                SET state='posted', revision=$1, inventory_transaction_id=$2,
                    modified_at=$3, resolved_by_user_id=$4, resolved_at=$3
                WHERE tenant_id=$5 AND inventory_owner_id=$6 AND facility_id=$7
                  AND id=$8 AND revision=$9 AND state='awaiting_approval'
                "#,
            )
            .bind(next_revision.get())
            .bind(transaction_id)
            .bind(decided_at)
            .bind(context.actor_id.get())
            .bind(access.tenant_id.get())
            .bind(inventory_owner_id)
            .bind(facility_id)
            .bind(command.variance_id.get())
            .bind(previous_revision.get())
            .execute(&mut *tx)
            .await?;
            require_single_variance_update(updated.rows_affected())?;
            (
                CycleCountVarianceStatus::Posted,
                CycleCountDisposition::Posted,
                None,
                Some(transaction_id),
            )
        }
    };

    let decision_row = sqlx::query(
        r#"
        INSERT INTO cycle_count_variance_decisions (
            tenant_id, inventory_owner_id, facility_id, variance_case_id,
            expected_revision, resulting_revision, decision, reason_code, note,
            next_task_id, inventory_transaction_id, decided_by_user_id, decided_at
        )
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .bind(command.variance_id.get())
    .bind(previous_revision.get())
    .bind(next_revision.get())
    .bind(decision_text(command.details.decision))
    .bind(reason_text(command.details.reason))
    .bind(command.details.note.as_ref().map(|note| note.as_str()))
    .bind(next_task_id)
    .bind(inventory_transaction_id)
    .bind(context.actor_id.get())
    .bind(decided_at)
    .fetch_one(&mut *tx)
    .await?;
    let decision_id = CycleCountVarianceDecisionId::new(decision_row.try_get("id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let result = DecideCycleCountVarianceResult {
        decision_id,
        variance_id: command.variance_id,
        previous_status,
        status,
        previous_revision,
        revision: next_revision,
        disposition,
        next_task_id,
        inventory_transaction_id,
        decided_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        decided_at,
    };
    let event_key = format!("cycle-count-variance-decision:{}", decision_id.get());
    let aggregate_id = command.variance_id.to_string();
    let ordering_key = format!("cycle-count-variance:{}", command.variance_id);
    let payload =
        serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?;
    outbox::enqueue(
        &mut tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(
                InventoryOwnerId::new(inventory_owner_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            facility_id: Some(
                FacilityId::new(facility_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "cycle_count_variance",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: next_revision
                .get()
                .checked_sub(1)
                .filter(|sequence| *sequence > 0)
                .ok_or_else(|| {
                    AppError::internal("cycle count variance event sequence is invalid")
                })?,
            event_type: match command.details.decision {
                CycleCountVarianceDecision::ApproveAdjustment => {
                    "inventory.cycle_count_variance.approved"
                }
                CycleCountVarianceDecision::RequestRecount => {
                    "inventory.cycle_count_variance.recount_requested"
                }
            },
            schema_version: 1,
            payload: &payload,
            occurred_at: decided_at,
        },
    )
    .await?;
    Ok(prepared
        .commit_with_inventory_transaction(tx, result, inventory_transaction_id)
        .await?)
}

fn require_single_variance_update(rows: u64) -> AppResult<()> {
    if rows == 1 {
        Ok(())
    } else {
        Err(AppError::conflict(
            "cycle count variance changed during decision",
        ))
    }
}

const fn decision_text(decision: CycleCountVarianceDecision) -> &'static str {
    match decision {
        CycleCountVarianceDecision::ApproveAdjustment => "approve_adjustment",
        CycleCountVarianceDecision::RequestRecount => "request_recount",
    }
}

const fn reason_text(reason: wareboxes_domain::CycleCountVarianceReason) -> &'static str {
    match reason {
        wareboxes_domain::CycleCountVarianceReason::VerifiedPhysicalCount => {
            "verified_physical_count"
        }
        wareboxes_domain::CycleCountVarianceReason::PackagingOrUomIssue => "packaging_or_uom_issue",
        wareboxes_domain::CycleCountVarianceReason::ReceivingOrShippingTiming => {
            "receiving_or_shipping_timing"
        }
        wareboxes_domain::CycleCountVarianceReason::SuspectedMiscount => "suspected_miscount",
        wareboxes_domain::CycleCountVarianceReason::Other => "other",
    }
}
