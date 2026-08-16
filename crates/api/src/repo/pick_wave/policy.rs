use std::collections::BTreeMap;

use sqlx::Row;
use wareboxes_application::pick_wave::{PickWavePolicyResolution, ResolvePickWavePoliciesQuery};
use wareboxes_application::wave_policy::{
    wave_policy_hash, WavePolicyExpectation, WavePolicyReadModel, WavePolicySource,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    ConfigurationScope, ConfigurationVersionId, DecisionRuleDefinition, FacilityId,
    InventoryOwnerId, OrderId, TenantId, Timestamp,
};

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub(crate) async fn resolve_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    effective_at: Timestamp,
    serialize_mutations: bool,
) -> AppResult<WavePolicyReadModel> {
    if serialize_mutations {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("configuration-kind:{}:wave", tenant_id.get()))
            .execute(&mut **tx)
            .await?;
    }
    let row = sqlx::query(
        r#"
        SELECT id,revision,scope_level,inventory_owner_id,facility_id,definition
        FROM configuration_versions
        WHERE tenant_id=$1 AND kind='wave' AND status='active'
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
        return Ok(WavePolicyReadModel::product_default());
    };
    let definition = serde_json::from_value::<DecisionRuleDefinition>(row.try_get("definition")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let DecisionRuleDefinition::Wave {
        max_orders,
        require_complete_allocation,
    } = definition
    else {
        return Err(AppError::internal(
            "resolved wave configuration has another rule kind",
        ));
    };
    let configuration_id = ConfigurationVersionId::new(row.try_get("id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let configuration_revision: i64 = row.try_get("revision")?;
    if configuration_revision <= 0 {
        return Err(AppError::internal("wave policy revision is invalid"));
    }
    Ok(WavePolicyReadModel {
        source: WavePolicySource::Configuration,
        configuration_id: Some(configuration_id),
        configuration_revision: Some(configuration_revision),
        configuration_scope: Some(configuration_scope(&row)?),
        max_orders,
        require_complete_allocation,
        policy_hash: wave_policy_hash(max_orders, require_complete_allocation),
    })
}

pub(crate) fn require_expected_policy(
    actual: &WavePolicyReadModel,
    expected: &WavePolicyExpectation,
) -> AppResult<()> {
    if actual.matches_expectation(expected) {
        Ok(())
    } else {
        Err(AppError::conflict(
            "wave policy changed; refresh planning evidence and retry",
        ))
    }
}

pub async fn resolve_policies(
    db: &Db,
    access: &TenantAccess,
    query: &ResolvePickWavePoliciesQuery,
) -> AppResult<Vec<PickWavePolicyResolution>> {
    if query.orders.is_empty() || query.orders.len() > 10_000 {
        return Err(AppError::bad_request(
            "wave policy resolution requires 1..=10000 orders",
        ));
    }
    let expected = query
        .orders
        .iter()
        .map(|order| (order.order_id.get(), order.expected_revision))
        .collect::<BTreeMap<_, _>>();
    if expected.len() != query.orders.len() {
        return Err(AppError::bad_request(
            "wave policy resolution orders must be unique",
        ));
    }
    let ids = expected.keys().copied().collect::<Vec<_>>();
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        "wms_supervisor",
    )
    .await?;
    if !scope.includes_facility(query.facility_id.get()) {
        return Err(AppError::not_found("pick wave order"));
    }
    let rows = sqlx::query(
        r#"
        SELECT orders.id,orders.inventory_owner_id,orders.revision,orders.status
        FROM orders
        JOIN inventory_owner_facilities assignment
          ON assignment.tenant_id=orders.tenant_id
         AND assignment.inventory_owner_id=orders.inventory_owner_id
         AND assignment.facility_id=$3 AND assignment.deleted IS NULL
        WHERE orders.tenant_id=$1 AND orders.id=ANY($2) AND orders.deleted IS NULL
          AND ($4 OR orders.inventory_owner_id=ANY($5))
        ORDER BY orders.id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(&ids)
    .bind(query.facility_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut *tx)
    .await?;
    if rows.len() != ids.len() {
        return Err(AppError::not_found("pick wave order"));
    }
    let effective_at = now_iso();
    let mut policies = BTreeMap::<i64, WavePolicyReadModel>::new();
    let mut resolutions = Vec::with_capacity(rows.len());
    for row in rows {
        let order_id = OrderId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?;
        let owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?;
        if row.try_get::<String, _>("status")? != "open"
            || expected.get(&order_id.get()).map(|revision| revision.get())
                != Some(row.try_get("revision")?)
        {
            return Err(AppError::conflict(
                "pick wave order revision or status is stale",
            ));
        }
        let policy = if let Some(policy) = policies.get(&owner_id.get()) {
            policy.clone()
        } else {
            let policy = resolve_policy_tx(
                &mut tx,
                access.tenant_id,
                owner_id,
                query.facility_id,
                effective_at,
                false,
            )
            .await?;
            policies.insert(owner_id.get(), policy.clone());
            policy
        };
        resolutions.push(PickWavePolicyResolution {
            order_id,
            inventory_owner_id: owner_id,
            policy,
        });
    }
    tx.commit().await?;
    resolutions.sort_by_key(|resolution| resolution.order_id.get());
    Ok(resolutions)
}

pub(super) async fn require_complete_allocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    order_id: OrderId,
) -> AppResult<()> {
    let line_ids = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT line.id FROM order_items line
        WHERE line.tenant_id=$1 AND line.inventory_owner_id=$2
          AND line.order_id=$3 AND line.deleted IS NULL
        ORDER BY line.id FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .fetch_all(&mut **tx)
    .await?;
    if line_ids.is_empty() {
        return Err(AppError::conflict(
            "wave policy requires every order line to be fully allocated",
        ));
    }
    sqlx::query(
        r#"
        SELECT reservation.id FROM inventory_reservations reservation
        WHERE reservation.tenant_id=$1 AND reservation.inventory_owner_id=$2
          AND reservation.facility_id=$3 AND reservation.order_id=$4
        ORDER BY reservation.id FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(facility_id.get())
    .bind(order_id.get())
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        SELECT allocation.id FROM inventory_allocations allocation
        JOIN inventory_reservations reservation
          ON reservation.tenant_id=allocation.tenant_id
         AND reservation.inventory_owner_id=allocation.inventory_owner_id
         AND reservation.id=allocation.reservation_id
        WHERE allocation.tenant_id=$1 AND allocation.inventory_owner_id=$2
          AND allocation.facility_id=$3 AND reservation.order_id=$4
        ORDER BY allocation.id FOR SHARE OF allocation
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(facility_id.get())
    .bind(order_id.get())
    .fetch_all(&mut **tx)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT line.id,demand.effective_qty,
               COALESCE(SUM(allocation.qty) FILTER (
                 WHERE reservation.status='active' AND reservation.deleted IS NULL
                   AND allocation.status='allocated' AND allocation.deleted IS NULL
                   AND allocation.execution_stage='pick_source'
                   AND allocation.facility_id=$4),0)::bigint AS allocated_qty
        FROM order_items line
        JOIN outbound_effective_demand demand
          ON demand.tenant_id=line.tenant_id
         AND demand.inventory_owner_id=line.inventory_owner_id
         AND demand.order_id=line.order_id AND demand.order_item_id=line.id
        LEFT JOIN inventory_reservations reservation
          ON reservation.tenant_id=line.tenant_id
         AND reservation.inventory_owner_id=line.inventory_owner_id
         AND reservation.order_id=line.order_id AND reservation.order_item_id=line.id
        LEFT JOIN inventory_allocations allocation
          ON allocation.tenant_id=reservation.tenant_id
         AND allocation.inventory_owner_id=reservation.inventory_owner_id
         AND allocation.reservation_id=reservation.id
        WHERE line.tenant_id=$1 AND line.inventory_owner_id=$2
          AND line.order_id=$3 AND line.deleted IS NULL
        GROUP BY line.id,demand.effective_qty ORDER BY line.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .bind(facility_id.get())
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != line_ids.len()
        || rows.iter().any(|row| {
            row.try_get::<i64, _>("effective_qty").ok()
                != row.try_get::<i64, _>("allocated_qty").ok()
        })
    {
        return Err(AppError::conflict(
            "wave policy requires every order line to be fully allocated",
        ));
    }
    Ok(())
}

pub(crate) fn source_text(source: WavePolicySource) -> &'static str {
    match source {
        WavePolicySource::ProductDefault => "product_default",
        WavePolicySource::Configuration => "configuration",
    }
}

pub(crate) fn definition_json(policy: &WavePolicyReadModel) -> serde_json::Value {
    serde_json::json!({
        "max_orders": policy.max_orders,
        "require_complete_allocation": policy.require_complete_allocation,
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

pub(super) fn frozen_policy(row: &sqlx::postgres::PgRow) -> AppResult<WavePolicyReadModel> {
    let source = match row.try_get::<String, _>("wave_policy_source")?.as_str() {
        "product_default" => WavePolicySource::ProductDefault,
        "configuration" => WavePolicySource::Configuration,
        _ => return Err(AppError::internal("wave policy source is invalid")),
    };
    let configuration_scope = match row.try_get::<Option<String>, _>("wave_policy_scope_level")? {
        None => None,
        Some(level) => Some(match level.as_str() {
            "tenant" => ConfigurationScope::Tenant,
            "inventory_owner" => ConfigurationScope::InventoryOwner {
                inventory_owner_id: InventoryOwnerId::new(required(
                    row,
                    "wave_policy_scope_owner_id",
                )?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            },
            "facility" => ConfigurationScope::Facility {
                facility_id: FacilityId::new(required(row, "wave_policy_scope_facility_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            },
            "owner_facility" => ConfigurationScope::OwnerFacility {
                inventory_owner_id: InventoryOwnerId::new(required(
                    row,
                    "wave_policy_scope_owner_id",
                )?)
                .map_err(|error| AppError::internal(error.to_string()))?,
                facility_id: FacilityId::new(required(row, "wave_policy_scope_facility_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            },
            _ => return Err(AppError::internal("wave policy scope is invalid")),
        }),
    };
    let definition: serde_json::Value = row.try_get("wave_policy_definition")?;
    let max_orders = definition
        .get("max_orders")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::internal("wave policy order limit is invalid"))?;
    let require_complete_allocation = definition
        .get("require_complete_allocation")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| AppError::internal("wave policy allocation rule is invalid"))?;
    let policy = WavePolicyReadModel {
        source,
        configuration_id: row
            .try_get::<Option<i64>, _>("wave_policy_configuration_id")?
            .map(ConfigurationVersionId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        configuration_revision: row.try_get("wave_policy_configuration_revision")?,
        configuration_scope,
        max_orders,
        require_complete_allocation,
        policy_hash: row.try_get("wave_policy_hash")?,
    };
    if !policy.matches_expectation(&policy.expectation())
        || policy.policy_hash != wave_policy_hash(max_orders, require_complete_allocation)
    {
        return Err(AppError::internal("frozen wave policy evidence is invalid"));
    }
    Ok(policy)
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
        _ => Err(AppError::internal("wave policy scope is invalid")),
    }
}

fn required(row: &sqlx::postgres::PgRow, name: &str) -> AppResult<i64> {
    row.try_get::<Option<i64>, _>(name)?
        .ok_or_else(|| AppError::internal(format!("wave policy {name} is missing")))
}

pub(super) fn unique_owner_counts(
    orders: &[(InventoryOwnerId, &WavePolicyReadModel)],
) -> AppResult<()> {
    let mut counts = BTreeMap::<i64, usize>::new();
    let mut identities = BTreeMap::<i64, WavePolicyExpectation>::new();
    for (owner_id, policy) in orders {
        let count = counts.entry(owner_id.get()).or_default();
        *count = count
            .checked_add(1)
            .ok_or_else(|| AppError::bad_request("too many wave orders"))?;
        let expectation = policy.expectation();
        if identities
            .insert(owner_id.get(), expectation.clone())
            .is_some_and(|existing| existing != expectation)
        {
            return Err(AppError::conflict(
                "one client resolved multiple wave policies",
            ));
        }
        if *count > usize::try_from(policy.max_orders).unwrap_or(usize::MAX) {
            return Err(AppError::conflict(
                "wave order count exceeds the effective client policy",
            ));
        }
    }
    Ok(())
}
