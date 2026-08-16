use std::collections::BTreeMap;

use sqlx::Row;
use wareboxes_application::putaway_policy::{
    putaway_policy_hash, PutawayPolicyExpectation, PutawayPolicyReadModel, PutawayPolicySource,
};
use wareboxes_domain::{
    ConfigurationScope, ConfigurationVersionId, DecisionRuleDefinition, FacilityId,
    InventoryOwnerId, TenantId, Timestamp,
};

use crate::error::{AppError, AppResult};

pub(crate) async fn load_task_policy(
    db: &crate::db::Db,
    access: &wareboxes_core::models::TenantAccess,
    task_id: i64,
) -> AppResult<PutawayPolicyReadModel> {
    let mut tx = db.begin().await?;
    crate::db::bind_tenant_context(&mut tx, access.tenant_id).await?;
    let row = sqlx::query(
        r#"
        SELECT putaway_policy_source,putaway_policy_configuration_id,
               putaway_policy_configuration_revision,putaway_policy_scope_level,
               putaway_policy_scope_owner_id,putaway_policy_scope_facility_id,
               putaway_policy_definition,putaway_policy_hash
        FROM putaway_tasks WHERE tenant_id=$1 AND task_id=$2
        UNION ALL
        SELECT putaway_policy_source,putaway_policy_configuration_id,
               putaway_policy_configuration_revision,putaway_policy_scope_level,
               putaway_policy_scope_owner_id,putaway_policy_scope_facility_id,
               putaway_policy_definition,putaway_policy_hash
        FROM license_plate_putaway_tasks WHERE tenant_id=$1 AND task_id=$2
        LIMIT 1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("putaway task"))?;
    let policy = frozen_policy(&row)?;
    tx.commit().await?;
    Ok(policy)
}

#[derive(Debug, Clone)]
pub(crate) struct PutawayContent {
    pub item_id: i64,
    pub item_batch_id: i64,
    pub uom: String,
    pub quantity: i64,
}

pub(crate) async fn resolve_putaway_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    effective_at: Timestamp,
    serialize_mutations: bool,
) -> AppResult<PutawayPolicyReadModel> {
    if serialize_mutations {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("configuration-kind:{}:putaway", tenant_id.get()))
            .execute(&mut **tx)
            .await?;
    }

    let row = sqlx::query(
        r#"
        SELECT id,revision,scope_level,inventory_owner_id,facility_id,definition
        FROM configuration_versions
        WHERE tenant_id=$1 AND kind='putaway' AND status='active'
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
        return Ok(PutawayPolicyReadModel::product_default());
    };
    let definition = serde_json::from_value::<DecisionRuleDefinition>(row.try_get("definition")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let DecisionRuleDefinition::Putaway {
        require_zone_compatibility,
        enforce_location_capacity,
        allow_mixed_lots,
    } = definition
    else {
        return Err(AppError::internal(
            "resolved putaway configuration has another rule kind",
        ));
    };
    let configuration_scope = configuration_scope(&row)?;
    let configuration_id = ConfigurationVersionId::new(row.try_get("id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let configuration_revision: i64 = row.try_get("revision")?;
    if configuration_revision <= 0 {
        return Err(AppError::internal("putaway policy revision is invalid"));
    }
    Ok(PutawayPolicyReadModel {
        source: PutawayPolicySource::Configuration,
        configuration_id: Some(configuration_id),
        configuration_revision: Some(configuration_revision),
        configuration_scope: Some(configuration_scope),
        require_zone_compatibility,
        enforce_location_capacity,
        allow_mixed_lots,
        policy_hash: putaway_policy_hash(
            require_zone_compatibility,
            enforce_location_capacity,
            allow_mixed_lots,
        ),
    })
}

pub(crate) fn require_expected_policy(
    actual: &PutawayPolicyReadModel,
    expected: &PutawayPolicyExpectation,
) -> AppResult<()> {
    if actual.matches_expectation(expected) {
        Ok(())
    } else {
        Err(AppError::conflict(
            "putaway policy changed; refresh planning evidence and retry",
        ))
    }
}

pub(crate) fn source_text(source: PutawayPolicySource) -> &'static str {
    match source {
        PutawayPolicySource::ProductDefault => "product_default",
        PutawayPolicySource::Configuration => "configuration",
    }
}

pub(crate) fn definition_json(policy: &PutawayPolicyReadModel) -> serde_json::Value {
    serde_json::json!({
        "require_zone_compatibility": policy.require_zone_compatibility,
        "enforce_location_capacity": policy.enforce_location_capacity,
        "allow_mixed_lots": policy.allow_mixed_lots,
    })
}

pub(crate) fn scope_values(
    scope: Option<ConfigurationScope>,
) -> (Option<&'static str>, Option<i64>, Option<i64>) {
    match scope {
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
    }
}

pub(crate) fn frozen_policy(row: &sqlx::postgres::PgRow) -> AppResult<PutawayPolicyReadModel> {
    let source = match row.try_get::<String, _>("putaway_policy_source")?.as_str() {
        "product_default" => PutawayPolicySource::ProductDefault,
        "configuration" => PutawayPolicySource::Configuration,
        _ => return Err(AppError::internal("putaway policy source is invalid")),
    };
    let configuration_scope = match row
        .try_get::<Option<String>, _>("putaway_policy_scope_level")?
    {
        None => None,
        Some(level) => Some(match level.as_str() {
            "tenant" => ConfigurationScope::Tenant,
            "inventory_owner" => ConfigurationScope::InventoryOwner {
                inventory_owner_id: InventoryOwnerId::new(required(
                    row,
                    "putaway_policy_scope_owner_id",
                )?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            },
            "facility" => ConfigurationScope::Facility {
                facility_id: FacilityId::new(required(row, "putaway_policy_scope_facility_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            },
            "owner_facility" => ConfigurationScope::OwnerFacility {
                inventory_owner_id: InventoryOwnerId::new(required(
                    row,
                    "putaway_policy_scope_owner_id",
                )?)
                .map_err(|error| AppError::internal(error.to_string()))?,
                facility_id: FacilityId::new(required(row, "putaway_policy_scope_facility_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            },
            _ => return Err(AppError::internal("putaway policy scope is invalid")),
        }),
    };
    let definition: serde_json::Value = row.try_get("putaway_policy_definition")?;
    let require_zone_compatibility = definition
        .get("require_zone_compatibility")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| AppError::internal("putaway policy zone rule is invalid"))?;
    let enforce_location_capacity = definition
        .get("enforce_location_capacity")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| AppError::internal("putaway policy capacity rule is invalid"))?;
    let allow_mixed_lots = definition
        .get("allow_mixed_lots")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| AppError::internal("putaway policy lot rule is invalid"))?;
    let policy = PutawayPolicyReadModel {
        source,
        configuration_id: row
            .try_get::<Option<i64>, _>("putaway_policy_configuration_id")?
            .map(ConfigurationVersionId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        configuration_revision: row.try_get("putaway_policy_configuration_revision")?,
        configuration_scope,
        require_zone_compatibility,
        enforce_location_capacity,
        allow_mixed_lots,
        policy_hash: row.try_get("putaway_policy_hash")?,
    };
    if !policy.matches_expectation(&policy.expectation())
        || policy.policy_hash
            != putaway_policy_hash(
                require_zone_compatibility,
                enforce_location_capacity,
                allow_mixed_lots,
            )
    {
        return Err(AppError::internal(
            "frozen putaway policy evidence is invalid",
        ));
    }
    Ok(policy)
}

pub(crate) async fn validate_destination_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    destination_location_id: i64,
    contents: &[PutawayContent],
    policy: &PutawayPolicyReadModel,
) -> AppResult<()> {
    if contents.is_empty() || contents.iter().any(|line| line.quantity <= 0) {
        return Err(AppError::internal("putaway policy content is invalid"));
    }

    // Putaway configuration can add planning and execution checks, but it never
    // weakens the active item-storage topology or the loose-inventory lot guard.
    // Those remain database invariants even when the corresponding decision flag
    // is disabled; `allow_mixed_lots` applies to container contents and additional
    // putaway checks, not to unsafe co-mingling of loose balance rows.

    let zone_purpose: Option<String> = sqlx::query_scalar(
        r#"
        SELECT zone.purpose
        FROM storage_zone_locations membership
        INNER JOIN storage_zones zone
          ON zone.tenant_id=membership.tenant_id AND zone.id=membership.storage_zone_id
        WHERE membership.tenant_id=$1 AND membership.facility_id=$2
          AND membership.location_id=$3
          AND zone.effective_from<=statement_timestamp()
          AND (zone.effective_to IS NULL OR zone.effective_to>statement_timestamp())
        ORDER BY zone.id
        LIMIT 1
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(destination_location_id)
    .fetch_optional(&mut **tx)
    .await?;

    let mut totals = BTreeMap::<(i64, String), i64>::new();
    for content in contents {
        totals
            .entry((content.item_id, content.uom.clone()))
            .and_modify(|quantity| *quantity = quantity.saturating_add(content.quantity))
            .or_insert(content.quantity);
    }

    for ((item_id, uom), incoming_quantity) in totals {
        let storage_policy = sqlx::query(
            r#"
            SELECT id,max_quantity_per_location
            FROM item_storage_policies
            WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
              AND item_id=$4 AND uom=$5
              AND effective_from<=statement_timestamp()
              AND (effective_to IS NULL OR effective_to>statement_timestamp())
            ORDER BY revision DESC,id DESC
            LIMIT 1
            FOR SHARE
            "#,
        )
        .bind(tenant_id.get())
        .bind(inventory_owner_id.get())
        .bind(facility_id.get())
        .bind(item_id)
        .bind(&uom)
        .fetch_optional(&mut **tx)
        .await?;

        if policy.require_zone_compatibility {
            let zone_purpose = zone_purpose.as_ref().ok_or_else(|| {
                AppError::conflict("putaway destination has no active storage zone")
            })?;
            let allowed = if let Some(storage_policy) = storage_policy.as_ref() {
                let policy_id: i64 = storage_policy.try_get("id")?;
                sqlx::query_scalar::<_, bool>(
                    r#"
                    SELECT EXISTS(
                      SELECT 1 FROM item_storage_policy_zone_purposes
                      WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
                        AND item_storage_policy_id=$4 AND purpose=$5
                    )
                    "#,
                )
                .bind(tenant_id.get())
                .bind(inventory_owner_id.get())
                .bind(facility_id.get())
                .bind(policy_id)
                .bind(zone_purpose)
                .fetch_one(&mut **tx)
                .await?
            } else {
                false
            };
            if !allowed {
                return Err(AppError::conflict(
                    "putaway destination zone is incompatible with item storage policy",
                ));
            }
        }

        if policy.enforce_location_capacity {
            let maximum = storage_policy
                .as_ref()
                .map(|row| row.try_get::<Option<i64>, _>("max_quantity_per_location"))
                .transpose()?
                .flatten();
            if let Some(maximum) = maximum {
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                    .bind(format!(
                        "putaway-capacity:{}:{}:{}:{}:{}",
                        tenant_id.get(),
                        inventory_owner_id.get(),
                        destination_location_id,
                        item_id,
                        uom
                    ))
                    .execute(&mut **tx)
                    .await?;
                let current: i64 = sqlx::query_scalar(
                    r#"
                    SELECT COALESCE(SUM(qty_on_hand),0)::bigint
                    FROM inventory_balances
                    WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
                      AND location_id=$4 AND item_id=$5 AND uom=$6 AND deleted IS NULL
                    "#,
                )
                .bind(tenant_id.get())
                .bind(inventory_owner_id.get())
                .bind(facility_id.get())
                .bind(destination_location_id)
                .bind(item_id)
                .bind(&uom)
                .fetch_one(&mut **tx)
                .await?;
                if current
                    .checked_add(incoming_quantity)
                    .is_none_or(|projected| projected > maximum)
                {
                    return Err(AppError::conflict(
                        "putaway destination capacity would be exceeded",
                    ));
                }
            }
        }
    }

    if !policy.allow_mixed_lots {
        let mut batches = contents
            .iter()
            .map(|content| (content.item_id, content.item_batch_id))
            .collect::<Vec<_>>();
        batches.sort_unstable();
        batches.dedup();
        for (_, item_batch_id) in batches {
            crate::repo::inventory::ensure_location_accepts_batch_tx(
                tx,
                tenant_id,
                inventory_owner_id.get(),
                destination_location_id,
                item_batch_id,
            )
            .await?;
        }
    }
    Ok(())
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
        _ => Err(AppError::internal("putaway policy scope is invalid")),
    }
}

fn required(row: &sqlx::postgres::PgRow, name: &str) -> AppResult<i64> {
    row.try_get::<Option<i64>, _>(name)?
        .ok_or_else(|| AppError::internal(format!("putaway policy {name} is missing")))
}
