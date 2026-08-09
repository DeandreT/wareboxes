use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::Revision;

const MAX_SCAN_LENGTH: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundQaRequirement {
    NotRequired,
    ScanEveryCarton,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundQaSessionStatus {
    Open,
    Passed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureOutboundQaPolicyRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub requirement: OutboundQaRequirement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundQaPolicyResponse {
    pub policy_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub requirement: OutboundQaRequirement,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartOutboundQaRequest {
    pub expected_order_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifyOutboundQaCartonRequest {
    pub expected_revision: Revision,
    pub carton_barcode: String,
}

impl<'de> Deserialize<'de> for VerifyOutboundQaCartonRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            expected_revision: Revision,
            carton_barcode: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.carton_barcode.is_empty()
            || raw.carton_barcode.trim() != raw.carton_barcode
            || raw.carton_barcode.chars().count() > MAX_SCAN_LENGTH
            || raw.carton_barcode.chars().any(char::is_control)
        {
            return Err(D::Error::custom("carton_barcode is invalid"));
        }
        Ok(Self {
            expected_revision: raw.expected_revision,
            carton_barcode: raw.carton_barcode,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteOutboundQaRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundQaProgressResponse {
    pub expected_carton_count: i64,
    pub verified_carton_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundQaSessionSummaryResponse {
    pub session_id: i64,
    pub policy_id: i64,
    pub policy_revision: Revision,
    pub status: OutboundQaSessionStatus,
    pub revision: Revision,
    pub progress: OutboundQaProgressResponse,
    pub started_at: String,
    pub passed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundQaCartonResponse {
    pub verification_id: i64,
    pub carton_id: i64,
    pub license_plate_id: i64,
    pub sequence: i64,
    pub carton_barcode: String,
    pub content_count: i64,
    pub packed_quantity: i64,
    pub verified_by: i64,
    pub verified_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundQaSessionResponse {
    pub session_id: i64,
    pub packing_session_id: i64,
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub policy_id: i64,
    pub policy_revision: Revision,
    pub status: OutboundQaSessionStatus,
    pub revision: Revision,
    pub progress: OutboundQaProgressResponse,
    pub started_by: i64,
    pub started_at: String,
    pub passed_by: Option<i64>,
    pub passed_at: Option<String>,
    pub verifications: Vec<OutboundQaCartonResponse>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn policy_and_scan_requests_are_strict() {
        assert!(
            serde_json::from_value::<ConfigureOutboundQaPolicyRequest>(json!({
                "inventory_owner_id": 2,
                "facility_id": 3,
                "requirement": "scan_every_carton"
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<ConfigureOutboundQaPolicyRequest>(json!({
                "inventory_owner_id": 2,
                "facility_id": 3,
                "requirement": "scan_every_carton",
                "extra": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<VerifyOutboundQaCartonRequest>(json!({
                "expected_revision": 1,
                "carton_barcode": " CARTON "
            }))
            .is_err()
        );
    }
}
