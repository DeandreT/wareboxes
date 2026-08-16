use sqlx::Row;
use wareboxes_application::replenishment_decision_policy::ReplenishmentDecisionPolicyReadModel;
use wareboxes_domain::{
    ConfigurationScope, ConfigurationVersionId, DecisionRuleDefinition, FacilityId,
    InventoryOwnerId, ReplenishmentPolicyThresholds, TenantId, Timestamp,
};

use super::PolicyRow;
use crate::error::{AppError, AppResult};

pub(super) async fn resolve_decision_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    policy: &PolicyRow,
    effective_at: Timestamp,
    serialize_configuration: bool,
) -> AppResult<ReplenishmentDecisionPolicyReadModel> {
    if serialize_configuration {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!(
                "configuration-kind:{}:replenishment",
                tenant_id.get()
            ))
            .execute(&mut **tx)
            .await?;
    }
    let scope = policy.scope();
    let row = sqlx::query(
        r#"SELECT id,revision,scope_level,inventory_owner_id,facility_id,definition
        FROM configuration_versions
        WHERE tenant_id=$1 AND kind='replenishment' AND status='active'
          AND activated_at<=$2 AND effective_from<=$2
          AND (effective_until IS NULL OR effective_until>$2)
          AND (inventory_owner_id IS NULL OR inventory_owner_id=$3)
          AND (facility_id IS NULL OR facility_id=$4)
        ORDER BY CASE scope_level
          WHEN 'owner_facility' THEN 2
          WHEN 'inventory_owner' THEN 1
          WHEN 'facility' THEN 1
          ELSE 0 END DESC,
          effective_from DESC,revision DESC,id DESC
        LIMIT 1"#,
    )
    .bind(tenant_id.get())
    .bind(effective_at)
    .bind(scope.inventory_owner_id.get())
    .bind(scope.facility_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(ReplenishmentDecisionPolicyReadModel::product_default(
            policy.definition.thresholds(),
        ));
    };
    configured_policy(
        row.try_get("id")?,
        row.try_get("revision")?,
        &row.try_get::<String, _>("scope_level")?,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
        row.try_get("definition")?,
        policy.definition.thresholds(),
    )
}

pub(super) fn decision_policy_from_readiness_row(
    row: &sqlx::postgres::PgRow,
    operational: ReplenishmentPolicyThresholds,
) -> AppResult<ReplenishmentDecisionPolicyReadModel> {
    let Some(configuration_id) = row.try_get::<Option<i64>, _>("decision_configuration_id")? else {
        return Ok(ReplenishmentDecisionPolicyReadModel::product_default(
            operational,
        ));
    };
    configured_policy(
        configuration_id,
        row.try_get::<Option<i64>, _>("decision_configuration_revision")?
            .ok_or_else(|| AppError::internal("replenishment configuration revision is missing"))?,
        &row.try_get::<Option<String>, _>("decision_scope_level")?
            .ok_or_else(|| AppError::internal("replenishment configuration scope is missing"))?,
        row.try_get("decision_inventory_owner_id")?,
        row.try_get("decision_facility_id")?,
        row.try_get::<Option<serde_json::Value>, _>("decision_definition")?
            .ok_or_else(|| {
                AppError::internal("replenishment configuration definition is missing")
            })?,
        operational,
    )
}

fn configured_policy(
    configuration_id: i64,
    configuration_revision: i64,
    scope_level: &str,
    inventory_owner_id: Option<i64>,
    facility_id: Option<i64>,
    definition: serde_json::Value,
    operational: ReplenishmentPolicyThresholds,
) -> AppResult<ReplenishmentDecisionPolicyReadModel> {
    let configuration_scope = configuration_scope(scope_level, inventory_owner_id, facility_id)?;
    let definition = serde_json::from_value::<DecisionRuleDefinition>(definition)
        .map_err(|error| AppError::internal(error.to_string()))?;
    ReplenishmentDecisionPolicyReadModel::from_configuration(
        ConfigurationVersionId::new(configuration_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        configuration_revision,
        configuration_scope,
        &definition,
        operational,
    )
    .map_err(|error| AppError::internal(error.to_string()))
}

fn configuration_scope(
    scope_level: &str,
    inventory_owner_id: Option<i64>,
    facility_id: Option<i64>,
) -> AppResult<ConfigurationScope> {
    match scope_level {
        "tenant" if inventory_owner_id.is_none() && facility_id.is_none() => {
            Ok(ConfigurationScope::Tenant)
        }
        "inventory_owner" if facility_id.is_none() => Ok(ConfigurationScope::InventoryOwner {
            inventory_owner_id: InventoryOwnerId::new(inventory_owner_id.ok_or_else(|| {
                AppError::internal("replenishment configuration owner is missing")
            })?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        }),
        "facility" if inventory_owner_id.is_none() => Ok(ConfigurationScope::Facility {
            facility_id: FacilityId::new(facility_id.ok_or_else(|| {
                AppError::internal("replenishment configuration facility is missing")
            })?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        }),
        "owner_facility" => Ok(ConfigurationScope::OwnerFacility {
            inventory_owner_id: InventoryOwnerId::new(inventory_owner_id.ok_or_else(|| {
                AppError::internal("replenishment configuration owner is missing")
            })?)
            .map_err(|error| AppError::internal(error.to_string()))?,
            facility_id: FacilityId::new(facility_id.ok_or_else(|| {
                AppError::internal("replenishment configuration facility is missing")
            })?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        }),
        _ => Err(AppError::internal(
            "replenishment configuration scope is invalid",
        )),
    }
}
