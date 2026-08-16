use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{FacilityId, InventoryOwnerId, ReceivingValidationError};

const PRODUCT_DEFAULT_HASH: &str =
    "d52ecae3b5747640fb1bcdf91c7fb3a8800fa4ccce0c220267b44da3a8808326";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptPolicySource {
    ProductDefault,
    Configuration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "level", deny_unknown_fields)]
pub enum ReceiptPolicyScope {
    Tenant,
    InventoryOwner {
        inventory_owner_id: InventoryOwnerId,
    },
    Facility {
        facility_id: FacilityId,
    },
    OwnerFacility {
        inventory_owner_id: InventoryOwnerId,
        facility_id: FacilityId,
    },
}

/// Exact effective policy frozen into a scanner session and every durable
/// unexpected-receipt command derived from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ReceiptPolicyInput", into = "ReceiptPolicyInput")]
pub struct ReceiptPolicy {
    source: ReceiptPolicySource,
    configuration_id: Option<i64>,
    configuration_revision: Option<i64>,
    configuration_scope: Option<ReceiptPolicyScope>,
    allow_unexpected: bool,
    quarantine_unmapped_items: bool,
    over_receipt_tolerance_basis_points: u16,
    policy_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptPolicyInput {
    pub source: ReceiptPolicySource,
    pub configuration_id: Option<i64>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ReceiptPolicyScope>,
    pub allow_unexpected: bool,
    pub quarantine_unmapped_items: bool,
    pub over_receipt_tolerance_basis_points: u16,
    pub policy_hash: String,
}

impl TryFrom<ReceiptPolicyInput> for ReceiptPolicy {
    type Error = ReceivingValidationError;

    fn try_from(input: ReceiptPolicyInput) -> Result<Self, Self::Error> {
        Self::try_new(input)
    }
}

impl From<ReceiptPolicy> for ReceiptPolicyInput {
    fn from(policy: ReceiptPolicy) -> Self {
        Self {
            source: policy.source,
            configuration_id: policy.configuration_id,
            configuration_revision: policy.configuration_revision,
            configuration_scope: policy.configuration_scope,
            allow_unexpected: policy.allow_unexpected,
            quarantine_unmapped_items: policy.quarantine_unmapped_items,
            over_receipt_tolerance_basis_points: policy.over_receipt_tolerance_basis_points,
            policy_hash: policy.policy_hash,
        }
    }
}

impl ReceiptPolicy {
    pub fn try_new(input: ReceiptPolicyInput) -> Result<Self, ReceivingValidationError> {
        let identity_is_valid = match input.source {
            ReceiptPolicySource::ProductDefault => {
                input.configuration_id.is_none()
                    && input.configuration_revision.is_none()
                    && input.configuration_scope.is_none()
                    && input.allow_unexpected
                    && input.quarantine_unmapped_items
                    && input.over_receipt_tolerance_basis_points == 10_000
                    && input.policy_hash == PRODUCT_DEFAULT_HASH
            }
            ReceiptPolicySource::Configuration => {
                input.configuration_id.is_some_and(|id| id > 0)
                    && input
                        .configuration_revision
                        .is_some_and(|revision| revision > 0)
                    && input.configuration_scope.is_some()
            }
        };
        if !identity_is_valid || input.over_receipt_tolerance_basis_points > 10_000 {
            return Err(ReceivingValidationError::InvalidReceiptPolicy);
        }
        let expected_hash = receipt_policy_hash(
            input.allow_unexpected,
            input.quarantine_unmapped_items,
            input.over_receipt_tolerance_basis_points,
        );
        if input.policy_hash != expected_hash {
            return Err(ReceivingValidationError::InvalidReceiptPolicy);
        }
        Ok(Self {
            source: input.source,
            configuration_id: input.configuration_id,
            configuration_revision: input.configuration_revision,
            configuration_scope: input.configuration_scope,
            allow_unexpected: input.allow_unexpected,
            quarantine_unmapped_items: input.quarantine_unmapped_items,
            over_receipt_tolerance_basis_points: input.over_receipt_tolerance_basis_points,
            policy_hash: input.policy_hash,
        })
    }

    #[must_use]
    pub fn product_default() -> Self {
        Self {
            source: ReceiptPolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            configuration_scope: None,
            allow_unexpected: true,
            quarantine_unmapped_items: true,
            over_receipt_tolerance_basis_points: 10_000,
            policy_hash: PRODUCT_DEFAULT_HASH.to_owned(),
        }
    }

    #[must_use]
    pub const fn source(&self) -> ReceiptPolicySource {
        self.source
    }

    #[must_use]
    pub const fn configuration_id(&self) -> Option<i64> {
        self.configuration_id
    }

    #[must_use]
    pub const fn configuration_revision(&self) -> Option<i64> {
        self.configuration_revision
    }

    #[must_use]
    pub const fn configuration_scope(&self) -> Option<ReceiptPolicyScope> {
        self.configuration_scope
    }

    #[must_use]
    pub const fn allow_unexpected(&self) -> bool {
        self.allow_unexpected
    }

    #[must_use]
    pub const fn quarantine_unmapped_items(&self) -> bool {
        self.quarantine_unmapped_items
    }

    #[must_use]
    pub const fn over_receipt_tolerance_basis_points(&self) -> u16 {
        self.over_receipt_tolerance_basis_points
    }

    #[must_use]
    pub fn policy_hash(&self) -> &str {
        &self.policy_hash
    }
}

fn receipt_policy_hash(
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
    fn product_default_is_stable_and_valid() {
        let policy = ReceiptPolicy::product_default();
        assert_eq!(policy.source(), ReceiptPolicySource::ProductDefault);
        assert_eq!(policy.policy_hash(), PRODUCT_DEFAULT_HASH);
        assert!(
            ReceiptPolicy::try_new(ReceiptPolicyInput {
                source: policy.source(),
                configuration_id: policy.configuration_id(),
                configuration_revision: policy.configuration_revision(),
                configuration_scope: policy.configuration_scope(),
                allow_unexpected: policy.allow_unexpected(),
                quarantine_unmapped_items: policy.quarantine_unmapped_items(),
                over_receipt_tolerance_basis_points: policy.over_receipt_tolerance_basis_points(),
                policy_hash: policy.policy_hash().to_owned(),
            })
            .is_ok()
        );
    }

    #[test]
    fn deserialization_revalidates_policy_evidence() {
        let mut value = serde_json::to_value(ReceiptPolicy::product_default()).unwrap();
        value["policy_hash"] = serde_json::Value::String("0".repeat(64));
        assert!(serde_json::from_value::<ReceiptPolicy>(value).is_err());
    }
}
