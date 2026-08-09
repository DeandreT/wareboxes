use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbound_qa::{
    ConfigureOutboundQaPolicyCommand, ConfigureOutboundQaPolicyResult,
    CONFIGURE_OUTBOUND_QA_POLICY_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{OutboundQaPolicyId, OutboundQaPolicyRevision, UserId};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

use super::{
    active_policy_tx, enqueue_event_tx, require_scope, require_stored_visible_before_replay_tx,
};

pub async fn configure_policy(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureOutboundQaPolicyCommand,
) -> AppResult<ConfigureOutboundQaPolicyResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, CONFIGURE_OUTBOUND_QA_POLICY_OPERATION, command)?;
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
    require_stored_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ConfigureOutboundQaPolicyResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    require_scope(
        &scope,
        command.inventory_owner_id.get(),
        command.facility_id.get(),
        "outbound QA policy",
    )?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "outbound-qa-policy:{}:{}:{}",
            access.tenant_id, command.inventory_owner_id, command.facility_id
        ))
        .execute(&mut *tx)
        .await?;
    let valid_scope: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM inventory_owner_facilities owner_facility
            JOIN inventory_owners owner
              ON owner.tenant_id=owner_facility.tenant_id
             AND owner.id=owner_facility.inventory_owner_id AND owner.deleted IS NULL
            JOIN facilities facility
              ON facility.tenant_id=owner_facility.tenant_id
             AND facility.id=owner_facility.facility_id AND facility.deleted IS NULL
            WHERE owner_facility.tenant_id=$1
              AND owner_facility.inventory_owner_id=$2
              AND owner_facility.facility_id=$3)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if !valid_scope {
        return Err(AppError::not_found("outbound QA policy"));
    }
    let predecessor = active_policy_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id,
        true,
    )
    .await?;
    match (command.expected_revision, predecessor.as_ref()) {
        (None, None) => {}
        (Some(expected), Some(current)) if expected == current.revision => {}
        (None, Some(_)) => {
            return Err(AppError::conflict(
                "outbound QA policy already exists at this scope",
            ));
        }
        _ => return Err(AppError::conflict("outbound QA policy revision is stale")),
    }
    let configured_at = now_iso();
    if let Some(predecessor) = predecessor.as_ref() {
        let updated = sqlx::query(
            r#"
            UPDATE outbound_qa_policies SET effective_to=$1
            WHERE tenant_id=$2 AND id=$3 AND effective_to IS NULL
            "#,
        )
        .bind(configured_at)
        .bind(access.tenant_id.get())
        .bind(predecessor.policy_id.get())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::conflict("outbound QA policy changed"));
        }
    }
    let revision = match predecessor.as_ref() {
        Some(current) => current
            .revision
            .checked_next()
            .ok_or_else(|| AppError::internal("outbound QA policy revision overflow"))?,
        None => OutboundQaPolicyRevision::new(1)
            .map_err(|error| AppError::internal(error.to_string()))?,
    };
    let policy_id_raw: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO outbound_qa_policies (
            tenant_id,inventory_owner_id,facility_id,requirement,revision,
            supersedes_policy_id,effective_from,configured_by_user_id,configured_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$7) RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(command.requirement.as_str())
    .bind(revision.get())
    .bind(predecessor.as_ref().map(|policy| policy.policy_id.get()))
    .bind(configured_at)
    .bind(context.actor_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let result = ConfigureOutboundQaPolicyResult {
        policy_id: OutboundQaPolicyId::new(policy_id_raw)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: command.inventory_owner_id,
        facility_id: command.facility_id,
        requirement: command.requirement,
        revision,
        configured_by: UserId::new(context.actor_id.get())
            .map_err(|error| AppError::internal(error.to_string()))?,
        configured_at,
    };
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id,
        context.actor_id.get(),
        &format!(
            "outbound_qa_policy:{}:{}",
            command.inventory_owner_id, command.facility_id,
        ),
        "outbound_qa_policy",
        result.policy_id.get(),
        "outbound.qa.policy_configured",
        &format!("configured:{}", revision.get()),
        &serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?,
        configured_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}
