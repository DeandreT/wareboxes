use sqlx::Row;
use wareboxes_application::cycle_count_control::{
    ConfigureCycleCountPolicyCommand, ConfigureCycleCountPolicyResult,
    CONFIGURE_CYCLE_COUNT_POLICY_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{CycleCountPolicyId, CycleCountPolicyRevision, UserId};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub async fn configure_cycle_count_policy_in_scope(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureCycleCountPolicyCommand,
) -> AppResult<ConfigureCycleCountPolicyResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, CONFIGURE_CYCLE_COUNT_POLICY_OPERATION, command)?;
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
    if !scope.includes_inventory_owner(command.inventory_owner_id.get())
        || !scope.includes_facility(command.facility_id.get())
    {
        return Err(AppError::not_found("cycle count policy"));
    }
    if let Some(result) = prepared
        .replayed::<ConfigureCycleCountPolicyResult>(&mut tx)
        .await?
    {
        if result.inventory_owner_id != command.inventory_owner_id
            || result.facility_id != command.facility_id
        {
            return Err(AppError::not_found("cycle count policy"));
        }
        tx.commit().await?;
        return Ok(result);
    }

    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT || ':' || $2::TEXT || ':' || $3::TEXT || ':cycle-count-policy', 0))",
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .execute(&mut *tx)
    .await?;
    let references_exist: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM inventory_owner_facilities owner_facility
            JOIN inventory_owners owner
              ON owner.tenant_id=owner_facility.tenant_id
             AND owner.id=owner_facility.inventory_owner_id
             AND owner.deleted IS NULL
            JOIN facilities facility
              ON facility.tenant_id=owner_facility.tenant_id
             AND facility.id=owner_facility.facility_id
             AND facility.deleted IS NULL
            WHERE owner_facility.tenant_id=$1
              AND owner_facility.inventory_owner_id=$2
              AND owner_facility.facility_id=$3
              AND owner_facility.deleted IS NULL
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if !references_exist {
        return Err(AppError::not_found("cycle count policy scope"));
    }
    let predecessor = sqlx::query(
        r#"
        SELECT id, revision
        FROM cycle_count_policies
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND effective_to IS NULL
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .fetch_optional(&mut *tx)
    .await?;
    match (command.expected_revision, predecessor.as_ref()) {
        (None, None) => {}
        (Some(expected), Some(row)) if row.try_get::<i64, _>("revision")? == expected.get() => {}
        (None, Some(_)) => {
            return Err(AppError::conflict(
                "cycle count policy already exists; expected revision is required",
            ));
        }
        _ => {
            return Err(AppError::conflict(
                "cycle count policy revision does not match expected revision",
            ));
        }
    }
    let configured_at = now_iso();
    if let Some(row) = predecessor.as_ref() {
        sqlx::query(
            "UPDATE cycle_count_policies SET effective_to=$1 WHERE tenant_id=$2 AND id=$3 AND effective_to IS NULL",
        )
        .bind(configured_at)
        .bind(access.tenant_id.get())
        .bind(row.try_get::<i64, _>("id")?)
        .execute(&mut *tx)
        .await?;
    }
    let previous_revision = predecessor
        .as_ref()
        .map(|row| -> AppResult<CycleCountPolicyRevision> {
            CycleCountPolicyRevision::new(row.try_get::<i64, _>("revision")?)
                .map_err(|error| AppError::internal(error.to_string()))
        })
        .transpose()?;
    let revision = previous_revision
        .map_or_else(
            || CycleCountPolicyRevision::new(1),
            |revision| {
                revision
                    .checked_next()
                    .ok_or(wareboxes_domain::CycleCountError::InvalidRevision { value: i64::MAX })
            },
        )
        .map_err(|error| AppError::internal(error.to_string()))?;
    let row = sqlx::query(
        r#"
        INSERT INTO cycle_count_policies (
            tenant_id, inventory_owner_id, facility_id, absolute_tolerance_qty,
            percentage_tolerance_bps, automatic_recount_limit, revision,
            supersedes_policy_id, effective_from, configured_by_user_id
        )
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(command.policy.absolute_tolerance_quantity())
    .bind(
        i32::try_from(command.policy.percentage_tolerance_basis_points())
            .map_err(|_| AppError::internal("cycle count percentage is out of range"))?,
    )
    .bind(
        i16::try_from(command.policy.automatic_recount_limit())
            .map_err(|_| AppError::internal("cycle count recount limit is out of range"))?,
    )
    .bind(revision.get())
    .bind(
        predecessor
            .as_ref()
            .map(|row| row.try_get::<i64, _>("id"))
            .transpose()?,
    )
    .bind(configured_at)
    .bind(context.actor_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let policy_id = CycleCountPolicyId::new(row.try_get("id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let result = ConfigureCycleCountPolicyResult {
        policy_id,
        inventory_owner_id: command.inventory_owner_id,
        facility_id: command.facility_id,
        policy: command.policy,
        previous_revision,
        revision,
        configured_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        configured_at,
    };
    let event_key = format!("cycle-count-policy:{}:{}", policy_id.get(), revision.get());
    outbox::enqueue(
        &mut tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(command.inventory_owner_id),
            facility_id: Some(command.facility_id),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "cycle_count_policy",
            aggregate_id: &policy_id.to_string(),
            ordering_key: &format!(
                "cycle-count-policy:{}:{}:{}",
                access.tenant_id, command.inventory_owner_id, command.facility_id
            ),
            aggregate_sequence: revision.get(),
            event_type: "inventory.cycle_count_policy.configured",
            schema_version: 1,
            payload: &serde_json::to_value(&result)
                .map_err(|error| AppError::internal(error.to_string()))?,
            occurred_at: configured_at,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}
