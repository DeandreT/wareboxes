//! Effective wave decision evidence shared by planning and execution.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wareboxes_domain::{ConfigurationScope, ConfigurationVersionId, MAX_WAVE_ORDERS};

pub const PRODUCT_DEFAULT_WAVE_POLICY_HASH: &str =
    "03e485c29e6c4e032786157f4f1e216bd741a35ef6f6c3895b35e9c579f443b9";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WavePolicySource {
    ProductDefault,
    Configuration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WavePolicyExpectation {
    pub source: WavePolicySource,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub policy_hash: String,
}

impl WavePolicyExpectation {
    pub fn is_well_formed(&self) -> bool {
        let identity_is_valid = match self.source {
            WavePolicySource::ProductDefault => {
                self.configuration_id.is_none() && self.configuration_revision.is_none()
            }
            WavePolicySource::Configuration => {
                self.configuration_id.is_some()
                    && self
                        .configuration_revision
                        .is_some_and(|revision| revision > 0)
            }
        };
        identity_is_valid
            && (self.source != WavePolicySource::ProductDefault
                || self.policy_hash == PRODUCT_DEFAULT_WAVE_POLICY_HASH)
            && self.policy_hash.len() == 64
            && self
                .policy_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WavePolicyReadModel {
    pub source: WavePolicySource,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub max_orders: u32,
    pub require_complete_allocation: bool,
    pub policy_hash: String,
}

impl WavePolicyReadModel {
    pub fn product_default() -> Self {
        Self {
            source: WavePolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            configuration_scope: None,
            max_orders: MAX_WAVE_ORDERS,
            require_complete_allocation: false,
            policy_hash: PRODUCT_DEFAULT_WAVE_POLICY_HASH.to_owned(),
        }
    }

    pub fn expectation(&self) -> WavePolicyExpectation {
        WavePolicyExpectation {
            source: self.source,
            configuration_id: self.configuration_id,
            configuration_revision: self.configuration_revision,
            policy_hash: self.policy_hash.clone(),
        }
    }

    pub fn matches_expectation(&self, expected: &WavePolicyExpectation) -> bool {
        expected.is_well_formed() && self.expectation() == *expected
    }
}

pub fn wave_policy_hash(max_orders: u32, require_complete_allocation: bool) -> String {
    let canonical = format!("wave-policy-v1|{max_orders}|{require_complete_allocation}");
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_default_hash_and_behavior_are_stable() {
        let policy = WavePolicyReadModel::product_default();
        assert_eq!(policy.max_orders, MAX_WAVE_ORDERS);
        assert!(!policy.require_complete_allocation);
        assert_eq!(policy.policy_hash, PRODUCT_DEFAULT_WAVE_POLICY_HASH);
        assert_eq!(
            wave_policy_hash(MAX_WAVE_ORDERS, false),
            PRODUCT_DEFAULT_WAVE_POLICY_HASH
        );
        assert!(policy.matches_expectation(&policy.expectation()));
    }
}
