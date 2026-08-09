//! Versioned approved item substitutions.

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::item_substitution::{
    ConfigureItemSubstitutionPolicyCommand, ConfigureItemSubstitutionPolicyResult,
    ItemSubstitutionPolicyFilter, ItemSubstitutionPolicyReadModel,
    RetireItemSubstitutionPolicyCommand, RetireItemSubstitutionPolicyResult,
    CONFIGURE_ITEM_SUBSTITUTION_POLICY_OPERATION, RETIRE_ITEM_SUBSTITUTION_POLICY_OPERATION,
};
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryOwnerId, ItemSubstitutionDefinition,
    ItemSubstitutionPolicyId, ItemSubstitutionPolicyRevision, SubstitutionQuantity,
    SubstitutionUom, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::next_outbox_sequence_tx;

mod execution;
pub use execution::substitute_shortage;

pub async fn configure_policy(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureItemSubstitutionPolicyCommand,
) -> AppResult<ConfigureItemSubstitutionPolicyResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(
        context,
        CONFIGURE_ITEM_SUBSTITUTION_POLICY_OPERATION,
        command,
    )?;
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
    require_replay_visibility_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ConfigureItemSubstitutionPolicyResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    require_scope(
        &scope,
        command.inventory_owner_id.get(),
        command.facility_id.get(),
    )?;
    lock_natural_key_tx(&mut tx, access.tenant_id, command).await?;
    require_active_scope_and_items_tx(&mut tx, access.tenant_id, command).await?;
    let predecessor = latest_policy_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id,
        &command.definition,
        true,
    )
    .await?;
    match (command.expected_revision, predecessor.as_ref()) {
        (None, None) => {}
        (Some(expected), Some(current)) if expected == current.revision => {}
        (None, Some(_)) => {
            return Err(AppError::conflict(
                "item substitution policy already has revision history",
            ));
        }
        _ => {
            return Err(AppError::conflict(
                "item substitution policy revision is stale",
            ))
        }
    }
    let configured_at = now_iso();
    if let Some(current) = predecessor.as_ref() {
        if current.active {
            close_policy_tx(
                &mut tx,
                access.tenant_id,
                current.policy_id,
                configured_at,
                None,
            )
            .await?;
        }
    }
    let revision = match predecessor.as_ref() {
        None => ItemSubstitutionPolicyRevision::new(1).map_err(internal)?,
        Some(current) => current
            .revision
            .checked_next()
            .ok_or_else(|| AppError::internal("item substitution policy revision overflow"))?,
    };
    let policy_id = ItemSubstitutionPolicyId::new(
        sqlx::query_scalar(
            r#"INSERT INTO item_substitution_policies (
                 tenant_id,inventory_owner_id,facility_id,
                 source_item_id,source_uom,substitute_item_id,substitute_uom,
                 source_qty,substitute_qty,revision,supersedes_policy_id,
                 effective_from,configured_by_user_id,configured_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$12)
               RETURNING id"#,
        )
        .bind(access.tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(command.definition.source_item_id.get())
        .bind(command.definition.source_uom.as_str())
        .bind(command.definition.substitute_item_id.get())
        .bind(command.definition.substitute_uom.as_str())
        .bind(command.definition.source_quantity.get())
        .bind(command.definition.substitute_quantity.get())
        .bind(revision.get())
        .bind(predecessor.as_ref().map(|value| value.policy_id.get()))
        .bind(configured_at)
        .bind(context.actor_id.get())
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(internal)?;
    let result = ItemSubstitutionPolicyReadModel {
        policy_id,
        inventory_owner_id: command.inventory_owner_id,
        facility_id: command.facility_id,
        definition: command.definition.clone(),
        revision,
        active: true,
        configured_by: context.actor_id,
        configured_at,
        retired_by: None,
        retired_at: None,
    };
    enqueue_policy_event_tx(
        &mut tx,
        access.tenant_id,
        &result,
        "outbound.item_substitution.policy_configured",
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn retire_policy(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RetireItemSubstitutionPolicyCommand,
) -> AppResult<RetireItemSubstitutionPolicyResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, RETIRE_ITEM_SUBSTITUTION_POLICY_OPERATION, command)?;
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
    require_replay_visibility_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<RetireItemSubstitutionPolicyResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let hint = policy_by_id_tx(&mut tx, access.tenant_id, command.policy_id, false)
        .await?
        .ok_or_else(|| AppError::not_found("item substitution policy"))?;
    require_scope(
        &scope,
        hint.inventory_owner_id.get(),
        hint.facility_id.get(),
    )?;
    lock_model_key_tx(&mut tx, access.tenant_id, &hint).await?;
    let current = policy_by_id_tx(&mut tx, access.tenant_id, command.policy_id, true)
        .await?
        .ok_or_else(|| AppError::not_found("item substitution policy"))?;
    if !current.active || current.revision != command.expected_revision {
        return Err(AppError::conflict(
            "item substitution policy revision is stale or retired",
        ));
    }
    let retired_at = now_iso();
    close_policy_tx(
        &mut tx,
        access.tenant_id,
        current.policy_id,
        retired_at,
        Some(context.actor_id.get()),
    )
    .await?;
    let result = ItemSubstitutionPolicyReadModel {
        active: false,
        retired_by: Some(context.actor_id),
        retired_at: Some(retired_at),
        ..current
    };
    enqueue_policy_event_tx(
        &mut tx,
        access.tenant_id,
        &result,
        "outbound.item_substitution.policy_retired",
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn list_policies(
    db: &Db,
    access: &TenantAccess,
    filter: &ItemSubstitutionPolicyFilter,
) -> AppResult<Vec<ItemSubstitutionPolicyReadModel>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "orders").await?;
    require_scope(
        &scope,
        filter.inventory_owner_id.get(),
        filter.facility_id.get(),
    )?;
    let rows = sqlx::query(
        r#"SELECT id,inventory_owner_id,facility_id,source_item_id,source_uom,
                  substitute_item_id,substitute_uom,source_qty,substitute_qty,
                  revision,effective_to,configured_by_user_id,configured_at,
                  retired_by_user_id
           FROM item_substitution_policies
           WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
             AND ($4::bigint IS NULL OR source_item_id=$4)
             AND (NOT $5 OR effective_to IS NULL)
           ORDER BY source_item_id,source_uom,substitute_item_id,substitute_uom,revision DESC,id DESC"#,
    )
    .bind(access.tenant_id.get())
    .bind(filter.inventory_owner_id.get())
    .bind(filter.facility_id.get())
    .bind(filter.source_item_id)
    .bind(filter.active_only)
    .fetch_all(&mut *tx)
    .await?;
    let result = rows.iter().map(map_row).collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(result)
}

async fn require_active_scope_and_items_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &ConfigureItemSubstitutionPolicyCommand,
) -> AppResult<()> {
    let item_ids = [
        command.definition.source_item_id.get(),
        command.definition.substitute_item_id.get(),
    ];
    let rows = sqlx::query(
        r#"SELECT item.id,item.packaging_unit
           FROM inventory_owner_items owner_item
           JOIN items item ON item.tenant_id=owner_item.tenant_id
                          AND item.id=owner_item.item_id AND item.deleted IS NULL
           JOIN inventory_owner_facilities assignment
             ON assignment.tenant_id=owner_item.tenant_id
            AND assignment.inventory_owner_id=owner_item.inventory_owner_id
            AND assignment.facility_id=$3 AND assignment.deleted IS NULL
           WHERE owner_item.tenant_id=$1 AND owner_item.inventory_owner_id=$2
             AND owner_item.item_id=ANY($4) AND owner_item.deleted IS NULL
           ORDER BY item.id FOR SHARE OF owner_item,item,assignment"#,
    )
    .bind(tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(item_ids)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != 2 {
        return Err(AppError::conflict(
            "source and substitute items must be active client items at the facility",
        ));
    }
    for (item_id, uom) in [
        (
            command.definition.source_item_id.get(),
            command.definition.source_uom.as_str(),
        ),
        (
            command.definition.substitute_item_id.get(),
            command.definition.substitute_uom.as_str(),
        ),
    ] {
        let actual = rows
            .iter()
            .find(|row| row.try_get::<i64, _>("id").ok() == Some(item_id))
            .map(|row| row.try_get::<String, _>("packaging_unit"))
            .transpose()?
            .ok_or_else(|| AppError::conflict("item is no longer available"))?;
        if actual != uom {
            return Err(AppError::conflict(
                "item substitution UOM must match the active client item",
            ));
        }
    }
    Ok(())
}

async fn latest_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    definition: &ItemSubstitutionDefinition,
    lock: bool,
) -> AppResult<Option<ItemSubstitutionPolicyReadModel>> {
    let suffix = if lock { " FOR UPDATE" } else { "" };
    let query = format!(
        "SELECT id,inventory_owner_id,facility_id,source_item_id,source_uom,substitute_item_id,substitute_uom,source_qty,substitute_qty,revision,effective_to,configured_by_user_id,configured_at,retired_by_user_id FROM item_substitution_policies WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3 AND source_item_id=$4 AND source_uom=$5 AND substitute_item_id=$6 AND substitute_uom=$7 ORDER BY revision DESC LIMIT 1{suffix}"
    );
    sqlx::query(&query)
        .bind(tenant_id.get())
        .bind(owner_id.get())
        .bind(facility_id.get())
        .bind(definition.source_item_id.get())
        .bind(definition.source_uom.as_str())
        .bind(definition.substitute_item_id.get())
        .bind(definition.substitute_uom.as_str())
        .fetch_optional(&mut **tx)
        .await?
        .as_ref()
        .map(map_row)
        .transpose()
}

async fn policy_by_id_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ItemSubstitutionPolicyId,
    lock: bool,
) -> AppResult<Option<ItemSubstitutionPolicyReadModel>> {
    let suffix = if lock { " FOR UPDATE" } else { "" };
    let query = format!(
        "SELECT id,inventory_owner_id,facility_id,source_item_id,source_uom,substitute_item_id,substitute_uom,source_qty,substitute_qty,revision,effective_to,configured_by_user_id,configured_at,retired_by_user_id FROM item_substitution_policies WHERE tenant_id=$1 AND id=$2{suffix}"
    );
    sqlx::query(&query)
        .bind(tenant_id.get())
        .bind(policy_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .as_ref()
        .map(map_row)
        .transpose()
}

fn map_row(row: &sqlx::postgres::PgRow) -> AppResult<ItemSubstitutionPolicyReadModel> {
    let effective_to: Option<Timestamp> = row.try_get("effective_to")?;
    Ok(ItemSubstitutionPolicyReadModel {
        policy_id: ItemSubstitutionPolicyId::new(row.try_get("id")?).map_err(internal)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        definition: ItemSubstitutionDefinition::new(
            CatalogItemId::new(row.try_get("source_item_id")?).map_err(internal)?,
            SubstitutionUom::new(row.try_get::<String, _>("source_uom")?).map_err(internal)?,
            CatalogItemId::new(row.try_get("substitute_item_id")?).map_err(internal)?,
            SubstitutionUom::new(row.try_get::<String, _>("substitute_uom")?).map_err(internal)?,
            SubstitutionQuantity::new(row.try_get("source_qty")?).map_err(internal)?,
            SubstitutionQuantity::new(row.try_get("substitute_qty")?).map_err(internal)?,
        )
        .map_err(internal)?,
        revision: ItemSubstitutionPolicyRevision::new(row.try_get("revision")?)
            .map_err(internal)?,
        active: effective_to.is_none(),
        configured_by: UserId::new(row.try_get("configured_by_user_id")?).map_err(internal)?,
        configured_at: row.try_get("configured_at")?,
        retired_by: row
            .try_get::<Option<i64>, _>("retired_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        retired_at: effective_to,
    })
}

async fn close_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ItemSubstitutionPolicyId,
    effective_to: Timestamp,
    retired_by: Option<i64>,
) -> AppResult<()> {
    let updated = sqlx::query(
        "UPDATE item_substitution_policies SET effective_to=$1,retired_by_user_id=$2 WHERE tenant_id=$3 AND id=$4 AND effective_to IS NULL",
    )
    .bind(effective_to)
    .bind(retired_by)
    .bind(tenant_id.get())
    .bind(policy_id.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AppError::conflict("item substitution policy changed"))
    }
}

async fn lock_natural_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &ConfigureItemSubstitutionPolicyCommand,
) -> AppResult<()> {
    lock_key_tx(
        tx,
        tenant_id,
        command.inventory_owner_id,
        command.facility_id,
        &command.definition,
    )
    .await
}

async fn lock_model_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    model: &ItemSubstitutionPolicyReadModel,
) -> AppResult<()> {
    lock_key_tx(
        tx,
        tenant_id,
        model.inventory_owner_id,
        model.facility_id,
        &model.definition,
    )
    .await
}

async fn lock_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    definition: &ItemSubstitutionDefinition,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "item-substitution:{tenant_id}:{owner_id}:{facility_id}:{}:{}:{}:{}",
            definition.source_item_id.get(),
            definition.source_uom,
            definition.substitute_item_id.get(),
            definition.substitute_uom
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn require_replay_visibility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let policy_id: Option<i64> = sqlx::query_scalar(
        r#"SELECT (result_json->>'policy_id')::bigint
           FROM command_idempotency_records
           WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3"#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    let Some(policy_id) = policy_id else {
        return Ok(());
    };
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id FROM item_substitution_policies WHERE tenant_id=$1 AND id=$2",
    )
    .bind(prepared.tenant_id().get())
    .bind(policy_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("item substitution policy"))?;
    require_scope(
        scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
    )
}

fn require_scope(scope: &ScopeBindings, owner_id: i64, facility_id: i64) -> AppResult<()> {
    if scope.includes_inventory_owner(owner_id) && scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::not_found("item substitution policy"))
    }
}

async fn enqueue_policy_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &ItemSubstitutionPolicyReadModel,
    event_type: &str,
) -> AppResult<()> {
    let ordering_key = format!(
        "item-substitution-policy:{}:{}:{}:{}:{}:{}",
        result.inventory_owner_id,
        result.facility_id,
        result.definition.source_item_id.get(),
        result.definition.source_uom,
        result.definition.substitute_item_id.get(),
        result.definition.substitute_uom
    );
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!(
        "{ordering_key}:revision:{}:{event_type}",
        result.revision.get()
    );
    let aggregate_id = result.policy_id.to_string();
    let payload = serde_json::to_value(result).map_err(internal)?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(result.inventory_owner_id),
            facility_id: Some(result.facility_id),
            actor_user_id: Some(result.retired_by.unwrap_or(result.configured_by).get()),
            event_key: &event_key,
            aggregate_type: "item_substitution_policy",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at: result.retired_at.unwrap_or(result.configured_at),
        },
    )
    .await?;
    Ok(())
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}
