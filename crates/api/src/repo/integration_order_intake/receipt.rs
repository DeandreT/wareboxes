use sqlx::Row;
use wareboxes_application::integration::{
    IntegrationInboxOwnerMappingEvidence, NewIntegrationInboxReceipt, ReceiveIntegrationInboxResult,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    ExternalInventoryOwnerKey, IntegrationOrderOwnerMappingId,
    IntegrationOrderOwnerMappingRevision, IntegrationSourceKey, InventoryOwnerId, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};
use wareboxes_persistence_postgres::integration_inbox;

use super::super::access::{lock_current_scope_tx, require_permission_tx};
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExternalOrderReceipt<'a> {
    pub(crate) source_key: &'a IntegrationSourceKey,
    pub(crate) external_inventory_owner_key: &'a ExternalInventoryOwnerKey,
    pub(crate) deduplication_key: &'a str,
    pub(crate) content_type: &'a str,
    pub(crate) raw_payload: &'a [u8],
    pub(crate) request_id: &'a str,
}

pub(crate) async fn receive_external_order(
    db: &Db,
    access: &TenantAccess,
    actor_id: UserId,
    input: ExternalOrderReceipt<'_>,
) -> AppResult<ReceiveIntegrationInboxResult> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, actor_id.get(), "orders").await?;

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "integration-inbox:{}:{}:{}",
            access.tenant_id,
            input.source_key.as_str(),
            input.deduplication_key
        ))
        .execute(&mut *tx)
        .await?;

    let existing = sqlx::query(
        r#"
        SELECT inventory_owner_id,external_inventory_owner_key,
               owner_mapping_id,owner_mapping_revision
        FROM integration_inbox_keys
        WHERE tenant_id=$1 AND source_key=$2 AND deduplication_key=$3
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(input.source_key.as_str())
    .bind(input.deduplication_key)
    .fetch_optional(&mut *tx)
    .await?;

    let (inventory_owner_id, owner_mapping) = if let Some(row) = existing {
        let owner_id = row
            .try_get::<Option<i64>, _>("inventory_owner_id")?
            .ok_or_else(|| AppError::conflict("integration inbox key has no inventory owner"))?;
        if !scope.includes_inventory_owner(owner_id) {
            return Err(AppError::not_found("integration inbox key"));
        }
        let external_key = row
            .try_get::<Option<String>, _>("external_inventory_owner_key")?
            .ok_or_else(|| {
                AppError::conflict("integration inbox key has no owner mapping evidence")
            })?;
        if external_key != input.external_inventory_owner_key.as_str() {
            return Err(AppError::conflict(
                "integration deduplication key was reused with a different owner mapping",
            ));
        }
        let mapping_id = row
            .try_get::<Option<i64>, _>("owner_mapping_id")?
            .ok_or_else(|| {
                AppError::conflict("integration inbox key has incomplete owner mapping evidence")
            })?;
        let mapping_revision = row
            .try_get::<Option<i64>, _>("owner_mapping_revision")?
            .ok_or_else(|| {
                AppError::conflict("integration inbox key has incomplete owner mapping evidence")
            })?;
        (
            InventoryOwnerId::new(owner_id)
                .map_err(|error| AppError::internal(error.to_string()))?,
            IntegrationInboxOwnerMappingEvidence {
                external_inventory_owner_key: input.external_inventory_owner_key.clone(),
                mapping_id: IntegrationOrderOwnerMappingId::new(mapping_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                mapping_revision: IntegrationOrderOwnerMappingRevision::new(mapping_revision)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            },
        )
    } else {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(super::super::integration_mapping::owner_natural_lock_key(
                access.tenant_id,
                input.source_key.as_str(),
                input.external_inventory_owner_key.as_str(),
            ))
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query(
            r#"
            SELECT mapping.id,mapping.revision,mapping.inventory_owner_id
            FROM integration_order_owner_mappings mapping
            JOIN inventory_owners owner
              ON owner.tenant_id=mapping.tenant_id
             AND owner.id=mapping.inventory_owner_id
             AND owner.deleted IS NULL
            WHERE mapping.tenant_id=$1
              AND mapping.source_key=$2
              AND mapping.external_inventory_owner_key=$3
              AND mapping.effective_to IS NULL
              AND ($4 OR mapping.inventory_owner_id=ANY($5))
            FOR SHARE OF mapping,owner
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(input.source_key.as_str())
        .bind(input.external_inventory_owner_key.as_str())
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::not_found("active integration inventory-owner mapping"))?;
        (
            InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            IntegrationInboxOwnerMappingEvidence {
                external_inventory_owner_key: input.external_inventory_owner_key.clone(),
                mapping_id: IntegrationOrderOwnerMappingId::new(row.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                mapping_revision: IntegrationOrderOwnerMappingRevision::new(
                    row.try_get("revision")?,
                )
                .map_err(|error| AppError::internal(error.to_string()))?,
            },
        )
    };

    let receipt = NewIntegrationInboxReceipt {
        tenant_id: access.tenant_id,
        inventory_owner_id: Some(inventory_owner_id),
        facility_id: None,
        owner_mapping: Some(&owner_mapping),
        source_key: input.source_key.as_str(),
        deduplication_key: input.deduplication_key,
        content_type: input.content_type,
        raw_payload: input.raw_payload,
        request_id: Some(input.request_id),
    };
    let received = integration_inbox::receive_tx(&mut tx, &receipt)
        .await
        .map_err(AppError::from)?;
    tx.commit().await?;
    Ok(received)
}
