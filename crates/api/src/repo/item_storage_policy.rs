//! Versioned item storage compatibility and capacity policy persistence.

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::item_storage_policy::{
    ConfigureItemStoragePolicyCommand, ConfigureItemStoragePolicyResult, ItemStoragePolicyCursor,
    ItemStoragePolicyPage, ItemStoragePolicyPageQuery, ItemStoragePolicyReadModel,
    RetireItemStoragePolicyCommand, RetireItemStoragePolicyResult,
    CONFIGURE_ITEM_STORAGE_POLICY_OPERATION, RETIRE_ITEM_STORAGE_POLICY_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    AllowedStorageZonePurposes, CatalogItemId, FacilityId, InventoryOwnerId,
    ItemStorageLocationCapacity, ItemStoragePolicyDefinition, ItemStoragePolicyId,
    ItemStoragePolicyRevision, ItemStoragePolicyStatus, ItemStoragePolicyUom, StorageZonePurpose,
    TenantId, Timestamp, UserId,
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
    id: ItemStoragePolicyId,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    inventory_owner_name: String,
    facility_id: FacilityId,
    facility_name: String,
    item_id: CatalogItemId,
    item_description: String,
    uom: ItemStoragePolicyUom,
    max_quantity_per_location: Option<ItemStorageLocationCapacity>,
    revision: ItemStoragePolicyRevision,
    status: ItemStoragePolicyStatus,
    configured_by: UserId,
    configured_at: Timestamp,
    retired_by: Option<UserId>,
    retired_at: Option<Timestamp>,
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

fn policy_header(row: &sqlx::postgres::PgRow) -> AppResult<PolicyHeader> {
    let retired_at = row.try_get::<Option<Timestamp>, _>("effective_to")?;
    Ok(PolicyHeader {
        id: ItemStoragePolicyId::new(row.try_get("id")?).map_err(internal)?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?).map_err(internal)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(internal)?,
        facility_name: row.try_get("facility_name")?,
        item_id: CatalogItemId::new(row.try_get("item_id")?).map_err(internal)?,
        item_description: row.try_get("item_description")?,
        uom: ItemStoragePolicyUom::new(row.try_get::<String, _>("uom")?).map_err(internal)?,
        max_quantity_per_location: row
            .try_get::<Option<i64>, _>("max_quantity_per_location")?
            .map(ItemStorageLocationCapacity::new)
            .transpose()
            .map_err(internal)?,
        revision: ItemStoragePolicyRevision::new(row.try_get("revision")?).map_err(internal)?,
        status: if retired_at.is_some() {
            ItemStoragePolicyStatus::Retired
        } else {
            ItemStoragePolicyStatus::Active
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

fn build_policy(
    header: PolicyHeader,
    purposes: Vec<StorageZonePurpose>,
) -> AppResult<ItemStoragePolicyReadModel> {
    Ok(ItemStoragePolicyReadModel {
        item_storage_policy_id: header.id,
        inventory_owner_name: header.inventory_owner_name,
        facility_name: header.facility_name,
        item_description: header.item_description,
        definition: ItemStoragePolicyDefinition {
            tenant_id: header.tenant_id,
            inventory_owner_id: header.inventory_owner_id,
            facility_id: header.facility_id,
            item_id: header.item_id,
            uom: header.uom,
            allowed_zone_purposes: AllowedStorageZonePurposes::new(purposes).map_err(internal)?,
            max_quantity_per_location: header.max_quantity_per_location,
        },
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
        Err(AppError::not_found("item storage policy"))
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
        JOIN item_storage_policies policy
          ON policy.tenant_id=command.tenant_id
         AND policy.id=(command.result_json->>'item_storage_policy_id')::BIGINT
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
    uom: &ItemStoragePolicyUom,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "item_storage_policy:{tenant_id}:{inventory_owner_id}:{facility_id}:{}:{uom}",
            item_id.get()
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn lock_zoned_positions_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &ItemStoragePolicyDefinition,
) -> AppResult<()> {
    let location_ids = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT member.location_id
        FROM storage_zone_locations member
        JOIN storage_zones zone
          ON zone.tenant_id=member.tenant_id AND zone.facility_id=member.facility_id
         AND zone.id=member.storage_zone_id AND zone.effective_to IS NULL
        WHERE member.tenant_id=$1 AND member.facility_id=$2
        ORDER BY member.location_id
        FOR SHARE OF zone
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.facility_id.get())
    .fetch_all(&mut **tx)
    .await?;
    for location_id in &location_ids {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!(
                "inventory-location-item:{}:{}:{}:{}",
                definition.tenant_id,
                definition.inventory_owner_id,
                location_id,
                definition.item_id.get()
            ))
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query(
        r#"
        SELECT id FROM inventory_balances
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND item_id=$4 AND uom=$5 AND location_id=ANY($6)
        ORDER BY id FOR UPDATE
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.inventory_owner_id.get())
    .bind(definition.facility_id.get())
    .bind(definition.item_id.get())
    .bind(definition.uom.as_str())
    .bind(&location_ids)
    .fetch_all(&mut **tx)
    .await?;
    Ok(())
}

async fn validate_current_positions_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &ItemStoragePolicyDefinition,
) -> AppResult<()> {
    let allowed = definition
        .allowed_zone_purposes
        .as_slice()
        .iter()
        .map(|purpose| purpose.as_str().to_owned())
        .collect::<Vec<_>>();
    let incompatible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM inventory_balances balance
            JOIN storage_zone_locations member
              ON member.tenant_id=balance.tenant_id
             AND member.facility_id=balance.facility_id
             AND member.location_id=balance.location_id
            JOIN storage_zones zone
              ON zone.tenant_id=member.tenant_id AND zone.facility_id=member.facility_id
             AND zone.id=member.storage_zone_id AND zone.effective_to IS NULL
            WHERE balance.tenant_id=$1 AND balance.inventory_owner_id=$2
              AND balance.facility_id=$3 AND balance.item_id=$4 AND balance.uom=$5
              AND balance.deleted IS NULL AND balance.qty_on_hand>0
              AND NOT (zone.purpose=ANY($6))
        )
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.inventory_owner_id.get())
    .bind(definition.facility_id.get())
    .bind(definition.item_id.get())
    .bind(definition.uom.as_str())
    .bind(&allowed)
    .fetch_one(&mut **tx)
    .await?;
    if incompatible {
        return Err(AppError::conflict(
            "current inventory occupies a storage-zone purpose not allowed by the policy",
        ));
    }
    if let Some(capacity) = definition.max_quantity_per_location {
        let over_capacity: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM inventory_balances balance
                JOIN storage_zone_locations member
                  ON member.tenant_id=balance.tenant_id
                 AND member.facility_id=balance.facility_id
                 AND member.location_id=balance.location_id
                JOIN storage_zones zone
                  ON zone.tenant_id=member.tenant_id AND zone.facility_id=member.facility_id
                 AND zone.id=member.storage_zone_id AND zone.effective_to IS NULL
                WHERE balance.tenant_id=$1 AND balance.inventory_owner_id=$2
                  AND balance.facility_id=$3 AND balance.item_id=$4 AND balance.uom=$5
                  AND balance.deleted IS NULL
                GROUP BY balance.location_id
                HAVING sum(balance.qty_on_hand)>$6
            )
            "#,
        )
        .bind(definition.tenant_id.get())
        .bind(definition.inventory_owner_id.get())
        .bind(definition.facility_id.get())
        .bind(definition.item_id.get())
        .bind(definition.uom.as_str())
        .bind(capacity.get())
        .fetch_one(&mut **tx)
        .await?;
        if over_capacity {
            return Err(AppError::conflict(
                "current inventory exceeds the requested per-location capacity",
            ));
        }
    }
    Ok(())
}

async fn latest_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &ItemStoragePolicyDefinition,
) -> AppResult<Option<(ItemStoragePolicyId, ItemStoragePolicyRevision, bool)>> {
    let row = sqlx::query(
        r#"
        SELECT id,revision,effective_to IS NULL AS active
        FROM item_storage_policies
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
            ItemStoragePolicyId::new(row.try_get("id")?).map_err(internal)?,
            ItemStoragePolicyRevision::new(row.try_get("revision")?).map_err(internal)?,
            row.try_get("active")?,
        ))
    })
    .transpose()
}

async fn retire_row_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ItemStoragePolicyId,
    actor_id: i64,
    retired_at: Timestamp,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE item_storage_policies SET effective_to=$3,retired_by_user_id=$4 WHERE tenant_id=$1 AND id=$2 AND effective_to IS NULL",
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
    command: &ConfigureItemStoragePolicyCommand,
    revision: ItemStoragePolicyRevision,
    predecessor: Option<ItemStoragePolicyId>,
    actor_id: i64,
    configured_at: Timestamp,
) -> AppResult<ItemStoragePolicyId> {
    let definition = &command.definition;
    let purpose_count = i64::try_from(definition.allowed_zone_purposes.as_slice().len())
        .map_err(|_| AppError::bad_request("too many allowed storage-zone purposes"))?;
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO item_storage_policies
            (tenant_id,inventory_owner_id,facility_id,item_id,uom,
             max_quantity_per_location,revision,supersedes_item_storage_policy_id,
             allowed_purpose_count,effective_from,configured_by_user_id,configured_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$10)
        RETURNING id
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.inventory_owner_id.get())
    .bind(definition.facility_id.get())
    .bind(definition.item_id.get())
    .bind(definition.uom.as_str())
    .bind(
        definition
            .max_quantity_per_location
            .map(ItemStorageLocationCapacity::get),
    )
    .bind(revision.get())
    .bind(predecessor.map(ItemStoragePolicyId::get))
    .bind(purpose_count)
    .bind(configured_at)
    .bind(actor_id)
    .fetch_one(&mut **tx)
    .await?;
    ItemStoragePolicyId::new(id).map_err(internal)
}

async fn insert_purposes_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &ItemStoragePolicyDefinition,
    policy_id: ItemStoragePolicyId,
) -> AppResult<()> {
    for (index, purpose) in definition
        .allowed_zone_purposes
        .as_slice()
        .iter()
        .enumerate()
    {
        sqlx::query(
            r#"
            INSERT INTO item_storage_policy_zone_purposes
                (tenant_id,inventory_owner_id,facility_id,item_storage_policy_id,
                 purpose,purpose_sequence)
            VALUES ($1,$2,$3,$4,$5,$6)
            "#,
        )
        .bind(definition.tenant_id.get())
        .bind(definition.inventory_owner_id.get())
        .bind(definition.facility_id.get())
        .bind(policy_id.get())
        .bind(purpose.as_str())
        .bind(
            i64::try_from(index + 1)
                .map_err(|_| AppError::bad_request("too many allowed storage-zone purposes"))?,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn purposes_for_policies_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_ids: &[i64],
) -> AppResult<HashMap<i64, Vec<StorageZonePurpose>>> {
    if policy_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT item_storage_policy_id,purpose
        FROM item_storage_policy_zone_purposes
        WHERE tenant_id=$1 AND item_storage_policy_id=ANY($2)
        ORDER BY item_storage_policy_id,purpose_sequence
        "#,
    )
    .bind(tenant_id.get())
    .bind(policy_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut grouped = HashMap::<i64, Vec<StorageZonePurpose>>::new();
    for row in rows {
        let raw: String = row.try_get("purpose")?;
        let purpose = StorageZonePurpose::parse(&raw)
            .ok_or_else(|| AppError::internal(format!("invalid storage-zone purpose: {raw}")))?;
        grouped
            .entry(row.try_get("item_storage_policy_id")?)
            .or_default()
            .push(purpose);
    }
    Ok(grouped)
}

const POLICY_SELECT: &str = r#"
    SELECT policy.*,owner.name AS inventory_owner_name,
           facility.name AS facility_name,
           COALESCE(item.description,'Item #' || item.id::TEXT) AS item_description
    FROM item_storage_policies policy
    JOIN inventory_owners owner
      ON owner.tenant_id=policy.tenant_id AND owner.id=policy.inventory_owner_id
    JOIN facilities facility
      ON facility.tenant_id=policy.tenant_id AND facility.id=policy.facility_id
    JOIN items item ON item.tenant_id=policy.tenant_id AND item.id=policy.item_id
"#;

async fn read_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy_id: ItemStoragePolicyId,
) -> AppResult<ItemStoragePolicyReadModel> {
    let query = format!("{POLICY_SELECT} WHERE policy.tenant_id=$1 AND policy.id=$2");
    let row = sqlx::query(&query)
        .bind(tenant_id.get())
        .bind(policy_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("item storage policy"))?;
    let mut purposes = purposes_for_policies_tx(tx, tenant_id, &[policy_id.get()]).await?;
    build_policy(
        policy_header(&row)?,
        purposes.remove(&policy_id.get()).unwrap_or_default(),
    )
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    actor_id: i64,
    policy_id: ItemStoragePolicyId,
    transition: &str,
    occurred_at: Timestamp,
    payload: &serde_json::Value,
) -> AppResult<()> {
    let event_key = format!("item-storage-policy:{}:{transition}", policy_id.get());
    let aggregate_id = policy_id.get().to_string();
    let ordering_key = format!("item-storage-policy:{}", policy_id.get());
    let event_type = format!("topology.item_storage_policy.{transition}");
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(actor_id),
            event_key: &event_key,
            aggregate_type: "item_storage_policy",
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

pub async fn configure_item_storage_policy(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureItemStoragePolicyCommand,
) -> AppResult<ConfigureItemStoragePolicyResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.definition.tenant_id != access.tenant_id {
        return Err(AppError::not_found("item storage policy"));
    }
    let prepared =
        PreparedCommand::new_v1(context, CONFIGURE_ITEM_STORAGE_POLICY_OPERATION, command)?;
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
    let predecessor = latest_policy_tx(&mut tx, &command.definition).await?;
    match (predecessor, command.expected_revision) {
        (None, None) | (Some((_, _, false)), None) => {}
        (Some((_, revision, true)), Some(expected)) if revision == expected => {}
        (Some((_, _, true)), None) => {
            return Err(AppError::conflict("item storage policy already exists"));
        }
        (None, Some(_)) | (Some((_, _, false)), Some(_)) => {
            return Err(AppError::conflict(
                "item storage policy has no active revision",
            ));
        }
        (Some((_, _, true)), Some(_)) => {
            return Err(AppError::conflict(
                "item storage policy revision does not match expected revision",
            ));
        }
    }
    lock_zoned_positions_tx(&mut tx, &command.definition).await?;
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
            .ok_or_else(|| AppError::internal("item storage policy revision overflow"))?,
        None => ItemStoragePolicyRevision::new(1).map_err(internal)?,
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
    insert_purposes_tx(&mut tx, &command.definition, policy_id).await?;
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

pub async fn retire_item_storage_policy(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RetireItemStoragePolicyCommand,
) -> AppResult<RetireItemStoragePolicyResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, RETIRE_ITEM_STORAGE_POLICY_OPERATION, command)?;
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
        FROM item_storage_policies WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.item_storage_policy_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("item storage policy"))?;
    let inventory_owner_id =
        InventoryOwnerId::new(hint.try_get("inventory_owner_id")?).map_err(internal)?;
    let facility_id = FacilityId::new(hint.try_get("facility_id")?).map_err(internal)?;
    let item_id = CatalogItemId::new(hint.try_get("item_id")?).map_err(internal)?;
    let uom = ItemStoragePolicyUom::new(hint.try_get::<String, _>("uom")?).map_err(internal)?;
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
        "SELECT revision,effective_to FROM item_storage_policies WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(access.tenant_id.get())
    .bind(command.item_storage_policy_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("item storage policy"))?;
    if row
        .try_get::<Option<Timestamp>, _>("effective_to")?
        .is_some()
    {
        return Err(AppError::conflict("item storage policy is already retired"));
    }
    let revision = ItemStoragePolicyRevision::new(row.try_get("revision")?).map_err(internal)?;
    if revision != command.expected_revision {
        return Err(AppError::conflict(
            "item storage policy revision does not match expected revision",
        ));
    }
    let purposes = purposes_for_policies_tx(
        &mut tx,
        access.tenant_id,
        &[command.item_storage_policy_id.get()],
    )
    .await?
    .remove(&command.item_storage_policy_id.get())
    .unwrap_or_default();
    let definition = ItemStoragePolicyDefinition {
        tenant_id: access.tenant_id,
        inventory_owner_id,
        facility_id,
        item_id,
        uom,
        allowed_zone_purposes: AllowedStorageZonePurposes::new(purposes).map_err(internal)?,
        max_quantity_per_location: None,
    };
    lock_zoned_positions_tx(&mut tx, &definition).await?;
    let retired_at = now_iso();
    retire_row_tx(
        &mut tx,
        access.tenant_id,
        command.item_storage_policy_id,
        context.actor_id.get(),
        retired_at,
    )
    .await?;
    let result = read_policy_tx(&mut tx, access.tenant_id, command.item_storage_policy_id).await?;
    let payload = serde_json::to_value(&result).map_err(internal)?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        inventory_owner_id,
        facility_id,
        context.actor_id.get(),
        command.item_storage_policy_id,
        "retired",
        retired_at,
        &payload,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn item_storage_policy_page(
    db: &Db,
    access: &TenantAccess,
    query: ItemStoragePolicyPageQuery,
) -> AppResult<ItemStoragePolicyPage> {
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
        return Err(AppError::not_found("item storage policy"));
    }
    let status = query.status.map(|status| match status {
        ItemStoragePolicyStatus::Active => "active",
        ItemStoragePolicyStatus::Retired => "retired",
    });
    let purpose = query.purpose.map(StorageZonePurpose::as_str);
    let sql = format!(
        r#"
        {POLICY_SELECT}
        WHERE policy.tenant_id=$1
          AND ($2 OR policy.inventory_owner_id=ANY($3))
          AND ($4 OR policy.facility_id=ANY($5))
          AND ($6::BIGINT IS NULL OR policy.inventory_owner_id=$6)
          AND ($7::BIGINT IS NULL OR policy.facility_id=$7)
          AND ($8::BIGINT IS NULL OR policy.item_id=$8)
          AND ($9::TEXT IS NULL OR EXISTS (
              SELECT 1 FROM item_storage_policy_zone_purposes purpose
              WHERE purpose.tenant_id=policy.tenant_id
                AND purpose.item_storage_policy_id=policy.id AND purpose.purpose=$9))
          AND (($10::TEXT IS NULL AND policy.effective_to IS NULL)
               OR ($10='active' AND policy.effective_to IS NULL)
               OR ($10='retired' AND policy.effective_to IS NOT NULL))
          AND ($11::BIGINT IS NULL OR policy.id>$11)
        ORDER BY policy.id LIMIT $12
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
        .bind(purpose)
        .bind(status)
        .bind(
            query
                .cursor
                .map(|cursor| cursor.after_item_storage_policy_id.get()),
        )
        .bind(i64::from(query.limit) + 1)
        .fetch_all(&mut *tx)
        .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let headers = rows
        .into_iter()
        .take(usize::from(query.limit))
        .map(|row| policy_header(&row))
        .collect::<AppResult<Vec<_>>>()?;
    let ids = headers
        .iter()
        .map(|header| header.id.get())
        .collect::<Vec<_>>();
    let mut purposes = purposes_for_policies_tx(&mut tx, access.tenant_id, &ids).await?;
    let mut items = Vec::with_capacity(headers.len());
    for header in headers {
        let id = header.id.get();
        items.push(build_policy(
            header,
            purposes.remove(&id).unwrap_or_default(),
        )?);
    }
    let next_cursor = if has_more {
        items.last().map(|item| ItemStoragePolicyCursor {
            after_item_storage_policy_id: item.item_storage_policy_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(ItemStoragePolicyPage { items, next_cursor })
}
