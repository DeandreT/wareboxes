use sqlx::Row;
use wareboxes_application::receipt_policy::{
    receipt_policy_hash, ReceiptPolicyReadModel, ReceiptPolicySource,
};
use wareboxes_domain::{
    ConfigurationScope, ConfigurationVersionId, DecisionRuleDefinition, FacilityId,
    InventoryOwnerId, TenantId, Timestamp,
};

use crate::error::{AppError, AppResult};

pub(crate) async fn resolve_receipt_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    effective_at: Timestamp,
    serialize_mutations: bool,
) -> AppResult<ReceiptPolicyReadModel> {
    if serialize_mutations {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("configuration-kind:{}:receipt", tenant_id.get()))
            .execute(&mut **tx)
            .await?;
    }

    let row = sqlx::query(
        r#"
        SELECT id,revision,scope_level,inventory_owner_id,facility_id,definition
        FROM configuration_versions
        WHERE tenant_id=$1 AND kind='receipt' AND status='active'
          AND effective_from<=$2 AND (effective_until IS NULL OR effective_until>$2)
          AND (inventory_owner_id IS NULL OR inventory_owner_id=$3)
          AND (facility_id IS NULL OR facility_id=$4)
        ORDER BY CASE scope_level
                   WHEN 'owner_facility' THEN 2
                   WHEN 'inventory_owner' THEN 1
                   WHEN 'facility' THEN 1
                   ELSE 0
                 END DESC,
                 effective_from DESC,revision DESC,id DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id.get())
    .bind(effective_at)
    .bind(inventory_owner_id.get())
    .bind(facility_id.get())
    .fetch_optional(&mut **tx)
    .await?;

    let Some(row) = row else {
        return Ok(ReceiptPolicyReadModel::product_default());
    };
    let definition = serde_json::from_value::<DecisionRuleDefinition>(row.try_get("definition")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let DecisionRuleDefinition::Receipt {
        allow_unexpected,
        quarantine_unmapped_items,
        over_receipt_tolerance_basis_points,
    } = definition
    else {
        return Err(AppError::internal(
            "resolved receipt configuration has another rule kind",
        ));
    };
    let configuration_scope = configuration_scope(&row)?;
    let configuration_id = ConfigurationVersionId::new(row.try_get("id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let configuration_revision: i64 = row.try_get("revision")?;
    if configuration_revision <= 0 {
        return Err(AppError::internal("receipt policy revision is invalid"));
    }
    Ok(ReceiptPolicyReadModel {
        source: ReceiptPolicySource::Configuration,
        configuration_id: Some(configuration_id),
        configuration_revision: Some(configuration_revision),
        configuration_scope: Some(configuration_scope),
        allow_unexpected,
        quarantine_unmapped_items,
        over_receipt_tolerance_basis_points,
        policy_hash: receipt_policy_hash(
            allow_unexpected,
            quarantine_unmapped_items,
            over_receipt_tolerance_basis_points,
        ),
    })
}

fn configuration_scope(row: &sqlx::postgres::PgRow) -> AppResult<ConfigurationScope> {
    match row.try_get::<String, _>("scope_level")?.as_str() {
        "tenant" => Ok(ConfigurationScope::Tenant),
        "inventory_owner" => Ok(ConfigurationScope::InventoryOwner {
            inventory_owner_id: InventoryOwnerId::new(required(row, "inventory_owner_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
        }),
        "facility" => Ok(ConfigurationScope::Facility {
            facility_id: FacilityId::new(required(row, "facility_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
        }),
        "owner_facility" => Ok(ConfigurationScope::OwnerFacility {
            inventory_owner_id: InventoryOwnerId::new(required(row, "inventory_owner_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            facility_id: FacilityId::new(required(row, "facility_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
        }),
        _ => Err(AppError::internal("receipt policy scope is invalid")),
    }
}

fn required(row: &sqlx::postgres::PgRow, name: &str) -> AppResult<i64> {
    row.try_get::<Option<i64>, _>(name)?
        .ok_or_else(|| AppError::internal(format!("receipt policy {name} is missing")))
}
