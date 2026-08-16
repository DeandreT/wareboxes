//! Effective receipt-policy evidence shared by inbound application workflows.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wareboxes_domain::{ConfigurationScope, ConfigurationVersionId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptPolicySource {
    ProductDefault,
    Configuration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptPolicyExpectation {
    pub source: ReceiptPolicySource,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub policy_hash: String,
}

impl ReceiptPolicyExpectation {
    pub fn is_well_formed(&self) -> bool {
        let identity_is_valid = match self.source {
            ReceiptPolicySource::ProductDefault => {
                self.configuration_id.is_none() && self.configuration_revision.is_none()
            }
            ReceiptPolicySource::Configuration => {
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
pub struct ReceiptPolicyReadModel {
    pub source: ReceiptPolicySource,
    pub configuration_id: Option<ConfigurationVersionId>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub allow_unexpected: bool,
    pub quarantine_unmapped_items: bool,
    pub over_receipt_tolerance_basis_points: u16,
    pub policy_hash: String,
}

impl ReceiptPolicyReadModel {
    pub fn product_default() -> Self {
        let allow_unexpected = true;
        let quarantine_unmapped_items = true;
        let over_receipt_tolerance_basis_points = 10_000;
        Self {
            source: ReceiptPolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            configuration_scope: None,
            allow_unexpected,
            quarantine_unmapped_items,
            over_receipt_tolerance_basis_points,
            policy_hash: receipt_policy_hash(
                allow_unexpected,
                quarantine_unmapped_items,
                over_receipt_tolerance_basis_points,
            ),
        }
    }

    pub fn expectation(&self) -> ReceiptPolicyExpectation {
        ReceiptPolicyExpectation {
            source: self.source,
            configuration_id: self.configuration_id,
            configuration_revision: self.configuration_revision,
            policy_hash: self.policy_hash.clone(),
        }
    }

    pub fn matches_expectation(&self, expected: &ReceiptPolicyExpectation) -> bool {
        expected.is_well_formed() && self.expectation() == *expected
    }
}

pub fn receipt_policy_hash(
    allow_unexpected: bool,
    quarantine_unmapped_items: bool,
    over_receipt_tolerance_basis_points: u16,
) -> String {
    let canonical = format!(
        "receipt-policy-v1|{allow_unexpected}|{quarantine_unmapped_items}|{over_receipt_tolerance_basis_points}"
    );
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_default_hash_and_identity_are_stable() {
        let policy = ReceiptPolicyReadModel::product_default();
        assert_eq!(policy.source, ReceiptPolicySource::ProductDefault);
        assert!(policy.allow_unexpected);
        assert!(policy.quarantine_unmapped_items);
        assert_eq!(policy.over_receipt_tolerance_basis_points, 10_000);
        assert_eq!(
            policy.policy_hash,
            "d52ecae3b5747640fb1bcdf91c7fb3a8800fa4ccce0c220267b44da3a8808326"
        );
        assert!(policy.matches_expectation(&policy.expectation()));
    }
}
