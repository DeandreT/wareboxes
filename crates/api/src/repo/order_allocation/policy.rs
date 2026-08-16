use sqlx::Row;
use wareboxes_application::order_allocation::{
    allocation_policy_hash, AllocationPolicyReadModel, AllocationPolicySource,
};
use wareboxes_domain::{
    AllocationStrategy, ConfigurationScope, ConfigurationVersionId, DecisionRuleDefinition,
    FacilityId, InventoryOwnerId, InventoryRotation, TenantId, Timestamp,
};

use crate::error::{AppError, AppResult};

pub(crate) async fn resolve_allocation_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    effective_at: Timestamp,
    serialize_mutations: bool,
) -> AppResult<AllocationPolicyReadModel> {
    if serialize_mutations {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("configuration-kind:{}:allocation", tenant_id.get()))
            .execute(&mut **tx)
            .await?;
    }

    let row = sqlx::query(
        r#"
        SELECT id,revision,scope_level,inventory_owner_id,facility_id,definition
        FROM configuration_versions
        WHERE tenant_id=$1 AND kind='allocation' AND status='active'
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
        return Ok(AllocationPolicyReadModel::product_default());
    };
    let definition = serde_json::from_value::<DecisionRuleDefinition>(row.try_get("definition")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let DecisionRuleDefinition::Allocation {
        rotation,
        allow_partial,
        require_complete_line,
    } = definition
    else {
        return Err(AppError::internal(
            "resolved allocation configuration has another rule kind",
        ));
    };
    let strategy = match rotation {
        InventoryRotation::Fifo => AllocationStrategy::Fifo,
        InventoryRotation::Fefo => AllocationStrategy::Fefo,
    };
    let configuration_scope = match row.try_get::<String, _>("scope_level")?.as_str() {
        "tenant" => ConfigurationScope::Tenant,
        "inventory_owner" => ConfigurationScope::InventoryOwner {
            inventory_owner_id: InventoryOwnerId::new(
                row.try_get::<Option<i64>, _>("inventory_owner_id")?
                    .ok_or_else(|| AppError::internal("allocation policy owner is missing"))?,
            )
            .map_err(|error| AppError::internal(error.to_string()))?,
        },
        "facility" => ConfigurationScope::Facility {
            facility_id: FacilityId::new(
                row.try_get::<Option<i64>, _>("facility_id")?
                    .ok_or_else(|| AppError::internal("allocation policy facility is missing"))?,
            )
            .map_err(|error| AppError::internal(error.to_string()))?,
        },
        "owner_facility" => ConfigurationScope::OwnerFacility {
            inventory_owner_id: InventoryOwnerId::new(
                row.try_get::<Option<i64>, _>("inventory_owner_id")?
                    .ok_or_else(|| AppError::internal("allocation policy owner is missing"))?,
            )
            .map_err(|error| AppError::internal(error.to_string()))?,
            facility_id: FacilityId::new(
                row.try_get::<Option<i64>, _>("facility_id")?
                    .ok_or_else(|| AppError::internal("allocation policy facility is missing"))?,
            )
            .map_err(|error| AppError::internal(error.to_string()))?,
        },
        _ => return Err(AppError::internal("allocation policy scope is invalid")),
    };
    let configuration_id = ConfigurationVersionId::new(row.try_get("id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let configuration_revision: i64 = row.try_get("revision")?;
    if configuration_revision <= 0 {
        return Err(AppError::internal("allocation policy revision is invalid"));
    }
    Ok(AllocationPolicyReadModel {
        source: AllocationPolicySource::Configuration,
        configuration_id: Some(configuration_id),
        configuration_revision: Some(configuration_revision),
        configuration_scope: Some(configuration_scope),
        strategy,
        allow_partial,
        require_complete_line,
        policy_hash: allocation_policy_hash(strategy, allow_partial, require_complete_line),
    })
}
