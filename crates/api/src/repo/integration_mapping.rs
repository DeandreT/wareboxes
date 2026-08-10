//! Versioned partner order item mapping persistence.

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::integration_mapping::{
    ConfigureIntegrationOrderItemMappingCommand, ConfigureIntegrationOrderItemMappingResult,
    IntegrationOrderItemMappingCursor, IntegrationOrderItemMappingPage,
    IntegrationOrderItemMappingPageQuery, IntegrationOrderItemMappingReadModel,
    RetireIntegrationOrderItemMappingCommand, RetireIntegrationOrderItemMappingResult,
    CONFIGURE_INTEGRATION_ORDER_ITEM_MAPPING_OPERATION,
    RETIRE_INTEGRATION_ORDER_ITEM_MAPPING_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CatalogItemId, ExternalItemKey, ExternalItemUom, IntegrationMappedUom,
    IntegrationOrderItemMappingDefinition, IntegrationOrderItemMappingId,
    IntegrationOrderItemMappingRevision, IntegrationOrderItemMappingStatus, IntegrationSourceKey,
    InventoryOwnerId, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::next_outbox_sequence_tx;

const PERMISSION: &str = "admin";

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

fn status_from(retired_at: Option<Timestamp>) -> IntegrationOrderItemMappingStatus {
    if retired_at.is_some() {
        IntegrationOrderItemMappingStatus::Retired
    } else {
        IntegrationOrderItemMappingStatus::Active
    }
}

fn map_row(row: &sqlx::postgres::PgRow) -> AppResult<IntegrationOrderItemMappingReadModel> {
    let retired_at = row.try_get("effective_to")?;
    Ok(IntegrationOrderItemMappingReadModel {
        mapping_id: IntegrationOrderItemMappingId::new(row.try_get("id")?).map_err(internal)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        item_description: row.try_get("item_description")?,
        definition: IntegrationOrderItemMappingDefinition {
            tenant_id: TenantId::new(row.try_get("tenant_id")?).map_err(internal)?,
            inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
                .map_err(internal)?,
            source_key: IntegrationSourceKey::new(row.try_get::<String, _>("source_key")?)
                .map_err(internal)?,
            external_item_key: ExternalItemKey::new(row.try_get::<String, _>("external_item_key")?)
                .map_err(internal)?,
            external_uom: ExternalItemUom::new(row.try_get::<String, _>("external_uom")?)
                .map_err(internal)?,
            item_id: CatalogItemId::new(row.try_get("item_id")?).map_err(internal)?,
            requested_uom: IntegrationMappedUom::new(row.try_get::<String, _>("requested_uom")?)
                .map_err(internal)?,
        },
        status: status_from(retired_at),
        revision: IntegrationOrderItemMappingRevision::new(row.try_get("revision")?)
            .map_err(internal)?,
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

const MAPPING_SELECT: &str = r#"
    SELECT mapping.*,owner.name AS inventory_owner_name,
           COALESCE(item.description,'Item #' || item.id::TEXT) AS item_description
    FROM integration_order_item_mappings mapping
    JOIN inventory_owners owner
      ON owner.tenant_id=mapping.tenant_id AND owner.id=mapping.inventory_owner_id
    JOIN items item ON item.tenant_id=mapping.tenant_id AND item.id=mapping.item_id
"#;

fn require_owner_scope(scope: &ScopeBindings, owner_id: i64) -> AppResult<()> {
    if scope.includes_inventory_owner(owner_id) {
        Ok(())
    } else {
        Err(AppError::not_found("integration order item mapping"))
    }
}

async fn require_replay_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let owner_id = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT mapping.inventory_owner_id
        FROM command_idempotency_records command
        JOIN integration_order_item_mappings mapping
          ON mapping.tenant_id=command.tenant_id
         AND mapping.id=(command.result_json->>'mapping_id')::BIGINT
        WHERE command.tenant_id=$1 AND command.operation=$2 AND command.idempotency_key=$3
        "#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(owner_id) = owner_id {
        require_owner_scope(scope, owner_id)?;
    }
    Ok(())
}

async fn lock_natural_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &IntegrationOrderItemMappingDefinition,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "integration-order-item-mapping:{}:{}:{}:{}:{}",
            definition.tenant_id,
            definition.inventory_owner_id,
            definition.source_key,
            definition.external_item_key,
            definition.external_uom
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn require_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &IntegrationOrderItemMappingDefinition,
) -> AppResult<()> {
    let target = sqlx::query_scalar::<_, String>(
        r#"
        SELECT item.packaging_unit
        FROM inventory_owner_items owner_item
        JOIN items item ON item.tenant_id=owner_item.tenant_id AND item.id=owner_item.item_id
        WHERE owner_item.tenant_id=$1 AND owner_item.inventory_owner_id=$2
          AND owner_item.item_id=$3 AND owner_item.deleted IS NULL AND item.deleted IS NULL
        FOR SHARE OF owner_item,item
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.inventory_owner_id.get())
    .bind(definition.item_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(target) = target else {
        return Err(AppError::not_found("active client item"));
    };
    if target != definition.requested_uom.as_str() {
        return Err(AppError::conflict(
            "mapped UOM must match the active catalog item packaging unit",
        ));
    }
    Ok(())
}

async fn latest_mapping_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &IntegrationOrderItemMappingDefinition,
) -> AppResult<
    Option<(
        IntegrationOrderItemMappingId,
        IntegrationOrderItemMappingRevision,
        bool,
    )>,
> {
    let row = sqlx::query(
        r#"
        SELECT id,revision,effective_to IS NULL AS active
        FROM integration_order_item_mappings
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND source_key=$3
          AND external_item_key=$4 AND external_uom=$5
        ORDER BY revision DESC LIMIT 1 FOR UPDATE
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.inventory_owner_id.get())
    .bind(definition.source_key.as_str())
    .bind(definition.external_item_key.as_str())
    .bind(definition.external_uom.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok((
            IntegrationOrderItemMappingId::new(row.try_get("id")?).map_err(internal)?,
            IntegrationOrderItemMappingRevision::new(row.try_get("revision")?).map_err(internal)?,
            row.try_get("active")?,
        ))
    })
    .transpose()
}

async fn retire_row_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    mapping_id: IntegrationOrderItemMappingId,
    actor_id: UserId,
    retired_at: Timestamp,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE integration_order_item_mappings SET effective_to=$3,retired_by_user_id=$4 WHERE tenant_id=$1 AND id=$2 AND effective_to IS NULL",
    )
    .bind(tenant_id.get())
    .bind(mapping_id.get())
    .bind(retired_at)
    .bind(actor_id.get())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_mapping_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &ConfigureIntegrationOrderItemMappingCommand,
    revision: IntegrationOrderItemMappingRevision,
    predecessor: Option<IntegrationOrderItemMappingId>,
    actor_id: UserId,
    configured_at: Timestamp,
) -> AppResult<IntegrationOrderItemMappingId> {
    let definition = &command.definition;
    let id = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO integration_order_item_mappings
            (tenant_id,inventory_owner_id,source_key,external_item_key,external_uom,
             item_id,requested_uom,revision,supersedes_mapping_id,effective_from,
             configured_by_user_id,configured_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$10)
        RETURNING id
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.inventory_owner_id.get())
    .bind(definition.source_key.as_str())
    .bind(definition.external_item_key.as_str())
    .bind(definition.external_uom.as_str())
    .bind(definition.item_id.get())
    .bind(definition.requested_uom.as_str())
    .bind(revision.get())
    .bind(predecessor.map(IntegrationOrderItemMappingId::get))
    .bind(configured_at)
    .bind(actor_id.get())
    .fetch_one(&mut **tx)
    .await?;
    IntegrationOrderItemMappingId::new(id).map_err(internal)
}

async fn read_mapping_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    mapping_id: IntegrationOrderItemMappingId,
) -> AppResult<IntegrationOrderItemMappingReadModel> {
    let query = format!("{MAPPING_SELECT} WHERE mapping.tenant_id=$1 AND mapping.id=$2");
    sqlx::query(&query)
        .bind(tenant_id.get())
        .bind(mapping_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .as_ref()
        .map(map_row)
        .transpose()?
        .ok_or_else(|| AppError::not_found("integration order item mapping"))
}

async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    result: &IntegrationOrderItemMappingReadModel,
    actor_id: UserId,
    transition: &str,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let mapping_id = result.mapping_id.get();
    let definition = &result.definition;
    let ordering_key = format!(
        "integration-order-item-mapping:{}:{}:{}:{}:{}:{}:{}",
        definition.inventory_owner_id,
        definition.source_key.as_str().len(),
        definition.source_key,
        definition.external_item_key.as_str().len(),
        definition.external_item_key,
        definition.external_uom.as_str().len(),
        definition.external_uom,
    );
    let sequence = next_outbox_sequence_tx(tx, result.definition.tenant_id, &ordering_key).await?;
    let event_type = format!("integration.order_item_mapping.{transition}");
    let event_key = format!("{ordering_key}:{}:{transition}", result.revision.get());
    let aggregate_id = mapping_id.to_string();
    let payload = serde_json::to_value(result).map_err(internal)?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: result.definition.tenant_id,
            inventory_owner_id: Some(result.definition.inventory_owner_id),
            facility_id: None,
            actor_user_id: Some(actor_id.get()),
            event_key: &event_key,
            aggregate_type: "integration_order_item_mapping",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: &event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

pub async fn configure(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureIntegrationOrderItemMappingCommand,
) -> AppResult<ConfigureIntegrationOrderItemMappingResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.definition.tenant_id != access.tenant_id {
        return Err(AppError::not_found("integration order item mapping"));
    }
    let prepared = PreparedCommand::new_v1(
        context,
        CONFIGURE_INTEGRATION_ORDER_ITEM_MAPPING_OPERATION,
        command,
    )?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    require_replay_visible_tx(&mut tx, &prepared, &scope).await?;
    require_owner_scope(&scope, command.definition.inventory_owner_id.get())?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    lock_natural_key_tx(&mut tx, &command.definition).await?;
    let predecessor = latest_mapping_tx(&mut tx, &command.definition).await?;
    match (predecessor, command.expected_revision) {
        (None, None) | (Some((_, _, false)), None) => {}
        (Some((_, revision, true)), Some(expected)) if revision == expected => {}
        (Some((_, _, true)), None) => {
            return Err(AppError::conflict(
                "integration order item mapping already exists",
            ));
        }
        (None, Some(_)) | (Some((_, _, false)), Some(_)) => {
            return Err(AppError::conflict(
                "integration order item mapping has no active revision",
            ));
        }
        (Some((_, _, true)), Some(_)) => {
            return Err(AppError::conflict(
                "integration order item mapping revision is stale",
            ));
        }
    }
    require_target_tx(&mut tx, &command.definition).await?;
    let configured_at = now_iso();
    let retired_predecessor = if let Some((id, _, true)) = predecessor {
        retire_row_tx(
            &mut tx,
            access.tenant_id,
            id,
            context.actor_id,
            configured_at,
        )
        .await?;
        Some(read_mapping_tx(&mut tx, access.tenant_id, id).await?)
    } else {
        None
    };
    let revision = match predecessor {
        Some((_, revision, _)) => revision
            .checked_next()
            .ok_or_else(|| AppError::internal("integration mapping revision overflow"))?,
        None => IntegrationOrderItemMappingRevision::new(1).map_err(internal)?,
    };
    let mapping_id = insert_mapping_tx(
        &mut tx,
        command,
        revision,
        predecessor.map(|(id, _, _)| id),
        context.actor_id,
        configured_at,
    )
    .await?;
    let result = read_mapping_tx(&mut tx, access.tenant_id, mapping_id).await?;
    if let Some(retired_predecessor) = retired_predecessor {
        enqueue_event_tx(
            &mut tx,
            &retired_predecessor,
            context.actor_id,
            "retired",
            configured_at,
        )
        .await?;
    }
    enqueue_event_tx(
        &mut tx,
        &result,
        context.actor_id,
        "configured",
        configured_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn retire(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RetireIntegrationOrderItemMappingCommand,
) -> AppResult<RetireIntegrationOrderItemMappingResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(
        context,
        RETIRE_INTEGRATION_ORDER_ITEM_MAPPING_OPERATION,
        command,
    )?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    require_replay_visible_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let hint = sqlx::query(
        r#"
        SELECT inventory_owner_id,source_key,external_item_key,external_uom,item_id,requested_uom
        FROM integration_order_item_mappings WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.mapping_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("integration order item mapping"))?;
    let definition = IntegrationOrderItemMappingDefinition {
        tenant_id: access.tenant_id,
        inventory_owner_id: InventoryOwnerId::new(hint.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        source_key: IntegrationSourceKey::new(hint.try_get::<String, _>("source_key")?)
            .map_err(internal)?,
        external_item_key: ExternalItemKey::new(hint.try_get::<String, _>("external_item_key")?)
            .map_err(internal)?,
        external_uom: ExternalItemUom::new(hint.try_get::<String, _>("external_uom")?)
            .map_err(internal)?,
        item_id: CatalogItemId::new(hint.try_get("item_id")?).map_err(internal)?,
        requested_uom: IntegrationMappedUom::new(hint.try_get::<String, _>("requested_uom")?)
            .map_err(internal)?,
    };
    require_owner_scope(&scope, definition.inventory_owner_id.get())?;
    lock_natural_key_tx(&mut tx, &definition).await?;
    let row = sqlx::query(
        "SELECT revision,effective_to FROM integration_order_item_mappings WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(access.tenant_id.get())
    .bind(command.mapping_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("integration order item mapping"))?;
    if row
        .try_get::<Option<Timestamp>, _>("effective_to")?
        .is_some()
    {
        return Err(AppError::conflict(
            "integration order item mapping is already retired",
        ));
    }
    let revision =
        IntegrationOrderItemMappingRevision::new(row.try_get("revision")?).map_err(internal)?;
    if revision != command.expected_revision {
        return Err(AppError::conflict(
            "integration order item mapping revision is stale",
        ));
    }
    let retired_at = now_iso();
    retire_row_tx(
        &mut tx,
        access.tenant_id,
        command.mapping_id,
        context.actor_id,
        retired_at,
    )
    .await?;
    let result = read_mapping_tx(&mut tx, access.tenant_id, command.mapping_id).await?;
    enqueue_event_tx(&mut tx, &result, context.actor_id, "retired", retired_at).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn page(
    db: &Db,
    access: &TenantAccess,
    query: IntegrationOrderItemMappingPageQuery,
) -> AppResult<IntegrationOrderItemMappingPage> {
    if query.limit == 0 || query.limit > 1_000 {
        return Err(AppError::bad_request(
            "integration mapping page limit must be between 1 and 1000",
        ));
    }
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;
    if let Some(owner_id) = query.inventory_owner_id {
        require_owner_scope(&scope, owner_id.get())?;
    }
    let sql = format!(
        r#"
        {MAPPING_SELECT}
        WHERE mapping.tenant_id=$1 AND ($2 OR mapping.inventory_owner_id=ANY($3))
          AND ($4::BIGINT IS NULL OR mapping.inventory_owner_id=$4)
          AND ($5::TEXT IS NULL OR mapping.source_key=$5)
          AND ($6::BIGINT IS NULL OR mapping.item_id=$6)
          AND (($7::TEXT IS NULL AND mapping.effective_to IS NULL)
               OR $7='active' AND mapping.effective_to IS NULL
               OR $7='retired' AND mapping.effective_to IS NOT NULL)
          AND mapping.id>$8
        ORDER BY mapping.id ASC LIMIT $9
        "#
    );
    let limit = i64::from(query.limit) + 1;
    let rows = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(query.source_key.as_deref())
        .bind(query.item_id.map(CatalogItemId::get))
        .bind(query.status.map(|status| match status {
            IntegrationOrderItemMappingStatus::Active => "active",
            IntegrationOrderItemMappingStatus::Retired => "retired",
        }))
        .bind(
            query
                .cursor
                .map_or(0, |cursor| cursor.after_mapping_id.get()),
        )
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(map_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = has_more
        .then(|| items.last().map(|item| item.mapping_id))
        .flatten()
        .map(|after_mapping_id| IntegrationOrderItemMappingCursor { after_mapping_id });
    tx.commit().await?;
    Ok(IntegrationOrderItemMappingPage { items, next_cursor })
}
