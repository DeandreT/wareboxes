use sqlx::Row;
use wareboxes_application::packing_decision_policy::{
    PackDecisionPolicyReadModel, PackDecisionPolicySource,
};
use wareboxes_domain::{
    ConfigurationScope, ConfigurationVersionId, DecisionRuleDefinition, FacilityId,
    InventoryOwnerId, TenantId, Timestamp,
};

use crate::error::{AppError, AppResult};

pub(super) async fn resolve_decision_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    effective_at: Timestamp,
) -> AppResult<PackDecisionPolicyReadModel> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("configuration-kind:{}:pack", tenant_id.get()))
        .execute(&mut **tx)
        .await?;
    let row = sqlx::query(
        r#"SELECT id,revision,scope_level,inventory_owner_id,facility_id,definition
        FROM configuration_versions
        WHERE tenant_id=$1 AND kind='pack' AND status='active'
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
    .bind(inventory_owner_id.get())
    .bind(facility_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(PackDecisionPolicyReadModel::product_default());
    };
    configured_policy(
        row.try_get("id")?,
        row.try_get("revision")?,
        &row.try_get::<String, _>("scope_level")?,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
        row.try_get("definition")?,
    )
}

pub(super) fn decision_policy_from_session_row(
    row: &sqlx::postgres::PgRow,
) -> AppResult<PackDecisionPolicyReadModel> {
    let source: String = row.try_get("pack_policy_source")?;
    let policy = match source.as_str() {
        "product_default" => PackDecisionPolicyReadModel::product_default(),
        "configuration" => configured_policy(
            required(row, "pack_configuration_id")?,
            required(row, "pack_configuration_revision")?,
            &required::<String>(row, "pack_scope_level")?,
            row.try_get("pack_inventory_owner_id")?,
            row.try_get("pack_facility_id")?,
            serde_json::json!({
                "kind": "pack",
                "require_station_scan": row.try_get::<bool, _>("require_station_scan")?,
                "require_weight": row.try_get::<bool, _>("require_weight")?,
                "allow_mixed_orders": row.try_get::<bool, _>("allow_mixed_orders")?,
            }),
        )?,
        _ => {
            return Err(AppError::internal(
                "packing session policy source is invalid",
            ))
        }
    };
    if row.try_get::<bool, _>("require_station_scan")? != policy.require_station_scan
        || row.try_get::<bool, _>("require_weight")? != policy.require_weight
        || row.try_get::<bool, _>("allow_mixed_orders")? != policy.allow_mixed_orders
        || row.try_get::<String, _>("pack_policy_hash")? != policy.policy_hash
    {
        return Err(AppError::internal(
            "packing session policy evidence is inconsistent",
        ));
    }
    Ok(policy)
}

fn required<T>(row: &sqlx::postgres::PgRow, column: &str) -> AppResult<T>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get::<Option<T>, _>(column)?
        .ok_or_else(|| AppError::internal(format!("packing session {column} is missing")))
}

fn configured_policy(
    configuration_id: i64,
    configuration_revision: i64,
    scope_level: &str,
    inventory_owner_id: Option<i64>,
    facility_id: Option<i64>,
    definition: serde_json::Value,
) -> AppResult<PackDecisionPolicyReadModel> {
    let scope = configuration_scope(scope_level, inventory_owner_id, facility_id)?;
    let definition = serde_json::from_value::<DecisionRuleDefinition>(definition)
        .map_err(|error| AppError::internal(error.to_string()))?;
    PackDecisionPolicyReadModel::from_configuration(
        ConfigurationVersionId::new(configuration_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        configuration_revision,
        scope,
        &definition,
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
            inventory_owner_id: owner(inventory_owner_id)?,
        }),
        "facility" if inventory_owner_id.is_none() => Ok(ConfigurationScope::Facility {
            facility_id: facility(facility_id)?,
        }),
        "owner_facility" => Ok(ConfigurationScope::OwnerFacility {
            inventory_owner_id: owner(inventory_owner_id)?,
            facility_id: facility(facility_id)?,
        }),
        _ => Err(AppError::internal("pack configuration scope is invalid")),
    }
}

fn owner(value: Option<i64>) -> AppResult<InventoryOwnerId> {
    InventoryOwnerId::new(value.ok_or_else(|| AppError::internal("pack owner is missing"))?)
        .map_err(|error| AppError::internal(error.to_string()))
}

fn facility(value: Option<i64>) -> AppResult<FacilityId> {
    FacilityId::new(value.ok_or_else(|| AppError::internal("pack facility is missing"))?)
        .map_err(|error| AppError::internal(error.to_string()))
}

pub(super) struct PolicyBindings<'a> {
    pub source: &'static str,
    pub configuration_id: Option<i64>,
    pub configuration_revision: Option<i64>,
    pub scope_level: Option<&'static str>,
    pub inventory_owner_id: Option<i64>,
    pub facility_id: Option<i64>,
    pub require_station_scan: bool,
    pub require_weight: bool,
    pub allow_mixed_orders: bool,
    pub policy_hash: &'a str,
}

pub(super) fn policy_bindings(policy: &PackDecisionPolicyReadModel) -> PolicyBindings<'_> {
    let (scope_level, inventory_owner_id, facility_id) = match policy.configuration_scope {
        None => (None, None, None),
        Some(ConfigurationScope::Tenant) => (Some("tenant"), None, None),
        Some(ConfigurationScope::InventoryOwner { inventory_owner_id }) => (
            Some("inventory_owner"),
            Some(inventory_owner_id.get()),
            None,
        ),
        Some(ConfigurationScope::Facility { facility_id }) => {
            (Some("facility"), None, Some(facility_id.get()))
        }
        Some(ConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        }) => (
            Some("owner_facility"),
            Some(inventory_owner_id.get()),
            Some(facility_id.get()),
        ),
    };
    PolicyBindings {
        source: match policy.source {
            PackDecisionPolicySource::ProductDefault => "product_default",
            PackDecisionPolicySource::Configuration => "configuration",
        },
        configuration_id: policy.configuration_id.map(ConfigurationVersionId::get),
        configuration_revision: policy.configuration_revision,
        scope_level,
        inventory_owner_id,
        facility_id,
        require_station_scan: policy.require_station_scan,
        require_weight: policy.require_weight,
        allow_mixed_orders: policy.allow_mixed_orders,
        policy_hash: &policy.policy_hash,
    }
}
