use wareboxes_api_contract::v1::{
    ConfigurationScope as ApiConfigurationScope, ReplenishmentDecisionPolicyResponse,
    ReplenishmentDecisionPolicySource as ApiPolicySource,
};
use wareboxes_application::replenishment_decision_policy::{
    ReplenishmentDecisionPolicyReadModel, ReplenishmentDecisionPolicySource,
};
use wareboxes_domain::ConfigurationScope;

use super::super::error::V1Result;
use super::revision;

pub(super) fn map_decision_policy(
    value: ReplenishmentDecisionPolicyReadModel,
) -> V1Result<ReplenishmentDecisionPolicyResponse> {
    Ok(ReplenishmentDecisionPolicyResponse {
        source: match value.source {
            ReplenishmentDecisionPolicySource::ProductDefault => ApiPolicySource::ProductDefault,
            ReplenishmentDecisionPolicySource::Configuration => ApiPolicySource::Configuration,
        },
        configuration_id: value.configuration_id.map(|id| id.get()),
        configuration_revision: value.configuration_revision.map(revision).transpose()?,
        configuration_scope: value.configuration_scope.map(map_scope),
        minimum_percent: value.minimum_percent,
        target_percent: value.target_percent,
        include_inbound_projection: value.include_inbound_projection,
        operational_minimum_quantity: value.operational_minimum.get(),
        operational_target_quantity: value.operational_target.get(),
        effective_minimum_quantity: value.effective_minimum.get(),
        effective_target_quantity: value.effective_target.get(),
        policy_hash: value.policy_hash,
    })
}

const fn map_scope(scope: ConfigurationScope) -> ApiConfigurationScope {
    match scope {
        ConfigurationScope::Tenant => ApiConfigurationScope::Tenant,
        ConfigurationScope::InventoryOwner { inventory_owner_id } => {
            ApiConfigurationScope::InventoryOwner {
                inventory_owner_id: inventory_owner_id.get(),
            }
        }
        ConfigurationScope::Facility { facility_id } => ApiConfigurationScope::Facility {
            facility_id: facility_id.get(),
        },
        ConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        } => ApiConfigurationScope::OwnerFacility {
            inventory_owner_id: inventory_owner_id.get(),
            facility_id: facility_id.get(),
        },
    }
}
