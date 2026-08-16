use wareboxes_application::order_allocation::{AllocationPolicyReadModel, AllocationPolicySource};
use wareboxes_application::picking::ReallocatePickShortageCommand;
use wareboxes_domain::{
    AllocationOutcome, ConfigurationScope, OrderRevision, PickShortageReallocationRunId,
    PickShortageRevision, TenantId, Timestamp,
};

use super::LockedShortage;
use crate::error::{AppError, AppResult};

#[allow(clippy::too_many_arguments)]
pub(super) async fn insert_run_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_user_id: i64,
    command: &ReallocatePickShortageCommand,
    policy: &AllocationPolicyReadModel,
    shortage: &LockedShortage,
    resulting_shortage_revision: PickShortageRevision,
    resulting_order_revision: OrderRevision,
    outcome: AllocationOutcome,
    allocated_quantity: i64,
    remaining_quantity: i64,
    allocation_count: i64,
    occurred_at: Timestamp,
) -> AppResult<PickShortageReallocationRunId> {
    let (scope_level, policy_owner_id, policy_facility_id) = policy_scope_values(policy);
    let policy_definition = serde_json::json!({
        "kind": "allocation",
        "rotation": policy.strategy.as_str(),
        "allow_partial": policy.allow_partial,
        "require_complete_line": policy.require_complete_line,
    });
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO pick_shortage_reallocation_runs (
            tenant_id, inventory_owner_id, facility_id, order_release_id,
            order_id, order_item_id, reservation_id, pick_shortage_id,
            created_by_user_id, created_at, expected_shortage_revision,
            resulting_shortage_revision, expected_order_revision,
            resulting_order_revision, requested_qty, allocated_qty,
            remaining_qty, allocation_count, outcome, strategy, policy_source,
            policy_configuration_id, policy_configuration_revision,
            policy_scope_level, policy_inventory_owner_id, policy_facility_id,
            policy_definition, policy_hash
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
            $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23,
            $24, $25, $26, $27, $28
        ) RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shortage.inventory_owner_id.get())
    .bind(shortage.facility_id)
    .bind(shortage.release_id)
    .bind(shortage.order_id.get())
    .bind(shortage.order_item_id)
    .bind(shortage.reservation_id)
    .bind(shortage.id.get())
    .bind(actor_user_id)
    .bind(occurred_at)
    .bind(command.expected_shortage_revision.get())
    .bind(resulting_shortage_revision.get())
    .bind(command.expected_order_revision.get())
    .bind(resulting_order_revision.get())
    .bind(shortage.remaining_quantity)
    .bind(allocated_quantity)
    .bind(remaining_quantity)
    .bind(allocation_count)
    .bind(outcome.as_str())
    .bind(policy.strategy.as_str())
    .bind(match policy.source {
        AllocationPolicySource::ProductDefault => "product_default",
        AllocationPolicySource::Configuration => "configuration",
    })
    .bind(policy.configuration_id.map(|id| id.get()))
    .bind(policy.configuration_revision)
    .bind(scope_level)
    .bind(policy_owner_id)
    .bind(policy_facility_id)
    .bind(policy_definition)
    .bind(&policy.policy_hash)
    .fetch_one(&mut **tx)
    .await?;
    PickShortageReallocationRunId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

fn policy_scope_values(
    policy: &AllocationPolicyReadModel,
) -> (Option<&'static str>, Option<i64>, Option<i64>) {
    match policy.configuration_scope {
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
