//! Transaction-scoped cycle-count policy and variance lifecycle support.

mod decision;
mod decision_policy;
mod policy;
mod read_model;

pub use decision::*;
pub use policy::*;
pub use read_model::*;

use sqlx::Row;
use wareboxes_domain::{
    decide_cycle_count_disposition_with_approval_threshold, CycleCountDisposition,
    CycleCountPolicyId, CycleCountPolicyRevision, CycleCountTolerancePolicy, CycleCountVarianceId,
    CycleCountVarianceRevision, CycleCountVarianceStatus, FacilityId, InventoryOwnerId,
};

use wareboxes_application::count_decision_policy::CountDecisionPolicyReadModel;

use crate::error::{AppError, AppResult};

use super::{create_item_location_cycle_count_task_tx, TaskDimensions};
use super::{CountTarget, LockedBalance};

#[derive(Debug, Clone, Copy)]
pub(super) struct CountPolicySnapshot {
    pub id: CycleCountPolicyId,
    pub revision: CycleCountPolicyRevision,
    pub policy: CycleCountTolerancePolicy,
}

#[derive(Debug, Clone)]
pub(super) struct PreparedCountControl {
    pub disposition: CycleCountDisposition,
    pub policy: Option<CountPolicySnapshot>,
    pub variance_id: Option<CycleCountVarianceId>,
    pub variance_revision: Option<CycleCountVarianceRevision>,
    pub attempt_sequence: u16,
    pub automatic_recounts_used: u16,
    pub allowed_variance_quantity: Option<i64>,
    pub decision_policy: Option<CountDecisionPolicyReadModel>,
}

pub(super) struct AdvancedCountControl {
    pub variance_revision: Option<CycleCountVarianceRevision>,
    pub next_recount_task_id: Option<i64>,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_count_control_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    actor_user_id: i64,
    task_id: i64,
    target: &CountTarget,
    balance: &LockedBalance,
    counted_quantity: i64,
    variance_quantity: i64,
    occurred_at: wareboxes_core::models::Timestamp,
) -> AppResult<PreparedCountControl> {
    if let Some(variance_id) = target.variance_id {
        return prepare_existing_case_tx(
            tx,
            tenant_id,
            task_id,
            target,
            balance,
            counted_quantity,
            variance_quantity,
            variance_id,
        )
        .await;
    }

    let Some(policy) =
        active_policy_tx(tx, tenant_id, target.inventory_owner_id, target.facility_id).await?
    else {
        return Ok(PreparedCountControl {
            disposition: CycleCountDisposition::Posted,
            policy: None,
            variance_id: None,
            variance_revision: None,
            attempt_sequence: target.attempt_sequence,
            automatic_recounts_used: 0,
            allowed_variance_quantity: None,
            decision_policy: None,
        });
    };
    let decision_policy = decision_policy::resolve_count_decision_policy_tx(
        tx,
        tenant_id,
        InventoryOwnerId::new(target.inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        FacilityId::new(target.facility_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        occurred_at,
        policy.policy,
    )
    .await?;
    let effective_tolerance = decision_policy
        .tolerance_policy(policy.policy.automatic_recount_limit())
        .map_err(|error| AppError::internal(error.to_string()))?;
    let allowed = effective_tolerance
        .allowed_variance_quantity(balance.qty_on_hand)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let disposition = decide_cycle_count_disposition_with_approval_threshold(
        effective_tolerance,
        balance.qty_on_hand,
        variance_quantity,
        0,
        decision_policy.approval_threshold_quantity,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    if disposition == CycleCountDisposition::Posted {
        return Ok(PreparedCountControl {
            disposition,
            policy: Some(policy),
            variance_id: None,
            variance_revision: None,
            attempt_sequence: target.attempt_sequence,
            automatic_recounts_used: 0,
            allowed_variance_quantity: Some(allowed),
            decision_policy: Some(decision_policy),
        });
    }

    let state = match disposition {
        CycleCountDisposition::RecountRequired => "awaiting_recount",
        CycleCountDisposition::ApprovalRequired => "awaiting_approval",
        CycleCountDisposition::Posted => unreachable!(),
    };
    let decision = decision_policy::count_decision_policy_bindings(&decision_policy)?;
    let row =
        sqlx::query(
            r#"
        INSERT INTO cycle_count_variance_cases (
            tenant_id, inventory_owner_id, facility_id, inventory_balance_id,
            location_id, item_id, item_batch_id, license_plate_id, uom, lot,
            expiration, serial, inventory_status, policy_id, policy_revision,
            absolute_tolerance_qty, percentage_tolerance_bps,
            automatic_recount_limit, latest_task_id, latest_attempt_sequence,
            automatic_recounts_used, system_qty_on_hand, system_qty_reserved,
            system_qty_held, counted_qty, variance_qty, allowed_variance_qty,
            state, revision, created_at, modified_at, count_policy_source,
            count_configuration_id, count_configuration_revision, count_scope_level,
            count_inventory_owner_id, count_facility_id,
            count_absolute_tolerance_qty, count_percentage_tolerance_bps,
            count_approval_threshold_qty, count_policy_hash
        )
        VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,
            $18,$19,$20,0,$21,$22,$23,$24,$25,$26,$27,1,$28,$28,$29,$30,
            $31,$32,$33,$34,$35,$36,$37,$38
        )
        RETURNING id
        "#,
        )
        .bind(tenant_id.get())
        .bind(target.inventory_owner_id)
        .bind(target.facility_id)
        .bind(target.inventory_balance_id)
        .bind(target.location_id)
        .bind(target.item_id)
        .bind(balance.item_batch_id)
        .bind(balance.license_plate_id)
        .bind(&balance.uom)
        .bind(&balance.lot)
        .bind(balance.expiration)
        .bind(&balance.serial)
        .bind(balance.status.as_str())
        .bind(policy.id.get())
        .bind(policy.revision.get())
        .bind(decision.absolute_tolerance_quantity)
        .bind(decision.percentage_tolerance_basis_points)
        .bind(
            i16::try_from(policy.policy.automatic_recount_limit()).map_err(|_| {
                AppError::internal("cycle count recount limit is out of database range")
            })?,
        )
        .bind(task_id)
        .bind(i16::try_from(target.attempt_sequence).map_err(|_| {
            AppError::internal("cycle count attempt sequence is out of database range")
        })?)
        .bind(balance.qty_on_hand)
        .bind(balance.qty_reserved)
        .bind(balance.qty_held)
        .bind(counted_quantity)
        .bind(variance_quantity)
        .bind(allowed)
        .bind(state)
        .bind(occurred_at)
        .bind(decision.source)
        .bind(decision.configuration_id)
        .bind(decision.configuration_revision)
        .bind(decision.scope_level)
        .bind(decision.inventory_owner_id)
        .bind(decision.facility_id)
        .bind(decision.absolute_tolerance_quantity)
        .bind(decision.percentage_tolerance_basis_points)
        .bind(decision.approval_threshold_quantity)
        .bind(decision.policy_hash)
        .fetch_one(&mut **tx)
        .await?;
    let variance_id = CycleCountVarianceId::new(row.try_get("id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let updated = sqlx::query(
        r#"
        UPDATE cycle_count_item_location_tasks
        SET variance_case_id=$1
        WHERE tenant_id=$2 AND task_id=$3 AND variance_case_id IS NULL
        "#,
    )
    .bind(variance_id.get())
    .bind(tenant_id.get())
    .bind(task_id)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "cycle count task variance case changed during confirmation",
        ));
    }

    let _ = actor_user_id;
    Ok(PreparedCountControl {
        disposition,
        policy: Some(policy),
        variance_id: Some(variance_id),
        variance_revision: Some(
            CycleCountVarianceRevision::new(1)
                .map_err(|error| AppError::internal(error.to_string()))?,
        ),
        attempt_sequence: target.attempt_sequence,
        automatic_recounts_used: 0,
        allowed_variance_quantity: Some(allowed),
        decision_policy: Some(decision_policy),
    })
}

#[allow(clippy::too_many_arguments)]
async fn prepare_existing_case_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    task_id: i64,
    target: &CountTarget,
    balance: &LockedBalance,
    _counted_quantity: i64,
    variance_quantity: i64,
    variance_id: CycleCountVarianceId,
) -> AppResult<PreparedCountControl> {
    let row = sqlx::query(
        r#"
        SELECT revision, state, latest_task_id, latest_attempt_sequence,
               automatic_recounts_used, policy_id, policy_revision,
               absolute_tolerance_qty, percentage_tolerance_bps,
               automatic_recount_limit, inventory_balance_id, location_id,
               item_id, item_batch_id, license_plate_id, uom, lot, expiration,
               serial, inventory_status, count_policy_source,
               count_configuration_id, count_configuration_revision,
               count_scope_level, count_inventory_owner_id, count_facility_id,
               count_absolute_tolerance_qty, count_percentage_tolerance_bps,
               count_approval_threshold_qty, count_policy_hash
        FROM cycle_count_variance_cases
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3 AND id=$4
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(variance_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("cycle count variance"))?;
    if row.try_get::<String, _>("state")? != "awaiting_recount"
        || row.try_get::<i64, _>("latest_task_id")? != task_id
        || row.try_get::<i16, _>("latest_attempt_sequence")?
            != i16::try_from(target.attempt_sequence)
                .map_err(|_| AppError::internal("cycle count attempt is out of range"))?
        || row.try_get::<i64, _>("inventory_balance_id")? != target.inventory_balance_id
        || row.try_get::<i64, _>("location_id")? != target.location_id
        || row.try_get::<i64, _>("item_id")? != target.item_id
        || row.try_get::<i64, _>("item_batch_id")? != balance.item_batch_id
        || row.try_get::<Option<i64>, _>("license_plate_id")? != balance.license_plate_id
        || row.try_get::<String, _>("uom")? != balance.uom
        || row.try_get::<Option<String>, _>("lot")? != balance.lot
        || row.try_get::<Option<wareboxes_core::models::Timestamp>, _>("expiration")?
            != balance.expiration
        || row.try_get::<Option<String>, _>("serial")? != balance.serial
        || row.try_get::<String, _>("inventory_status")? != balance.status.as_str()
    {
        return Err(AppError::conflict(
            "cycle count recount no longer matches its variance case",
        ));
    }
    let decision_policy = decision_policy::count_decision_policy_from_row(&row)?;
    let policy = CycleCountTolerancePolicy::new(
        decision_policy.absolute_tolerance_quantity,
        decision_policy.percentage_tolerance_basis_points,
        u16::try_from(row.try_get::<i16, _>("automatic_recount_limit")?)
            .map_err(|_| AppError::internal("stored cycle count recount limit is invalid"))?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let automatic_recounts_used = u16::try_from(row.try_get::<i16, _>("automatic_recounts_used")?)
        .map_err(|_| AppError::internal("stored automatic recount usage is invalid"))?;
    let allowed = policy
        .allowed_variance_quantity(balance.qty_on_hand)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let disposition = decide_cycle_count_disposition_with_approval_threshold(
        policy,
        balance.qty_on_hand,
        variance_quantity,
        automatic_recounts_used,
        decision_policy.approval_threshold_quantity,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(PreparedCountControl {
        disposition,
        policy: Some(CountPolicySnapshot {
            id: CycleCountPolicyId::new(row.try_get("policy_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            revision: CycleCountPolicyRevision::new(row.try_get("policy_revision")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            policy,
        }),
        variance_id: Some(variance_id),
        variance_revision: Some(
            CycleCountVarianceRevision::new(row.try_get("revision")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
        ),
        attempt_sequence: target.attempt_sequence,
        automatic_recounts_used,
        allowed_variance_quantity: Some(allowed),
        decision_policy: Some(decision_policy),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn advance_count_control_after_confirmation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    actor_user_id: i64,
    target: &CountTarget,
    balance: &LockedBalance,
    counted_quantity: i64,
    variance_quantity: i64,
    inventory_transaction_id: Option<i64>,
    control: PreparedCountControl,
    occurred_at: wareboxes_core::models::Timestamp,
) -> AppResult<AdvancedCountControl> {
    let Some(variance_id) = control.variance_id else {
        return Ok(AdvancedCountControl {
            variance_revision: None,
            next_recount_task_id: None,
        });
    };
    let current_revision = control
        .variance_revision
        .ok_or_else(|| AppError::internal("cycle count variance revision is missing"))?;
    let next_revision = current_revision
        .checked_next()
        .ok_or_else(|| AppError::internal("cycle count variance revision overflow"))?;
    let next_attempt = control
        .attempt_sequence
        .checked_add(1)
        .ok_or_else(|| AppError::internal("cycle count attempt sequence overflow"))?;

    let (state, next_task_id, automatic_recounts_used, resolved_by, resolved_at) =
        match control.disposition {
            CycleCountDisposition::Posted => (
                "posted",
                None,
                control.automatic_recounts_used,
                Some(actor_user_id),
                Some(occurred_at),
            ),
            CycleCountDisposition::ApprovalRequired => (
                "awaiting_approval",
                None,
                control.automatic_recounts_used,
                None,
                None,
            ),
            CycleCountDisposition::RecountRequired => {
                let task_id = create_item_location_cycle_count_task_tx(
                    tx,
                    tenant_id,
                    actor_user_id,
                    target.location_id,
                    target.item_id,
                    target.inventory_balance_id,
                    TaskDimensions {
                        facility_id: Some(target.facility_id),
                        inventory_owner_id: Some(target.inventory_owner_id),
                    },
                    Some("automatic_recount"),
                    None,
                    None,
                    Some("Blind recount required by cycle-count tolerance policy"),
                )
                .await?;
                attach_recount_task_tx(tx, tenant_id, task_id, variance_id, next_attempt).await?;
                (
                    "awaiting_recount",
                    Some(task_id),
                    control
                        .automatic_recounts_used
                        .checked_add(1)
                        .ok_or_else(|| AppError::internal("automatic recount usage overflow"))?,
                    None,
                    None,
                )
            }
        };
    let latest_task_id = next_task_id.unwrap_or(target.task_id);
    let latest_attempt = if next_task_id.is_some() {
        next_attempt
    } else {
        control.attempt_sequence
    };
    let updated =
        sqlx::query(
            r#"
        UPDATE cycle_count_variance_cases
        SET latest_task_id=$1, latest_attempt_sequence=$2,
            automatic_recounts_used=$3, system_qty_on_hand=$4,
            system_qty_reserved=$5, system_qty_held=$6, counted_qty=$7,
            variance_qty=$8, allowed_variance_qty=$9, state=$10,
            revision=$11, inventory_transaction_id=$12, modified_at=$13,
            resolved_by_user_id=$14, resolved_at=$15
        WHERE tenant_id=$16 AND inventory_owner_id=$17 AND facility_id=$18
          AND id=$19 AND revision=$20 AND state <> 'posted'
        "#,
        )
        .bind(latest_task_id)
        .bind(i16::try_from(latest_attempt).map_err(|_| {
            AppError::internal("cycle count attempt sequence is out of database range")
        })?)
        .bind(
            i16::try_from(automatic_recounts_used).map_err(|_| {
                AppError::internal("automatic recount usage is out of database range")
            })?,
        )
        .bind(balance.qty_on_hand)
        .bind(balance.qty_reserved)
        .bind(balance.qty_held)
        .bind(counted_quantity)
        .bind(variance_quantity)
        .bind(control.allowed_variance_quantity.ok_or_else(|| {
            AppError::internal("controlled cycle count is missing allowed variance")
        })?)
        .bind(state)
        .bind(next_revision.get())
        .bind(inventory_transaction_id)
        .bind(occurred_at)
        .bind(resolved_by)
        .bind(resolved_at)
        .bind(tenant_id.get())
        .bind(target.inventory_owner_id)
        .bind(target.facility_id)
        .bind(variance_id.get())
        .bind(current_revision.get())
        .execute(&mut **tx)
        .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "cycle count variance changed during confirmation",
        ));
    }
    Ok(AdvancedCountControl {
        variance_revision: Some(next_revision),
        next_recount_task_id: next_task_id,
    })
}

pub(super) async fn attach_recount_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    task_id: i64,
    variance_id: CycleCountVarianceId,
    attempt_sequence: u16,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE cycle_count_item_location_tasks
        SET variance_case_id=$1, attempt_sequence=$2
        WHERE tenant_id=$3 AND task_id=$4 AND variance_case_id IS NULL
        "#,
    )
    .bind(variance_id.get())
    .bind(
        i16::try_from(attempt_sequence)
            .map_err(|_| AppError::internal("cycle count attempt is out of database range"))?,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "cycle count recount task could not be linked to its variance",
        ));
    }
    Ok(())
}

async fn active_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
) -> AppResult<Option<CountPolicySnapshot>> {
    let row = sqlx::query(
        r#"
        SELECT id, revision, absolute_tolerance_qty,
               percentage_tolerance_bps, automatic_recount_limit
        FROM cycle_count_policies
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND effective_to IS NULL
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(CountPolicySnapshot {
            id: CycleCountPolicyId::new(row.try_get("id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            revision: CycleCountPolicyRevision::new(row.try_get("revision")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            policy: CycleCountTolerancePolicy::new(
                row.try_get("absolute_tolerance_qty")?,
                u32::try_from(row.try_get::<i32, _>("percentage_tolerance_bps")?)
                    .map_err(|_| AppError::internal("stored cycle count percentage is invalid"))?,
                u16::try_from(row.try_get::<i16, _>("automatic_recount_limit")?).map_err(|_| {
                    AppError::internal("stored cycle count recount limit is invalid")
                })?,
            )
            .map_err(|error| AppError::internal(error.to_string()))?,
        })
    })
    .transpose()
}

pub(super) fn variance_status(value: &str) -> AppResult<CycleCountVarianceStatus> {
    match value {
        "awaiting_recount" => Ok(CycleCountVarianceStatus::AwaitingRecount),
        "awaiting_approval" => Ok(CycleCountVarianceStatus::AwaitingApproval),
        "posted" => Ok(CycleCountVarianceStatus::Posted),
        _ => Err(AppError::internal(format!(
            "invalid cycle count variance status in database: {value}"
        ))),
    }
}
