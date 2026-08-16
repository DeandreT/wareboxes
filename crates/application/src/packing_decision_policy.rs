//! Effective Pack decision evidence frozen when a station session opens.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wareboxes_domain::{ConfigurationScope, ConfigurationVersionId, DecisionRuleDefinition};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackDecisionPolicySource {
    ProductDefault,
    Configuration,
}

impl PackDecisionPolicySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductDefault => "product_default",
            Self::Configuration => "configuration",
        }
    }
}

/// `allow_mixed_orders` controls concurrent open order sessions at a physical
/// station. Cartons and their inventory remain strictly order-pure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackDecisionPolicyReadModel {
    pub source: PackDecisionPolicySource,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub require_station_scan: bool,
    pub require_weight: bool,
    pub allow_mixed_orders: bool,
    pub policy_hash: String,
}

impl PackDecisionPolicyReadModel {
    pub fn product_default() -> Self {
        Self::new(
            PackDecisionPolicySource::ProductDefault,
            None,
            None,
            None,
            false,
            false,
            true,
        )
    }

    pub fn from_configuration(
        configuration_id: ConfigurationVersionId,
        configuration_revision: i64,
        configuration_scope: ConfigurationScope,
        definition: &DecisionRuleDefinition,
    ) -> Result<Self, PackDecisionPolicyError> {
        if configuration_revision <= 0 {
            return Err(PackDecisionPolicyError::InvalidConfigurationRevision);
        }
        let DecisionRuleDefinition::Pack {
            require_station_scan,
            require_weight,
            allow_mixed_orders,
        } = *definition
        else {
            return Err(PackDecisionPolicyError::WrongConfigurationKind);
        };
        definition
            .validate()
            .map_err(PackDecisionPolicyError::InvalidConfiguration)?;
        Ok(Self::new(
            PackDecisionPolicySource::Configuration,
            Some(configuration_id),
            Some(configuration_revision),
            Some(configuration_scope),
            require_station_scan,
            require_weight,
            allow_mixed_orders,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        source: PackDecisionPolicySource,
        configuration_id: Option<ConfigurationVersionId>,
        configuration_revision: Option<i64>,
        configuration_scope: Option<ConfigurationScope>,
        require_station_scan: bool,
        require_weight: bool,
        allow_mixed_orders: bool,
    ) -> Self {
        let policy_hash = pack_decision_policy_hash(
            source,
            configuration_id,
            configuration_revision,
            configuration_scope,
            require_station_scan,
            require_weight,
            allow_mixed_orders,
        );
        Self {
            source,
            configuration_id,
            configuration_revision,
            configuration_scope,
            require_station_scan,
            require_weight,
            allow_mixed_orders,
            policy_hash,
        }
    }

    pub fn is_consistent(&self) -> bool {
        let identity_valid = match self.source {
            PackDecisionPolicySource::ProductDefault => {
                self.configuration_id.is_none()
                    && self.configuration_revision.is_none()
                    && self.configuration_scope.is_none()
                    && !self.require_station_scan
                    && !self.require_weight
                    && self.allow_mixed_orders
            }
            PackDecisionPolicySource::Configuration => {
                self.configuration_id.is_some()
                    && self
                        .configuration_revision
                        .is_some_and(|revision| revision > 0)
                    && self.configuration_scope.is_some()
            }
        };
        identity_valid
            && self.policy_hash
                == pack_decision_policy_hash(
                    self.source,
                    self.configuration_id,
                    self.configuration_revision,
                    self.configuration_scope,
                    self.require_station_scan,
                    self.require_weight,
                    self.allow_mixed_orders,
                )
    }
}

#[allow(clippy::too_many_arguments)]
pub fn pack_decision_policy_hash(
    source: PackDecisionPolicySource,
    configuration_id: Option<ConfigurationVersionId>,
    configuration_revision: Option<i64>,
    configuration_scope: Option<ConfigurationScope>,
    require_station_scan: bool,
    require_weight: bool,
    allow_mixed_orders: bool,
) -> String {
    let canonical = format!(
        "pack-decision-policy-v1|{}|{}|{}|{}|{}|{}|{}",
        source.as_str(),
        configuration_id.map_or_else(|| "-".to_owned(), |id| id.get().to_string()),
        configuration_revision.map_or_else(|| "-".to_owned(), |value| value.to_string()),
        configuration_scope.map_or_else(|| "-".to_owned(), scope_hash_component),
        require_station_scan,
        require_weight,
        allow_mixed_orders,
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
pub enum PackDecisionPolicyError {
    #[error("resolved configuration is not a Pack rule")]
    WrongConfigurationKind,
    #[error("resolved Pack configuration revision is invalid")]
    InvalidConfigurationRevision,
    #[error("resolved Pack configuration is invalid: {0}")]
    InvalidConfiguration(wareboxes_domain::ConfigurationError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_domain::{FacilityId, InventoryOwnerId};

    #[test]
    fn product_default_preserves_existing_pack_behavior() {
        let policy = PackDecisionPolicyReadModel::product_default();
        assert!(!policy.require_station_scan);
        assert!(!policy.require_weight);
        assert!(policy.allow_mixed_orders);
        assert_eq!(
            policy.policy_hash,
            "a5fc7b3c670e596ce983c7ff342c1351cd95be99c889e0d12a8b26abbaa4ac57"
        );
        assert!(policy.is_consistent());
    }

    #[test]
    fn configured_identity_and_execution_flags_are_hashed() {
        let policy = PackDecisionPolicyReadModel::from_configuration(
            ConfigurationVersionId::new(17).unwrap(),
            4,
            ConfigurationScope::OwnerFacility {
                inventory_owner_id: InventoryOwnerId::new(19).unwrap(),
                facility_id: FacilityId::new(23).unwrap(),
            },
            &DecisionRuleDefinition::Pack {
                require_station_scan: true,
                require_weight: true,
                allow_mixed_orders: false,
            },
        )
        .unwrap();
        assert!(policy.is_consistent());
        let mut tampered = policy;
        tampered.require_weight = false;
        assert!(!tampered.is_consistent());
    }
}
