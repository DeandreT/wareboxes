//! Effective replenishment decision policy and immutable planning evidence.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wareboxes_domain::{
    ConfigurationScope, ConfigurationVersionId, DecisionRuleDefinition, ReplenishmentError,
    ReplenishmentLevel, ReplenishmentPolicyThresholds,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentDecisionPolicySource {
    ProductDefault,
    Configuration,
}

impl ReplenishmentDecisionPolicySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductDefault => "product_default",
            Self::Configuration => "configuration",
        }
    }
}

/// The shared rule is percentage-based. Percentages are applied to the item-specific
/// operational target, which remains the explicit pick-face stocking ceiling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplenishmentDecisionPolicyReadModel {
    pub source: ReplenishmentDecisionPolicySource,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub minimum_percent: Option<u8>,
    pub target_percent: Option<u8>,
    pub include_inbound_projection: bool,
    pub operational_minimum: ReplenishmentLevel,
    pub operational_target: ReplenishmentLevel,
    pub effective_minimum: ReplenishmentLevel,
    pub effective_target: ReplenishmentLevel,
    pub policy_hash: String,
}

impl ReplenishmentDecisionPolicyReadModel {
    pub fn product_default(
        operational: ReplenishmentPolicyThresholds,
    ) -> ReplenishmentDecisionPolicyReadModel {
        let operational_minimum = operational.minimum();
        let operational_target = operational.target();
        let mut value = Self {
            source: ReplenishmentDecisionPolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            configuration_scope: None,
            minimum_percent: None,
            target_percent: None,
            include_inbound_projection: true,
            operational_minimum,
            operational_target,
            effective_minimum: operational_minimum,
            effective_target: operational_target,
            policy_hash: String::new(),
        };
        value.policy_hash = replenishment_decision_policy_hash(&value);
        value
    }

    pub fn from_configuration(
        configuration_id: ConfigurationVersionId,
        configuration_revision: i64,
        configuration_scope: ConfigurationScope,
        definition: &DecisionRuleDefinition,
        operational: ReplenishmentPolicyThresholds,
    ) -> Result<Self, ReplenishmentDecisionPolicyError> {
        if configuration_revision <= 0 {
            return Err(ReplenishmentDecisionPolicyError::InvalidConfigurationRevision);
        }
        let DecisionRuleDefinition::Replenishment {
            minimum_percent,
            target_percent,
            include_inbound_projection,
        } = *definition
        else {
            return Err(ReplenishmentDecisionPolicyError::WrongConfigurationKind);
        };
        definition
            .validate()
            .map_err(ReplenishmentDecisionPolicyError::InvalidConfiguration)?;
        let operational_minimum = operational.minimum();
        let operational_target = operational.target();
        let effective_minimum = percentage_floor(operational_target, minimum_percent)?;
        let effective_target = percentage_ceil(operational_target, target_percent)?;
        ReplenishmentPolicyThresholds::new(effective_minimum, effective_target)
            .map_err(ReplenishmentDecisionPolicyError::InvalidEffectiveThresholds)?;
        let mut value = Self {
            source: ReplenishmentDecisionPolicySource::Configuration,
            configuration_id: Some(configuration_id),
            configuration_revision: Some(configuration_revision),
            configuration_scope: Some(configuration_scope),
            minimum_percent: Some(minimum_percent),
            target_percent: Some(target_percent),
            include_inbound_projection,
            operational_minimum,
            operational_target,
            effective_minimum,
            effective_target,
            policy_hash: String::new(),
        };
        value.policy_hash = replenishment_decision_policy_hash(&value);
        Ok(value)
    }

    pub fn effective_thresholds(
        &self,
    ) -> Result<ReplenishmentPolicyThresholds, ReplenishmentError> {
        ReplenishmentPolicyThresholds::new(self.effective_minimum, self.effective_target)
    }

    pub fn is_consistent(&self) -> bool {
        self.policy_hash == replenishment_decision_policy_hash(self)
            && self.effective_thresholds().is_ok()
            && ReplenishmentPolicyThresholds::new(self.operational_minimum, self.operational_target)
                .is_ok()
            && match self.source {
                ReplenishmentDecisionPolicySource::ProductDefault => {
                    self.configuration_id.is_none()
                        && self.configuration_revision.is_none()
                        && self.configuration_scope.is_none()
                        && self.minimum_percent.is_none()
                        && self.target_percent.is_none()
                        && self.include_inbound_projection
                        && self.effective_minimum == self.operational_minimum
                        && self.effective_target == self.operational_target
                }
                ReplenishmentDecisionPolicySource::Configuration => {
                    self.configuration_id.is_some()
                        && self
                            .configuration_revision
                            .is_some_and(|revision| revision > 0)
                        && self.configuration_scope.is_some()
                        && self.minimum_percent.zip(self.target_percent).is_some_and(
                            |(minimum, target)| {
                                minimum < target
                                    && target <= 100
                                    && percentage_floor(self.operational_target, minimum).ok()
                                        == Some(self.effective_minimum)
                                    && percentage_ceil(self.operational_target, target).ok()
                                        == Some(self.effective_target)
                            },
                        )
                }
            }
    }
}

fn percentage_floor(
    quantity: ReplenishmentLevel,
    percent: u8,
) -> Result<ReplenishmentLevel, ReplenishmentDecisionPolicyError> {
    let value = i128::from(quantity.get())
        .checked_mul(i128::from(percent))
        .ok_or(ReplenishmentDecisionPolicyError::QuantityOverflow)?
        / 100;
    ReplenishmentLevel::new(
        i64::try_from(value).map_err(|_| ReplenishmentDecisionPolicyError::QuantityOverflow)?,
    )
    .map_err(ReplenishmentDecisionPolicyError::InvalidEffectiveLevel)
}

fn percentage_ceil(
    quantity: ReplenishmentLevel,
    percent: u8,
) -> Result<ReplenishmentLevel, ReplenishmentDecisionPolicyError> {
    let numerator = i128::from(quantity.get())
        .checked_mul(i128::from(percent))
        .and_then(|value| value.checked_add(99))
        .ok_or(ReplenishmentDecisionPolicyError::QuantityOverflow)?;
    ReplenishmentLevel::new(
        i64::try_from(numerator / 100)
            .map_err(|_| ReplenishmentDecisionPolicyError::QuantityOverflow)?,
    )
    .map_err(ReplenishmentDecisionPolicyError::InvalidEffectiveLevel)
}

pub fn replenishment_decision_policy_hash(policy: &ReplenishmentDecisionPolicyReadModel) -> String {
    let mut digest = Sha256::new();
    digest.update(b"replenishment-decision-policy-v1|");
    digest.update(policy.source.as_str().as_bytes());
    for value in [
        policy.configuration_id.map(ConfigurationVersionId::get),
        policy.configuration_revision,
        policy.minimum_percent.map(i64::from),
        policy.target_percent.map(i64::from),
        Some(policy.operational_minimum.get()),
        Some(policy.operational_target.get()),
        Some(policy.effective_minimum.get()),
        Some(policy.effective_target.get()),
    ] {
        digest.update(b"|");
        digest.update(value.map_or_else(|| "-".to_owned(), |value| value.to_string()));
    }
    digest.update(b"|");
    digest.update(if policy.include_inbound_projection {
        b"true".as_slice()
    } else {
        b"false".as_slice()
    });
    digest.update(b"|");
    digest.update(
        policy
            .configuration_scope
            .map(scope_hash_component)
            .unwrap_or_else(|| "-".to_owned()),
    );
    hex::encode(digest.finalize())
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
pub enum ReplenishmentDecisionPolicyError {
    #[error("resolved configuration is not a replenishment rule")]
    WrongConfigurationKind,
    #[error("resolved replenishment configuration revision is invalid")]
    InvalidConfigurationRevision,
    #[error("resolved replenishment configuration is invalid: {0}")]
    InvalidConfiguration(wareboxes_domain::ConfigurationError),
    #[error("effective replenishment quantity overflow")]
    QuantityOverflow,
    #[error("effective replenishment level is invalid: {0}")]
    InvalidEffectiveLevel(ReplenishmentError),
    #[error("effective replenishment thresholds are invalid: {0}")]
    InvalidEffectiveThresholds(ReplenishmentError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_domain::{FacilityId, InventoryOwnerId};

    fn thresholds(minimum: i64, target: i64) -> ReplenishmentPolicyThresholds {
        ReplenishmentPolicyThresholds::new(
            ReplenishmentLevel::new(minimum).unwrap(),
            ReplenishmentLevel::new(target).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn percentage_policy_uses_floor_minimum_ceil_target_and_stable_hash() {
        let definition = DecisionRuleDefinition::Replenishment {
            minimum_percent: 30,
            target_percent: 80,
            include_inbound_projection: false,
        };
        let policy = ReplenishmentDecisionPolicyReadModel::from_configuration(
            ConfigurationVersionId::new(4).unwrap(),
            3,
            ConfigurationScope::OwnerFacility {
                inventory_owner_id: InventoryOwnerId::new(8).unwrap(),
                facility_id: FacilityId::new(9).unwrap(),
            },
            &definition,
            thresholds(2, 7),
        )
        .unwrap();
        assert_eq!(policy.effective_minimum.get(), 2);
        assert_eq!(policy.effective_target.get(), 6);
        assert!(policy.is_consistent());
        assert_eq!(policy.policy_hash.len(), 64);
    }

    #[test]
    fn product_default_preserves_operational_thresholds_and_inbound_projection() {
        let policy = ReplenishmentDecisionPolicyReadModel::product_default(thresholds(3, 10));
        assert_eq!(policy.effective_minimum.get(), 3);
        assert_eq!(policy.effective_target.get(), 10);
        assert!(policy.include_inbound_projection);
        assert!(policy.is_consistent());
    }

    #[test]
    fn consistency_rejects_a_rehashed_but_misderived_configuration_snapshot() {
        let mut policy = ReplenishmentDecisionPolicyReadModel::from_configuration(
            ConfigurationVersionId::new(4).unwrap(),
            3,
            ConfigurationScope::Tenant,
            &DecisionRuleDefinition::Replenishment {
                minimum_percent: 30,
                target_percent: 80,
                include_inbound_projection: true,
            },
            thresholds(2, 10),
        )
        .unwrap();
        policy.effective_target = ReplenishmentLevel::new(9).unwrap();
        policy.policy_hash = replenishment_decision_policy_hash(&policy);

        assert!(!policy.is_consistent());
    }
}
