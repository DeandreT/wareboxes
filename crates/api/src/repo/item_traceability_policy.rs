//! Versioned item identity and shelf-life policy persistence.

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::item_traceability_policy::{
    ConfigureItemTraceabilityPolicyCommand, ConfigureItemTraceabilityPolicyResult,
    ItemTraceabilityPolicyCursor, ItemTraceabilityPolicyPage, ItemTraceabilityPolicyPageQuery,
    ItemTraceabilityPolicyReadModel, RetireItemTraceabilityPolicyCommand,
    RetireItemTraceabilityPolicyResult, CONFIGURE_ITEM_TRACEABILITY_POLICY_OPERATION,
    RETIRE_ITEM_TRACEABILITY_POLICY_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryOwnerId, ItemTraceabilityPolicyDefinition,
    ItemTraceabilityPolicyId, ItemTraceabilityPolicyRevision, ItemTraceabilityPolicyStatus,
    ItemTraceabilityPolicyUom, MinimumShelfLifeDays, TenantId, Timestamp, TraceabilityRequirement,
    UserId,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::next_outbox_sequence_tx;

const READ_PERMISSION: &str = "wms";
const SUPERVISOR_PERMISSION: &str = "wms_supervisor";

#[derive(Debug)]
struct PolicyHeader {
    id: ItemTraceabilityPolicyId,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    inventory_owner_name: String,
    facility_id: FacilityId,
    facility_name: String,
    item_id: CatalogItemId,
    item_description: String,
    uom: ItemTraceabilityPolicyUom,
    lot: TraceabilityRequirement,
    serial: TraceabilityRequirement,
    expiration: TraceabilityRequirement,
    minimum_shelf_life_days: Option<MinimumShelfLifeDays>,
    revision: ItemTraceabilityPolicyRevision,
    status: ItemTraceabilityPolicyStatus,
    configured_by: UserId,
    configured_at: Timestamp,
    retired_by: Option<UserId>,
    retired_at: Option<Timestamp>,
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

fn requirement(value: &str) -> AppResult<TraceabilityRequirement> {
    match value {
        "not_tracked" => Ok(TraceabilityRequirement::NotTracked),
        "required" => Ok(TraceabilityRequirement::Required),
        other => Err(AppError::internal(format!(
            "invalid traceability requirement: {other}"
        ))),
    }
}

const fn requirement_name(value: TraceabilityRequirement) -> &'static str {
    match value {
        TraceabilityRequirement::NotTracked => "not_tracked",
        TraceabilityRequirement::Required => "required",
    }
}

fn policy_header(row: &sqlx::postgres::PgRow) -> AppResult<PolicyHeader> {
    let retired_at = row.try_get::<Option<Timestamp>, _>("effective_to")?;
    Ok(PolicyHeader {
        id: ItemTraceabilityPolicyId::new(row.try_get("id")?).map_err(internal)?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?).map_err(internal)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        facility_name: row.try_get("facility_name")?,
        item_id: CatalogItemId::new(row.try_get("item_id")?).map_err(internal)?,
        item_description: row.try_get("item_description")?,
        uom: ItemTraceabilityPolicyUom::new(row.try_get::<String, _>("uom")?).map_err(internal)?,
        lot: requirement(&row.try_get::<String, _>("lot_requirement")?)?,
        serial: requirement(&row.try_get::<String, _>("serial_requirement")?)?,
        expiration: requirement(&row.try_get::<String, _>("expiration_requirement")?)?,
        minimum_shelf_life_days: row
            .try_get::<Option<i64>, _>("minimum_shelf_life_days")?
            .map(|value| {
                u32::try_from(value)
                    .map_err(internal)
                    .and_then(|value| MinimumShelfLifeDays::new(value).map_err(internal))
            })
            .transpose()?,
        revision: ItemTraceabilityPolicyRevision::new(row.try_get("revision")?)
            .map_err(internal)?,
        status: if retired_at.is_some() {
            ItemTraceabilityPolicyStatus::Retired
        } else {
            ItemTraceabilityPolicyStatus::Active
        },
        configured_by: UserId::new(row.try_get("configured_by_user_id")?).map_err(internal)?,
        configured_at: row.try_get("configured_at")?,
        retired_by: row
            .try_get::<Option<i64>, _>("retired_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        retired_at,
    })
}

fn build_policy(header: PolicyHeader) -> AppResult<ItemTraceabilityPolicyReadModel> {
    Ok(ItemTraceabilityPolicyReadModel {
        item_traceability_policy_id: header.id,
        inventory_owner_name: header.inventory_owner_name,
        facility_name: header.facility_name,
        item_description: header.item_description,
        definition: ItemTraceabilityPolicyDefinition::new(
            header.tenant_id,
            header.inventory_owner_id,
            header.facility_id,
            header.item_id,
            header.uom,
            header.lot,
            header.serial,
            header.expiration,
            header.minimum_shelf_life_days,
        )
        .map_err(internal)?,
        status: header.status,
        revision: header.revision,
        configured_by: header.configured_by,
        configured_at: header.configured_at,
        retired_by: header.retired_by,
        retired_at: header.retired_at,
    })
}

fn require_scope(
    scope: &ScopeBindings,
    inventory_owner_id: i64,
    facility_id: i64,
) -> AppResult<()> {
    if scope.includes_inventory_owner(inventory_owner_id) && scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::not_found("item traceability policy"))
    }
}

async fn require_stored_policy_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT policy.inventory_owner_id,policy.facility_id
        FROM command_idempotency_records command
        JOIN item_traceability_policies policy
          ON policy.tenant_id=command.tenant_id
         AND policy.id=(command.result_json->>'item_traceability_policy_id')::BIGINT
        WHERE command.tenant_id=$1 AND command.operation=$2 AND command.idempotency_key=$3
        "#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = row {
        require_scope(
            scope,
            row.try_get("inventory_owner_id")?,
            row.try_get("facility_id")?,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn lock_natural_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    item_id: CatalogItemId,
    uom: &ItemTraceabilityPolicyUom,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "item_traceability_policy:{tenant_id}:{inventory_owner_id}:{facility_id}:{}:{uom}",
            item_id.get()
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn require_configuration_scope_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &ItemTraceabilityPolicyDefinition,
) -> AppResult<()> {
    let available: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT item.id
        FROM inventory_owners owner
        JOIN inventory_owner_facilities assignment
          ON assignment.tenant_id=owner.tenant_id
         AND assignment.inventory_owner_id=owner.id
         AND assignment.facility_id=$3
         AND assignment.deleted IS NULL
        JOIN facilities facility
          ON facility.tenant_id=assignment.tenant_id
         AND facility.id=assignment.facility_id
         AND facility.deleted IS NULL
        JOIN inventory_owner_items owner_item
          ON owner_item.tenant_id=owner.tenant_id
         AND owner_item.inventory_owner_id=owner.id
         AND owner_item.item_id=$4
         AND owner_item.deleted IS NULL
        JOIN items item
          ON item.tenant_id=owner_item.tenant_id
         AND item.id=owner_item.item_id
         AND item.deleted IS NULL
         AND item.packaging_unit=$5
        WHERE owner.tenant_id=$1 AND owner.id=$2 AND owner.deleted IS NULL
        FOR SHARE OF owner,assignment,facility,owner_item,item
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.inventory_owner_id.get())
    .bind(definition.facility_id.get())
    .bind(definition.item_id.get())
    .bind(definition.uom.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    if available.is_none() {
        return Err(AppError::not_found("item traceability policy"));
    }
    Ok(())
}

async fn lock_current_positions_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &ItemTraceabilityPolicyDefinition,
) -> AppResult<()> {
    let mut batch_ids = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT item_batch_id FROM inventory_balances
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND item_id=$4 AND uom=$5 AND deleted IS NULL AND qty_on_hand>0
        ORDER BY id FOR UPDATE
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.inventory_owner_id.get())
    .bind(definition.facility_id.get())
    .bind(definition.item_id.get())
    .bind(definition.uom.as_str())
    .fetch_all(&mut **tx)
    .await?;
    batch_ids.sort_unstable();
    batch_ids.dedup();
    if !batch_ids.is_empty() {
        sqlx::query(
            "SELECT id FROM item_batches WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id FOR SHARE",
        )
        .bind(definition.tenant_id.get())
        .bind(&batch_ids)
        .fetch_all(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn validate_current_positions_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &ItemTraceabilityPolicyDefinition,
) -> AppResult<()> {
    let incompatible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM inventory_balances balance
            JOIN item_batches batch
              ON batch.tenant_id=balance.tenant_id
             AND batch.inventory_owner_id=balance.inventory_owner_id
             AND batch.id=balance.item_batch_id
            WHERE balance.tenant_id=$1 AND balance.inventory_owner_id=$2
              AND balance.facility_id=$3 AND balance.item_id=$4 AND balance.uom=$5
              AND balance.deleted IS NULL AND balance.qty_on_hand>0
              AND (($6='required' AND batch.lot IS NULL)
                OR ($6='not_tracked' AND batch.lot IS NOT NULL)
                OR ($7='required' AND batch.serial IS NULL)
                OR ($7='not_tracked' AND batch.serial IS NOT NULL)
                OR ($8='required' AND batch.expiration IS NULL)
                OR ($8='not_tracked' AND batch.expiration IS NOT NULL)
                OR ($9::BIGINT IS NOT NULL AND batch.expiration < batch.created
                    + make_interval(days => $9::INTEGER)))
        ) OR ($7='required' AND EXISTS(
            SELECT 1
            FROM inventory_balances balance
            JOIN item_batches batch
              ON batch.tenant_id=balance.tenant_id
             AND batch.inventory_owner_id=balance.inventory_owner_id
             AND batch.id=balance.item_batch_id
            WHERE balance.tenant_id=$1 AND balance.inventory_owner_id=$2
              AND balance.facility_id=$3 AND balance.item_id=$4 AND balance.uom=$5
              AND balance.deleted IS NULL AND balance.qty_on_hand>0
            GROUP BY batch.serial
            HAVING batch.serial IS NULL OR sum(balance.qty_on_hand)>1
        ))
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.inventory_owner_id.get())
    .bind(definition.facility_id.get())
    .bind(definition.item_id.get())
    .bind(definition.uom.as_str())
    .bind(requirement_name(definition.lot))
    .bind(requirement_name(definition.serial))
    .bind(requirement_name(definition.expiration))
    .bind(
        definition
            .minimum_shelf_life_days
            .map(|days| i64::from(days.get())),
    )
    .fetch_one(&mut **tx)
    .await?;
    if incompatible {
        return Err(AppError::conflict(
            "current inventory does not satisfy the requested traceability policy",
        ));
    }
    Ok(())
}

async fn latest_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &ItemTraceabilityPolicyDefinition,
) -> AppResult<
    Option<(
        ItemTraceabilityPolicyId,
        ItemTraceabilityPolicyRevision,
        bool,
    )>,
> {
    let row = sqlx::query(
        r#"
        SELECT id,revision,effective_to IS NULL AS active
        FROM item_traceability_policies
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND item_id=$4 AND uom=$5
        ORDER BY revision DESC LIMIT 1 FOR UPDATE
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.inventory_owner_id.get())
    .bind(definition.facility_id.get())
    .bind(definition.item_id.get())
    .bind(definition.uom.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok((
            ItemTraceabilityPolicyId::new(row.try_get("id")?).map_err(internal)?,
            ItemTraceabilityPolicyRevision::new(row.try_get("revision")?).map_err(internal)?,
            row.try_get("active")?,
        ))
    })
    .transpose()
}

async fn retire_row_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ItemTraceabilityPolicyId,
    actor_id: i64,
    retired_at: Timestamp,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE item_traceability_policies SET effective_to=$3,retired_by_user_id=$4 WHERE tenant_id=$1 AND id=$2 AND effective_to IS NULL",
    )
    .bind(tenant_id.get())
    .bind(policy_id.get())
    .bind(retired_at)
    .bind(actor_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &ConfigureItemTraceabilityPolicyCommand,
    revision: ItemTraceabilityPolicyRevision,
    predecessor: Option<ItemTraceabilityPolicyId>,
    actor_id: i64,
    configured_at: Timestamp,
) -> AppResult<ItemTraceabilityPolicyId> {
    let definition = &command.definition;
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO item_traceability_policies
            (tenant_id,inventory_owner_id,facility_id,item_id,uom,lot_requirement,
             serial_requirement,expiration_requirement,minimum_shelf_life_days,
             revision,supersedes_item_traceability_policy_id,effective_from,
             configured_by_user_id,configured_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$12)
        RETURNING id
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.inventory_owner_id.get())
    .bind(definition.facility_id.get())
    .bind(definition.item_id.get())
    .bind(definition.uom.as_str())
    .bind(requirement_name(definition.lot))
    .bind(requirement_name(definition.serial))
    .bind(requirement_name(definition.expiration))
    .bind(
        definition
            .minimum_shelf_life_days
            .map(|days| i64::from(days.get())),
    )
    .bind(revision.get())
    .bind(predecessor.map(ItemTraceabilityPolicyId::get))
    .bind(configured_at)
    .bind(actor_id)
    .fetch_one(&mut **tx)
    .await?;
    ItemTraceabilityPolicyId::new(id).map_err(internal)
}

const POLICY_SELECT: &str = r#"
    SELECT policy.*,owner.name AS inventory_owner_name,
           facility.name AS facility_name,
           COALESCE(item.description,'Item #' || item.id::TEXT) AS item_description
    FROM item_traceability_policies policy
    JOIN inventory_owners owner
      ON owner.tenant_id=policy.tenant_id AND owner.id=policy.inventory_owner_id
    JOIN facilities facility
      ON facility.tenant_id=policy.tenant_id AND facility.id=policy.facility_id
    JOIN items item ON item.tenant_id=policy.tenant_id AND item.id=policy.item_id
"#;

async fn read_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ItemTraceabilityPolicyId,
) -> AppResult<ItemTraceabilityPolicyReadModel> {
    let query = format!("{POLICY_SELECT} WHERE policy.tenant_id=$1 AND policy.id=$2");
    let row = sqlx::query(&query)
        .bind(tenant_id.get())
        .bind(policy_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("item traceability policy"))?;
    build_policy(policy_header(&row)?)
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    actor_id: i64,
    policy_id: ItemTraceabilityPolicyId,
    transition: &str,
    occurred_at: Timestamp,
    payload: &serde_json::Value,
) -> AppResult<()> {
    let event_key = format!("item-traceability-policy:{}:{transition}", policy_id.get());
    let aggregate_id = policy_id.get().to_string();
    let ordering_key = format!("item-traceability-policy:{}", policy_id.get());
    let event_type = format!("inventory.item_traceability_policy.{transition}");
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(actor_id),
            event_key: &event_key,
            aggregate_type: "item_traceability_policy",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: &event_type,
            schema_version: 1,
            payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

pub async fn configure_item_traceability_policy(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureItemTraceabilityPolicyCommand,
) -> AppResult<ConfigureItemTraceabilityPolicyResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.definition.tenant_id != access.tenant_id {
        return Err(AppError::not_found("item traceability policy"));
    }
    let prepared = PreparedCommand::new_v1(
        context,
        CONFIGURE_ITEM_TRACEABILITY_POLICY_OPERATION,
        command,
    )?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    require_stored_policy_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    require_scope(
        &scope,
        command.definition.inventory_owner_id.get(),
        command.definition.facility_id.get(),
    )?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }

    lock_natural_key_tx(
        &mut tx,
        command.definition.tenant_id,
        command.definition.inventory_owner_id,
        command.definition.facility_id,
        command.definition.item_id,
        &command.definition.uom,
    )
    .await?;
    require_configuration_scope_tx(&mut tx, &command.definition).await?;
    let predecessor = latest_policy_tx(&mut tx, &command.definition).await?;
    match (predecessor, command.expected_revision) {
        (None, None) | (Some((_, _, false)), None) => {}
        (Some((_, revision, true)), Some(expected)) if revision == expected => {}
        (Some((_, _, true)), None) => {
            return Err(AppError::conflict(
                "item traceability policy already exists",
            ));
        }
        (None, Some(_)) | (Some((_, _, false)), Some(_)) => {
            return Err(AppError::conflict(
                "item traceability policy has no active revision",
            ));
        }
        (Some((_, _, true)), Some(_)) => {
            return Err(AppError::conflict(
                "item traceability policy revision does not match expected revision",
            ));
        }
    }
    lock_current_positions_tx(&mut tx, &command.definition).await?;
    validate_current_positions_tx(&mut tx, &command.definition).await?;

    let configured_at = now_iso();
    if let Some((predecessor_id, _, true)) = predecessor {
        retire_row_tx(
            &mut tx,
            access.tenant_id,
            predecessor_id,
            context.actor_id.get(),
            configured_at,
        )
        .await?;
    }
    let revision = match predecessor {
        Some((_, revision, _)) => revision
            .checked_next()
            .ok_or_else(|| AppError::internal("item traceability policy revision overflow"))?,
        None => ItemTraceabilityPolicyRevision::new(1).map_err(internal)?,
    };
    let policy_id = insert_policy_tx(
        &mut tx,
        command,
        revision,
        predecessor.map(|(id, _, _)| id),
        context.actor_id.get(),
        configured_at,
    )
    .await?;
    let result = read_policy_tx(&mut tx, access.tenant_id, policy_id).await?;
    let payload = serde_json::to_value(&result).map_err(internal)?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        command.definition.inventory_owner_id,
        command.definition.facility_id,
        context.actor_id.get(),
        policy_id,
        "configured",
        configured_at,
        &payload,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn retire_item_traceability_policy(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RetireItemTraceabilityPolicyCommand,
) -> AppResult<RetireItemTraceabilityPolicyResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared =
        PreparedCommand::new_v1(context, RETIRE_ITEM_TRACEABILITY_POLICY_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    require_stored_policy_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }

    let hint = sqlx::query(
        r#"
        SELECT inventory_owner_id,facility_id,item_id,uom
        FROM item_traceability_policies WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.item_traceability_policy_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("item traceability policy"))?;
    let inventory_owner_id =
        InventoryOwnerId::new(hint.try_get("inventory_owner_id")?).map_err(internal)?;
    let facility_id = FacilityId::new(hint.try_get("facility_id")?).map_err(internal)?;
    let item_id = CatalogItemId::new(hint.try_get("item_id")?).map_err(internal)?;
    let uom =
        ItemTraceabilityPolicyUom::new(hint.try_get::<String, _>("uom")?).map_err(internal)?;
    require_scope(&scope, inventory_owner_id.get(), facility_id.get())?;
    lock_natural_key_tx(
        &mut tx,
        access.tenant_id,
        inventory_owner_id,
        facility_id,
        item_id,
        &uom,
    )
    .await?;
    let row = sqlx::query(
        "SELECT revision,effective_to FROM item_traceability_policies WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(access.tenant_id.get())
    .bind(command.item_traceability_policy_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("item traceability policy"))?;
    if row
        .try_get::<Option<Timestamp>, _>("effective_to")?
        .is_some()
    {
        return Err(AppError::conflict(
            "item traceability policy is already retired",
        ));
    }
    let revision =
        ItemTraceabilityPolicyRevision::new(row.try_get("revision")?).map_err(internal)?;
    if revision != command.expected_revision {
        return Err(AppError::conflict(
            "item traceability policy revision does not match expected revision",
        ));
    }
    let retired_at = now_iso();
    retire_row_tx(
        &mut tx,
        access.tenant_id,
        command.item_traceability_policy_id,
        context.actor_id.get(),
        retired_at,
    )
    .await?;
    let result = read_policy_tx(
        &mut tx,
        access.tenant_id,
        command.item_traceability_policy_id,
    )
    .await?;
    let payload = serde_json::to_value(&result).map_err(internal)?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        inventory_owner_id,
        facility_id,
        context.actor_id.get(),
        command.item_traceability_policy_id,
        "retired",
        retired_at,
        &payload,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn item_traceability_policy_page(
    db: &Db,
    access: &TenantAccess,
    query: ItemTraceabilityPolicyPageQuery,
) -> AppResult<ItemTraceabilityPolicyPage> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        READ_PERMISSION,
    )
    .await?;
    if query
        .inventory_owner_id
        .is_some_and(|id| !scope.includes_inventory_owner(id.get()))
        || query
            .facility_id
            .is_some_and(|id| !scope.includes_facility(id.get()))
    {
        return Err(AppError::not_found("item traceability policy"));
    }
    let status = query.status.map(|status| match status {
        ItemTraceabilityPolicyStatus::Active => "active",
        ItemTraceabilityPolicyStatus::Retired => "retired",
    });
    let sql = format!(
        r#"
        {POLICY_SELECT}
        WHERE policy.tenant_id=$1
          AND ($2 OR policy.inventory_owner_id=ANY($3))
          AND ($4 OR policy.facility_id=ANY($5))
          AND ($6::BIGINT IS NULL OR policy.inventory_owner_id=$6)
          AND ($7::BIGINT IS NULL OR policy.facility_id=$7)
          AND ($8::BIGINT IS NULL OR policy.item_id=$8)
          AND ($9::TEXT IS NULL OR policy.lot_requirement=$9)
          AND ($10::TEXT IS NULL OR policy.serial_requirement=$10)
          AND ($11::TEXT IS NULL OR policy.expiration_requirement=$11)
          AND (($12::TEXT IS NULL AND policy.effective_to IS NULL)
               OR ($12='active' AND policy.effective_to IS NULL)
               OR ($12='retired' AND policy.effective_to IS NOT NULL))
          AND ($13::BIGINT IS NULL OR policy.id>$13)
        ORDER BY policy.id LIMIT $14
        "#
    );
    let rows = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(query.facility_id.map(FacilityId::get))
        .bind(query.item_id.map(CatalogItemId::get))
        .bind(query.lot.map(requirement_name))
        .bind(query.serial.map(requirement_name))
        .bind(query.expiration.map(requirement_name))
        .bind(status)
        .bind(
            query
                .cursor
                .map(|cursor| cursor.after_item_traceability_policy_id.get()),
        )
        .bind(i64::from(query.limit) + 1)
        .fetch_all(&mut *tx)
        .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let items = rows
        .into_iter()
        .take(usize::from(query.limit))
        .map(|row| policy_header(&row).and_then(build_policy))
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if has_more {
        items.last().map(|item| ItemTraceabilityPolicyCursor {
            after_item_traceability_policy_id: item.item_traceability_policy_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(ItemTraceabilityPolicyPage { items, next_cursor })
}
