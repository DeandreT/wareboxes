use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::Revision;

const MAX_SCAN_LENGTH: usize = 200;
const MAX_CANCELLATION_NOTE_LENGTH: usize = 500;

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
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundQaCancellationReason {
    PackingCorrection,
    QualityIssue,
    PolicyError,
    OperatorError,
    Other,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelOutboundQaRequest {
    pub expected_revision: Revision,
    pub reason: OutboundQaCancellationReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl<'de> Deserialize<'de> for CancelOutboundQaRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            expected_revision: Revision,
            reason: OutboundQaCancellationReason,
            #[serde(default)]
            note: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.note.as_ref().is_some_and(|note| {
            note.is_empty()
                || note.trim() != note
                || note.chars().count() > MAX_CANCELLATION_NOTE_LENGTH
                || note.chars().any(char::is_control)
        }) {
            return Err(D::Error::custom("outbound QA cancellation note is invalid"));
        }
        if raw.reason == OutboundQaCancellationReason::Other && raw.note.is_none() {
            return Err(D::Error::custom(
                "outbound QA cancellation reason Other requires a note",
            ));
        }
        Ok(Self {
            expected_revision: raw.expected_revision,
            reason: raw.reason,
            note: raw.note,
        })
    }
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
    pub attempt: i64,
    pub status: OutboundQaSessionStatus,
    pub revision: Revision,
    pub progress: OutboundQaProgressResponse,
    pub started_at: String,
    pub passed_at: Option<String>,
    pub cancelled_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundQaCancellationResponse {
    pub cancellation_id: i64,
    pub previous_status: OutboundQaSessionStatus,
    pub reason: OutboundQaCancellationReason,
    pub note: Option<String>,
    pub cancelled_by: i64,
    pub cancelled_at: String,
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
    pub attempt: i64,
    pub status: OutboundQaSessionStatus,
    pub revision: Revision,
    pub progress: OutboundQaProgressResponse,
    pub started_by: i64,
    pub started_at: String,
    pub passed_by: Option<i64>,
    pub passed_at: Option<String>,
    pub cancellation: Option<OutboundQaCancellationResponse>,
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
        assert!(serde_json::from_value::<CancelOutboundQaRequest>(json!({
            "expected_revision": 2,
            "reason": "other"
        }))
        .is_err());
        assert!(serde_json::from_value::<CancelOutboundQaRequest>(json!({
            "expected_revision": 2,
            "reason": "packing_correction",
            "unknown": true
        }))
        .is_err());
    }
}
