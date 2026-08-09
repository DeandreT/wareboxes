use serde::{Deserialize, Serialize};

use super::Revision;

pub const MAX_BACKORDER_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackorderPolicyMode {
    Block,
    SplitShortage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackorderReason {
    InventoryUnavailable,
    ClientRequested,
    ServiceLevel,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureBackorderPolicyRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub mode: BackorderPolicyMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackorderPolicyRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackorderPolicyResponse {
    pub policy_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub mode: BackorderPolicyMode,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SplitOrderBackorderRequest {
    pub facility_id: i64,
    pub expected_order_revision: Revision,
    pub expected_policy_revision: Revision,
    pub reason: BackorderReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl<'de> Deserialize<'de> for SplitOrderBackorderRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            facility_id: i64,
            expected_order_revision: Revision,
            expected_policy_revision: Revision,
            reason: BackorderReason,
            #[serde(default)]
            note: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.facility_id <= 0 {
            return Err(serde::de::Error::custom("facility_id must be positive"));
        }
        if raw.note.as_ref().is_some_and(|note| {
            note.is_empty()
                || note.trim() != note
                || note.chars().count() > MAX_BACKORDER_NOTE_LENGTH
                || note.chars().any(char::is_control)
        }) {
            return Err(serde::de::Error::custom("note is invalid"));
        }
        if raw.reason == BackorderReason::Other && raw.note.is_none() {
            return Err(serde::de::Error::custom("Other requires a note"));
        }
        Ok(Self {
            facility_id: raw.facility_id,
            expected_order_revision: raw.expected_order_revision,
            expected_policy_revision: raw.expected_policy_revision,
            reason: raw.reason,
            note: raw.note,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackorderSplitLineResponse {
    pub parent_order_line_id: i64,
    pub child_order_line_id: i64,
    pub line_key: String,
    pub item_id: i64,
    pub uom: String,
    pub original_quantity: i64,
    pub allocated_quantity: i64,
    pub previously_backordered_quantity: i64,
    pub newly_backordered_quantity: i64,
    pub resulting_parent_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitOrderBackorderResponse {
    pub split_id: i64,
    pub policy_id: i64,
    pub policy_revision: Revision,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub parent_order_id: i64,
    pub parent_order_key: String,
    pub parent_revision: Revision,
    pub child_order_id: i64,
    pub child_order_key: String,
    pub child_revision: Revision,
    pub original_quantity: i64,
    pub allocated_quantity: i64,
    pub previously_backordered_quantity: i64,
    pub newly_backordered_quantity: i64,
    pub parent_effective_quantity: i64,
    pub lines: Vec<BackorderSplitLineResponse>,
    pub reason: BackorderReason,
    pub note: Option<String>,
    pub split_by: i64,
    pub split_at: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn requests_are_strict_revisioned_and_server_derive_quantities() {
        let request = serde_json::from_value::<SplitOrderBackorderRequest>(json!({
            "facility_id": 4,
            "expected_order_revision": 3,
            "expected_policy_revision": 2,
            "reason": "inventory_unavailable"
        }))
        .unwrap();
        assert_eq!(request.facility_id, 4);
        assert!(serde_json::from_value::<SplitOrderBackorderRequest>(json!({
            "facility_id": 4,
            "expected_order_revision": 3,
            "expected_policy_revision": 2,
            "reason": "inventory_unavailable",
            "quantity": 7
        }))
        .is_err());
        assert!(serde_json::from_value::<SplitOrderBackorderRequest>(json!({
            "facility_id": 4,
            "expected_order_revision": 3,
            "expected_policy_revision": 2,
            "reason": "other"
        }))
        .is_err());
    }

    #[test]
    fn policy_request_has_no_tenant_identity() {
        assert!(
            serde_json::from_value::<ConfigureBackorderPolicyRequest>(json!({
                "inventory_owner_id": 2,
                "facility_id": 3,
                "mode": "split_shortage"
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<ConfigureBackorderPolicyRequest>(json!({
                "tenant_id": 1,
                "inventory_owner_id": 2,
                "facility_id": 3,
                "mode": "split_shortage"
            }))
            .is_err()
        );
    }
}
