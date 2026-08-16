use serde::{Deserialize, Serialize};

use serde::de::Error as _;

use super::{ConfigurationScope, CursorPage, OpaqueCursor, PageLimit, PutawayWorkflow};

pub const PRODUCT_DEFAULT_PUTAWAY_POLICY_HASH: &str =
    "9ebb7234209756a6ff122d74733521612cd2dd38dbb8ed8490e732c9b1625971";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawayPolicySource {
    ProductDefault,
    Configuration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PutawayPolicyExpectation {
    pub source: PutawayPolicySource,
    pub configuration_id: Option<i64>,
    pub configuration_revision: Option<i64>,
    pub policy_hash: String,
}

impl PutawayPolicyExpectation {
    pub fn product_default() -> Self {
        Self {
            source: PutawayPolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            policy_hash: PRODUCT_DEFAULT_PUTAWAY_POLICY_HASH.to_owned(),
        }
    }

    fn validate(&self) -> Result<(), &'static str> {
        let valid_identity = match self.source {
            PutawayPolicySource::ProductDefault => {
                self.configuration_id.is_none() && self.configuration_revision.is_none()
            }
            PutawayPolicySource::Configuration => {
                self.configuration_id.is_some_and(|id| id > 0)
                    && self
                        .configuration_revision
                        .is_some_and(|revision| revision > 0)
            }
        };
        if !valid_identity {
            return Err("putaway policy identity is invalid");
        }
        if self.policy_hash.len() != 64
            || !self
                .policy_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("putaway policy hash must be lowercase SHA-256 hex");
        }
        Ok(())
    }
}

impl Default for PutawayPolicyExpectation {
    fn default() -> Self {
        Self::product_default()
    }
}

impl<'de> Deserialize<'de> for PutawayPolicyExpectation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            source: PutawayPolicySource,
            configuration_id: Option<i64>,
            configuration_revision: Option<i64>,
            policy_hash: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        let value = Self {
            source: raw.source,
            configuration_id: raw.configuration_id,
            configuration_revision: raw.configuration_revision,
            policy_hash: raw.policy_hash,
        };
        value.validate().map_err(D::Error::custom)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayPolicyResponse {
    pub source: PutawayPolicySource,
    pub configuration_id: Option<i64>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub require_zone_compatibility: bool,
    pub enforce_location_capacity: bool,
    pub allow_mixed_lots: bool,
    pub policy_hash: String,
}

impl PutawayPolicyResponse {
    pub fn expectation(&self) -> PutawayPolicyExpectation {
        PutawayPolicyExpectation {
            source: self.source,
            configuration_id: self.configuration_id,
            configuration_revision: self.configuration_revision,
            policy_hash: self.policy_hash.clone(),
        }
    }
}

/// Creates one directed putaway task for loose inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePutawayTaskRequest {
    pub source_inventory_balance_id: i64,
    pub destination_location_id: i64,
    pub quantity: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_user_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_for: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default)]
    pub expected_policy: PutawayPolicyExpectation,
}

/// Identity of a newly created directed putaway task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePutawayTaskResponse {
    pub task_id: i64,
    pub putaway_policy: PutawayPolicyResponse,
}

/// Confirms the scanned destination for a directed putaway task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmPutawayRequest {
    pub destination_location_barcode: String,
    #[serde(default)]
    pub expected_policy: PutawayPolicyExpectation,
}

/// Result of atomically completing a directed loose-inventory putaway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayConfirmationResponse {
    pub task_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub inventory_transaction_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub destination_location_id: i64,
    pub destination_location_barcode: String,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub quantity: i64,
    pub inventory_status: String,
    pub confirmed_by: i64,
    pub confirmed_at: String,
    pub putaway_policy: PutawayPolicyResponse,
}

/// Stable lifecycle grouping used by the supervisor putaway monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawayWorkStatus {
    Pending,
    Claimed,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawayCandidateSort {
    #[default]
    ReceivedAt,
    Client,
    Facility,
    Source,
    Item,
    Quantity,
    Workflow,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawayWorkSort {
    Priority,
    #[default]
    CreatedAt,
    Client,
    Facility,
    Source,
    Destination,
    Quantity,
    Status,
    Workflow,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PutawaySortDirection {
    #[default]
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PutawayCandidatePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<PutawayWorkflow>,
    #[serde(default)]
    pub sort: PutawayCandidateSort,
    #[serde(default)]
    pub direction: PutawaySortDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PutawayWorkPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<PutawayWorkflow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PutawayWorkStatus>,
    #[serde(default)]
    pub sort: PutawayWorkSort,
    #[serde(default)]
    pub direction: PutawaySortDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayLocationResponse {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayCandidateResponse {
    pub workflow: PutawayWorkflow,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub source_inventory_balance_id: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub source_location: PutawayLocationResponse,
    pub item_count: i64,
    pub balance_count: i64,
    pub item_id: Option<i64>,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub uom: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub available_quantity: i64,
    pub received_at: String,
    pub putaway_policy: PutawayPolicyResponse,
}

pub type PutawayCandidatePage = CursorPage<PutawayCandidateResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutawayWorkResponse {
    pub task_id: i64,
    pub workflow: PutawayWorkflow,
    pub status: PutawayWorkStatus,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub source_inventory_balance_id: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub source_location: PutawayLocationResponse,
    pub destination_location: PutawayLocationResponse,
    pub item_count: i64,
    pub balance_count: i64,
    pub item_id: Option<i64>,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub uom: Option<String>,
    pub planned_quantity: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub assigned_user_id: Option<i64>,
    pub lease_expires_at: Option<String>,
    pub due_at: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub putaway_policy: PutawayPolicyResponse,
}

pub type PutawayWorkPage = CursorPage<PutawayWorkResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn putaway_confirmation_requires_only_the_scanned_destination_barcode() {
        assert_eq!(
            serde_json::from_str::<ConfirmPutawayRequest>(
                r#"{"destination_location_barcode":"A-01-01"}"#
            )
            .unwrap(),
            ConfirmPutawayRequest {
                destination_location_barcode: "A-01-01".into(),
                expected_policy: PutawayPolicyExpectation::product_default(),
            }
        );
        assert!(
            serde_json::from_str::<ConfirmPutawayRequest>(r#"{"destination_location_id":42}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<ConfirmPutawayRequest>(
            r#"{"destination_location_barcode":"A-01-01","task_id":4}"#
        )
        .is_err());
    }

    #[test]
    fn manager_page_requests_are_strict_and_sortable() {
        let request = serde_json::from_str::<PutawayCandidatePageRequest>(
            r#"{"facility_id":4,"workflow":"license_plate","sort":"quantity","direction":"desc"}"#,
        )
        .unwrap();
        assert_eq!(request.sort, PutawayCandidateSort::Quantity);
        assert_eq!(request.direction, PutawaySortDirection::Desc);
        assert!(serde_json::from_str::<PutawayWorkPageRequest>(
            r#"{"status":"pending","unknown":true}"#
        )
        .is_err());
    }
}
