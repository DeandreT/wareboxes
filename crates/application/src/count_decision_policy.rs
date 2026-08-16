//! Effective Count decision evidence frozen with cycle-count results.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wareboxes_domain::{
    ConfigurationScope, ConfigurationVersionId, CycleCountTolerancePolicy, DecisionRuleDefinition,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CountDecisionPolicySource {
    ProductDefault,
    Configuration,
}

impl CountDecisionPolicySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductDefault => "product_default",
            Self::Configuration => "configuration",
        }
    }
}

/// The operational cycle-count policy owns the recount limit. The inherited
/// Count rule may override tolerance and route material variances directly to
/// supervisor approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountDecisionPolicyReadModel {
    pub source: CountDecisionPolicySource,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub absolute_tolerance_quantity: i64,
    pub percentage_tolerance_basis_points: u32,
    pub approval_threshold_quantity: Option<i64>,
    pub policy_hash: String,
}

impl CountDecisionPolicyReadModel {
    pub fn product_default(operational_policy: CycleCountTolerancePolicy) -> Self {
        Self::new(
            CountDecisionPolicySource::ProductDefault,
            None,
            None,
            None,
            operational_policy.absolute_tolerance_quantity(),
            operational_policy.percentage_tolerance_basis_points(),
            None,
        )
    }

    pub fn from_configuration(
        configuration_id: ConfigurationVersionId,
        configuration_revision: i64,
        configuration_scope: ConfigurationScope,
        definition: &DecisionRuleDefinition,
    ) -> Result<Self, CountDecisionPolicyError> {
        if configuration_revision <= 0 {
            return Err(CountDecisionPolicyError::InvalidConfigurationRevision);
        }
        let DecisionRuleDefinition::Count {
            absolute_tolerance,
            percentage_tolerance_basis_points,
            approval_threshold,
        } = *definition
        else {
            return Err(CountDecisionPolicyError::WrongConfigurationKind);
        };
        definition
            .validate()
            .map_err(CountDecisionPolicyError::InvalidConfiguration)?;
        Ok(Self::new(
            CountDecisionPolicySource::Configuration,
            Some(configuration_id),
            Some(configuration_revision),
            Some(configuration_scope),
            absolute_tolerance,
            u32::from(percentage_tolerance_basis_points),
            Some(approval_threshold),
        ))
    }

    pub fn tolerance_policy(
        &self,
        automatic_recount_limit: u16,
    ) -> Result<CycleCountTolerancePolicy, CountDecisionPolicyError> {
        CycleCountTolerancePolicy::new(
            self.absolute_tolerance_quantity,
            self.percentage_tolerance_basis_points,
            automatic_recount_limit,
        )
        .map_err(CountDecisionPolicyError::InvalidTolerance)
    }

    pub fn is_consistent(&self) -> bool {
        let identity_valid = match self.source {
            CountDecisionPolicySource::ProductDefault => {
                self.configuration_id.is_none()
                    && self.configuration_revision.is_none()
                    && self.configuration_scope.is_none()
                    && self.approval_threshold_quantity.is_none()
            }
            CountDecisionPolicySource::Configuration => {
                self.configuration_id.is_some()
                    && self
                        .configuration_revision
                        .is_some_and(|revision| revision > 0)
                    && self.configuration_scope.is_some()
                    && self
                        .approval_threshold_quantity
                        .is_some_and(|threshold| threshold >= self.absolute_tolerance_quantity)
            }
        };
        identity_valid
            && self.absolute_tolerance_quantity >= 0
            && self.percentage_tolerance_basis_points <= 10_000
            && self.policy_hash
                == count_decision_policy_hash(
                    self.source,
                    self.configuration_id,
                    self.configuration_revision,
                    self.configuration_scope,
                    self.absolute_tolerance_quantity,
                    self.percentage_tolerance_basis_points,
                    self.approval_threshold_quantity,
                )
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        source: CountDecisionPolicySource,
        configuration_id: Option<ConfigurationVersionId>,
        configuration_revision: Option<i64>,
        configuration_scope: Option<ConfigurationScope>,
        absolute_tolerance_quantity: i64,
        percentage_tolerance_basis_points: u32,
        approval_threshold_quantity: Option<i64>,
    ) -> Self {
        let policy_hash = count_decision_policy_hash(
            source,
            configuration_id,
            configuration_revision,
            configuration_scope,
            absolute_tolerance_quantity,
            percentage_tolerance_basis_points,
            approval_threshold_quantity,
        );
        Self {
            source,
            configuration_id,
            configuration_revision,
            configuration_scope,
            absolute_tolerance_quantity,
            percentage_tolerance_basis_points,
            approval_threshold_quantity,
            policy_hash,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn count_decision_policy_hash(
    source: CountDecisionPolicySource,
    configuration_id: Option<ConfigurationVersionId>,
    configuration_revision: Option<i64>,
    configuration_scope: Option<ConfigurationScope>,
    absolute_tolerance_quantity: i64,
    percentage_tolerance_basis_points: u32,
    approval_threshold_quantity: Option<i64>,
) -> String {
    let canonical = format!(
        "count-decision-policy-v1|{}|{}|{}|{}|{}|{}|{}",
        source.as_str(),
        configuration_id.map_or_else(|| "-".to_owned(), |id| id.get().to_string()),
        configuration_revision.map_or_else(|| "-".to_owned(), |value| value.to_string()),
        configuration_scope.map_or_else(|| "-".to_owned(), scope_hash_component),
        absolute_tolerance_quantity,
        percentage_tolerance_basis_points,
        approval_threshold_quantity.map_or_else(|| "-".to_owned(), |value| value.to_string()),
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
pub enum CountDecisionPolicyError {
    #[error("resolved configuration is not a Count rule")]
    WrongConfigurationKind,
    #[error("resolved Count configuration revision is invalid")]
    InvalidConfigurationRevision,
    #[error("resolved Count configuration is invalid: {0}")]
    InvalidConfiguration(wareboxes_domain::ConfigurationError),
    #[error("effective Count tolerance is invalid: {0}")]
    InvalidTolerance(wareboxes_domain::CycleCountError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_domain::{FacilityId, InventoryOwnerId};

    #[test]
    fn product_default_freezes_operational_tolerance() {
        let policy = CountDecisionPolicyReadModel::product_default(
            CycleCountTolerancePolicy::new(2, 250, 1).unwrap(),
        );
        assert!(policy.is_consistent());
        assert_eq!(policy.absolute_tolerance_quantity, 2);
        assert_eq!(policy.approval_threshold_quantity, None);
    }

    #[test]
    fn configured_identity_and_threshold_are_hashed() {
        let policy = CountDecisionPolicyReadModel::from_configuration(
            ConfigurationVersionId::new(17).unwrap(),
            4,
            ConfigurationScope::OwnerFacility {
                inventory_owner_id: InventoryOwnerId::new(19).unwrap(),
                facility_id: FacilityId::new(23).unwrap(),
            },
            &DecisionRuleDefinition::Count {
                absolute_tolerance: 1,
                percentage_tolerance_basis_points: 200,
                approval_threshold: 5,
            },
        )
        .unwrap();
        assert!(policy.is_consistent());
        let mut tampered = policy;
        tampered.approval_threshold_quantity = Some(6);
        assert!(!tampered.is_consistent());
    }
}
