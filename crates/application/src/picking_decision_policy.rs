//! Effective Pick decision policy frozen when executable work is first claimed.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wareboxes_domain::{ConfigurationScope, ConfigurationVersionId, DecisionRuleDefinition};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickDecisionPolicySource {
    ProductDefault,
    Configuration,
}

impl PickDecisionPolicySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductDefault => "product_default",
            Self::Configuration => "configuration",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickDecisionPolicyReadModel {
    pub source: PickDecisionPolicySource,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub require_source_location_scan: bool,
    pub require_item_scan: bool,
    pub require_destination_container_scan: bool,
    pub policy_hash: String,
}

impl PickDecisionPolicyReadModel {
    pub fn product_default() -> Self {
        Self::new(
            PickDecisionPolicySource::ProductDefault,
            None,
            None,
            None,
            true,
            true,
            true,
        )
    }

    pub fn from_configuration(
        configuration_id: ConfigurationVersionId,
        configuration_revision: i64,
        configuration_scope: ConfigurationScope,
        definition: &DecisionRuleDefinition,
    ) -> Result<Self, PickDecisionPolicyError> {
        if configuration_revision <= 0 {
            return Err(PickDecisionPolicyError::InvalidConfigurationRevision);
        }
        let DecisionRuleDefinition::Pick {
            require_source_location_scan,
            require_item_scan,
            require_destination_container_scan,
        } = *definition
        else {
            return Err(PickDecisionPolicyError::WrongConfigurationKind);
        };
        definition
            .validate()
            .map_err(PickDecisionPolicyError::InvalidConfiguration)?;
        Ok(Self::new(
            PickDecisionPolicySource::Configuration,
            Some(configuration_id),
            Some(configuration_revision),
            Some(configuration_scope),
            require_source_location_scan,
            require_item_scan,
            require_destination_container_scan,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        source: PickDecisionPolicySource,
        configuration_id: Option<ConfigurationVersionId>,
        configuration_revision: Option<i64>,
        configuration_scope: Option<ConfigurationScope>,
        require_source_location_scan: bool,
        require_item_scan: bool,
        require_destination_container_scan: bool,
    ) -> Self {
        let policy_hash = pick_decision_policy_hash(
            source,
            configuration_id,
            configuration_revision,
            configuration_scope,
            require_source_location_scan,
            require_item_scan,
            require_destination_container_scan,
        );
        Self {
            source,
            configuration_id,
            configuration_revision,
            configuration_scope,
            require_source_location_scan,
            require_item_scan,
            require_destination_container_scan,
            policy_hash,
        }
    }

    pub fn is_consistent(&self) -> bool {
        let identity_is_valid = match self.source {
            PickDecisionPolicySource::ProductDefault => {
                self.configuration_id.is_none()
                    && self.configuration_revision.is_none()
                    && self.configuration_scope.is_none()
                    && self.require_source_location_scan
                    && self.require_item_scan
                    && self.require_destination_container_scan
            }
            PickDecisionPolicySource::Configuration => {
                self.configuration_id.is_some()
                    && self
                        .configuration_revision
                        .is_some_and(|revision| revision > 0)
                    && self.configuration_scope.is_some()
            }
        };
        identity_is_valid
            && self.policy_hash
                == pick_decision_policy_hash(
                    self.source,
                    self.configuration_id,
                    self.configuration_revision,
                    self.configuration_scope,
                    self.require_source_location_scan,
                    self.require_item_scan,
                    self.require_destination_container_scan,
                )
    }
}

#[allow(clippy::too_many_arguments)]
pub fn pick_decision_policy_hash(
    source: PickDecisionPolicySource,
    configuration_id: Option<ConfigurationVersionId>,
    configuration_revision: Option<i64>,
    configuration_scope: Option<ConfigurationScope>,
    require_source_location_scan: bool,
    require_item_scan: bool,
    require_destination_container_scan: bool,
) -> String {
    let canonical = format!(
        "pick-decision-policy-v1|{}|{}|{}|{}|{}|{}|{}",
        source.as_str(),
        configuration_id.map_or_else(|| "-".to_owned(), |id| id.get().to_string()),
        configuration_revision.map_or_else(|| "-".to_owned(), |value| value.to_string()),
        configuration_scope.map_or_else(|| "-".to_owned(), scope_hash_component),
        require_source_location_scan,
        require_item_scan,
        require_destination_container_scan,
    );
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

fn scope_hash_component(scope: ConfigurationScope) -> String {
    match scope {
        ConfigurationScope::Tenant => "tenant".to_owned(),
        ConfigurationScope::InventoryOwner { inventory_owner_id } => {
            format!("inventory_owner:{}", inventory_owner_id.get())
        }
        ConfigurationScope::Facility { facility_id } => {
            format!("facility:{}", facility_id.get())
        }
        ConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        } => format!(
            "owner_facility:{}:{}",
            inventory_owner_id.get(),
            facility_id.get()
        ),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PickDecisionPolicyError {
    #[error("resolved configuration is not a Pick rule")]
    WrongConfigurationKind,
    #[error("resolved Pick configuration revision is invalid")]
    InvalidConfigurationRevision,
    #[error("resolved Pick configuration is invalid: {0}")]
    InvalidConfiguration(wareboxes_domain::ConfigurationError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_domain::{FacilityId, InventoryOwnerId};

    #[test]
    fn product_default_preserves_existing_scan_safety() {
        let policy = PickDecisionPolicyReadModel::product_default();
        assert!(policy.require_source_location_scan);
        assert!(policy.require_item_scan);
        assert!(policy.require_destination_container_scan);
        assert!(policy.is_consistent());
    }

    #[test]
    fn configuration_identity_and_behavior_are_hashed() {
        let definition = DecisionRuleDefinition::Pick {
            require_source_location_scan: false,
            require_item_scan: true,
            require_destination_container_scan: false,
        };
        let policy = PickDecisionPolicyReadModel::from_configuration(
            ConfigurationVersionId::new(7).unwrap(),
            3,
            ConfigurationScope::OwnerFacility {
                inventory_owner_id: InventoryOwnerId::new(11).unwrap(),
                facility_id: FacilityId::new(13).unwrap(),
            },
            &definition,
        )
        .unwrap();
        assert!(policy.is_consistent());
        let mut changed = policy;
        changed.require_item_scan = false;
        assert!(!changed.is_consistent());
    }
}
