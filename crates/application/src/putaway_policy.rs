//! Effective putaway decision evidence shared by planning and execution.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wareboxes_domain::{ConfigurationScope, ConfigurationVersionId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawayPolicySource {
    ProductDefault,
    Configuration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutawayPolicyExpectation {
    pub source: PutawayPolicySource,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub policy_hash: String,
}

impl PutawayPolicyExpectation {
    pub fn is_well_formed(&self) -> bool {
        let identity_is_valid = match self.source {
            PutawayPolicySource::ProductDefault => {
                self.configuration_id.is_none() && self.configuration_revision.is_none()
            }
            PutawayPolicySource::Configuration => {
                self.configuration_id.is_some()
                    && self
                        .configuration_revision
                        .is_some_and(|revision| revision > 0)
            }
        };
        identity_is_valid
            && self.policy_hash.len() == 64
            && self
                .policy_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutawayPolicyReadModel {
    pub source: PutawayPolicySource,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub require_zone_compatibility: bool,
    pub enforce_location_capacity: bool,
    pub allow_mixed_lots: bool,
    pub policy_hash: String,
}

impl PutawayPolicyReadModel {
    pub fn product_default() -> Self {
        let require_zone_compatibility = false;
        let enforce_location_capacity = false;
        let allow_mixed_lots = false;
        Self {
            source: PutawayPolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            configuration_scope: None,
            require_zone_compatibility,
            enforce_location_capacity,
            allow_mixed_lots,
            policy_hash: putaway_policy_hash(
                require_zone_compatibility,
                enforce_location_capacity,
                allow_mixed_lots,
            ),
        }
    }

    pub fn expectation(&self) -> PutawayPolicyExpectation {
        PutawayPolicyExpectation {
            source: self.source,
            configuration_id: self.configuration_id,
            configuration_revision: self.configuration_revision,
            policy_hash: self.policy_hash.clone(),
        }
    }

    pub fn matches_expectation(&self, expected: &PutawayPolicyExpectation) -> bool {
        expected.is_well_formed() && self.expectation() == *expected
    }
}

pub fn putaway_policy_hash(
    require_zone_compatibility: bool,
    enforce_location_capacity: bool,
    allow_mixed_lots: bool,
) -> String {
    let canonical = format!(
        "putaway-policy-v1|{require_zone_compatibility}|{enforce_location_capacity}|{allow_mixed_lots}"
    );
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_default_hash_and_behavior_are_stable() {
        let policy = PutawayPolicyReadModel::product_default();
        assert!(!policy.require_zone_compatibility);
        assert!(!policy.enforce_location_capacity);
        assert!(!policy.allow_mixed_lots);
        assert_eq!(
            policy.policy_hash,
            "9ebb7234209756a6ff122d74733521612cd2dd38dbb8ed8490e732c9b1625971"
        );
        assert!(policy.matches_expectation(&policy.expectation()));
    }
}
