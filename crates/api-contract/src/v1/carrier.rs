use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CarrierAccountStatus {
    Active,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CarrierManifestJobStatus {
    Queued,
    Processing,
    RetryScheduled,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCarrierAccountRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub display_name: String,
    pub carrier_code: String,
    pub account_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconfigureCarrierAccountRequest {
    pub display_name: String,
    pub account_key: String,
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeCarrierAccountStatusRequest {
    pub status: CarrierAccountStatus,
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CarrierAccountResponse {
    pub account_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub display_name: String,
    pub carrier_code: String,
    /// Non-secret account identity understood by the deployment carrier gateway.
    pub account_key: String,
    pub status: CarrierAccountStatus,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
    pub updated_by: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CarrierAccountPageRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    #[serde(default)]
    pub include_disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type CarrierAccountPage = CursorPage<CarrierAccountResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueCarrierManifestRequest {
    pub account_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_code: Option<String>,
    pub expected_shipment_revision: Revision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelCarrierManifestRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryCarrierManifestRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CarrierManifestJobResponse {
    pub job_id: i64,
    pub shipment_id: i64,
    pub account_id: i64,
    pub account_revision: Revision,
    pub account_key: String,
    pub carrier_code: String,
    pub service_code: Option<String>,
    pub request_key: String,
    pub request_sha256: String,
    pub status: CarrierManifestJobStatus,
    pub revision: Revision,
    pub attempt_count: u32,
    pub next_attempt_at: Option<String>,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub manifest_id: Option<i64>,
    pub manifest_reference: Option<String>,
    pub requested_by: i64,
    pub requested_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CarrierManifestJobPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type CarrierManifestJobPage = CursorPage<CarrierManifestJobResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_key_is_explicitly_non_secret_in_the_wire_shape() {
        let request: CreateCarrierAccountRequest = serde_json::from_value(serde_json::json!({
            "inventory_owner_id": 1,
            "facility_id": 2,
            "display_name": "Parcel account",
            "carrier_code": "CARRIER",
            "account_key": "external-account-7"
        }))
        .unwrap();
        assert_eq!(request.account_key, "external-account-7");
    }

    #[test]
    fn manifest_job_statuses_are_stable_wire_values() {
        assert_eq!(
            serde_json::to_value(CarrierManifestJobStatus::RetryScheduled).unwrap(),
            serde_json::json!("retry_scheduled")
        );
    }
}
