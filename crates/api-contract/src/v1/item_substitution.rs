use serde::{Deserialize, Serialize};

use super::Revision;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureItemSubstitutionPolicyRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub source_item_id: i64,
    pub source_uom: String,
    pub substitute_item_id: i64,
    pub substitute_uom: String,
    pub source_quantity: i64,
    pub substitute_quantity: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetireItemSubstitutionPolicyRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemSubstitutionPolicyResponse {
    pub policy_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub source_item_id: i64,
    pub source_uom: String,
    pub substitute_item_id: i64,
    pub substitute_uom: String,
    pub source_quantity: i64,
    pub substitute_quantity: i64,
    pub revision: Revision,
    pub active: bool,
    pub configured_by: i64,
    pub configured_at: String,
    pub retired_by: Option<i64>,
    pub retired_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemSubstitutionPolicyListRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    #[serde(default)]
    pub source_item_id: Option<i64>,
    #[serde(default = "default_true")]
    pub active_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemSubstitutionReason {
    ClientAuthorized,
    InventoryUnavailable,
    ServiceRecovery,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubstitutePickShortageRequest {
    pub policy_id: i64,
    pub expected_policy_revision: Revision,
    pub expected_shortage_revision: Revision,
    pub expected_order_revision: Revision,
    pub reason: ItemSubstitutionReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl<'de> Deserialize<'de> for SubstitutePickShortageRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            policy_id: i64,
            expected_policy_revision: Revision,
            expected_shortage_revision: Revision,
            expected_order_revision: Revision,
            reason: ItemSubstitutionReason,
            #[serde(default)]
            note: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.policy_id <= 0 {
            return Err(serde::de::Error::custom("policy_id must be positive"));
        }
        if raw.note.as_ref().is_some_and(|note| {
            note.is_empty()
                || note.trim() != note
                || note.chars().count() > 500
                || note.chars().any(char::is_control)
        }) {
            return Err(serde::de::Error::custom("note is invalid"));
        }
        if raw.reason == ItemSubstitutionReason::Other && raw.note.is_none() {
            return Err(serde::de::Error::custom("Other requires a note"));
        }
        Ok(Self {
            policy_id: raw.policy_id,
            expected_policy_revision: raw.expected_policy_revision,
            expected_shortage_revision: raw.expected_shortage_revision,
            expected_order_revision: raw.expected_order_revision,
            reason: raw.reason,
            note: raw.note,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubstitutePickWorkResponse {
    pub task_id: i64,
    pub content_id: i64,
    pub inventory_allocation_id: i64,
    pub inventory_balance_id: i64,
    pub source_location_id: i64,
    pub quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubstitutePickShortageResponse {
    pub substitution_id: i64,
    pub shortage_id: i64,
    pub shortage_revision: Revision,
    pub policy_id: i64,
    pub policy_revision: Revision,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub order_id: i64,
    pub order_revision: Revision,
    pub source_order_line_id: i64,
    pub substitute_order_line_id: i64,
    pub substitute_reservation_id: i64,
    pub accepted_source_quantity: i64,
    pub substitute_quantity: i64,
    pub substitute_item_id: i64,
    pub substitute_uom: String,
    pub work: Vec<SubstitutePickWorkResponse>,
    pub reason: ItemSubstitutionReason,
    pub note: Option<String>,
    pub substituted_by: i64,
    pub substituted_at: String,
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn policy_request_is_strict_and_server_scoped() {
        let value = json!({
            "inventory_owner_id": 2,
            "facility_id": 3,
            "source_item_id": 4,
            "source_uom": "case",
            "substitute_item_id": 5,
            "substitute_uom": "each",
            "source_quantity": 1,
            "substitute_quantity": 12
        });
        assert!(serde_json::from_value::<ConfigureItemSubstitutionPolicyRequest>(value).is_ok());
        assert!(
            serde_json::from_value::<ConfigureItemSubstitutionPolicyRequest>(json!({
                "tenant_id": 1,
                "inventory_owner_id": 2,
                "facility_id": 3,
                "source_item_id": 4,
                "source_uom": "case",
                "substitute_item_id": 5,
                "substitute_uom": "each",
                "source_quantity": 1,
                "substitute_quantity": 12
            }))
            .is_err()
        );
    }

    #[test]
    fn substitution_request_is_strict_and_requires_valid_other_evidence() {
        let request = json!({
            "policy_id": 7,
            "expected_policy_revision": 2,
            "expected_shortage_revision": 3,
            "expected_order_revision": 4,
            "reason": "service_recovery"
        });
        assert!(serde_json::from_value::<SubstitutePickShortageRequest>(request).is_ok());
        assert!(
            serde_json::from_value::<SubstitutePickShortageRequest>(json!({
                "policy_id": 7,
                "expected_policy_revision": 2,
                "expected_shortage_revision": 3,
                "expected_order_revision": 4,
                "reason": "other"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SubstitutePickShortageRequest>(json!({
                "policy_id": 7,
                "expected_policy_revision": 2,
                "expected_shortage_revision": 3,
                "expected_order_revision": 4,
                "reason": "client_authorized",
                "accepted_quantity": 2
            }))
            .is_err()
        );
    }
}
