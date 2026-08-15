use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::{BillableEventType, BillingUnit, OpaqueCursor, PageLimit, Revision};

pub const MAX_BILLING_NOTE_LENGTH: usize = 500;
pub const MAX_BILLING_REFERENCE_LENGTH: usize = 160;
pub const MAX_BILLING_BATCH_KEY_LENGTH: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingContractStatus {
    Draft,
    Active,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingRunStatus {
    PendingReview,
    Approved,
    Rejected,
    Exported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingReviewDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateBillingContractRequest {
    pub inventory_owner_id: i64,
    pub contract_number: String,
    pub currency: String,
    pub effective_from: String,
    pub effective_until: Option<String>,
}

impl<'de> Deserialize<'de> for CreateBillingContractRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            inventory_owner_id: i64,
            contract_number: String,
            currency: String,
            effective_from: String,
            #[serde(default)]
            effective_until: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.inventory_owner_id <= 0
            || raw.contract_number.trim().is_empty()
            || raw.contract_number.len() > 80
            || raw.currency.len() != 3
            || !raw.currency.bytes().all(|byte| byte.is_ascii_alphabetic())
            || raw.effective_from.trim().is_empty()
        {
            return Err(D::Error::custom("invalid billing contract"));
        }
        Ok(Self {
            inventory_owner_id: raw.inventory_owner_id,
            contract_number: raw.contract_number,
            currency: raw.currency,
            effective_from: raw.effective_from,
            effective_until: raw.effective_until,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingLifecycleRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureBillingRateRequest {
    pub event_type: BillableEventType,
    pub unit: BillingUnit,
    pub currency: String,
    pub rate_minor: u64,
    pub minimum_charge_minor: u64,
    pub effective_from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureBillableEventRequest {
    pub facility_id: i64,
    pub event_type: BillableEventType,
    pub unit: BillingUnit,
    pub quantity: i64,
    pub source_reference: String,
    pub description: String,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureBillingStorageSnapshotRequest {
    pub facility_id: i64,
    pub snapshot_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateBillingRunRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    pub period_from: String,
    pub period_until: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewBillingRunRequest {
    pub expected_revision: Revision,
    pub decision: BillingReviewDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportBillingRunRequest {
    pub expected_revision: Revision,
    pub external_batch_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingContractResponse {
    pub contract_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub contract_number: String,
    pub currency: String,
    pub effective_from: String,
    pub effective_until: Option<String>,
    pub status: BillingContractStatus,
    pub revision: Revision,
    pub created_by: i64,
    pub created_at: String,
    pub activated_by: Option<i64>,
    pub activated_at: Option<String>,
    pub closed_by: Option<i64>,
    pub closed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingRateResponse {
    pub rate_id: i64,
    pub contract_id: i64,
    pub inventory_owner_id: i64,
    pub event_type: BillableEventType,
    pub unit: BillingUnit,
    pub currency: String,
    pub rate_minor: u64,
    pub minimum_charge_minor: u64,
    pub effective_from: String,
    pub effective_until: Option<String>,
    pub revision: Revision,
    pub active: bool,
    pub created_by: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillableEventResponse {
    pub event_id: i64,
    pub contract_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub event_type: BillableEventType,
    pub unit: BillingUnit,
    pub quantity: u64,
    pub source_type: String,
    pub source_reference: String,
    pub description: Option<String>,
    pub occurred_at: String,
    pub captured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingStorageSnapshotResponse {
    pub snapshot_id: i64,
    pub contract_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub snapshot_date: String,
    pub pallet_count: i64,
    pub unit_count: i64,
    pub captured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingChargeResponse {
    pub charge_id: i64,
    pub event_id: i64,
    pub rate_id: i64,
    pub event_type: BillableEventType,
    pub unit: BillingUnit,
    pub quantity: u64,
    pub rate_minor: u64,
    pub minimum_charge_minor: u64,
    pub gross_minor: u64,
    pub amount_minor: u64,
    pub currency: String,
    pub source_type: String,
    pub source_reference: String,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingRunResponse {
    pub run_id: i64,
    pub contract_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub contract_number: String,
    pub facility_id: Option<i64>,
    pub attempt: i64,
    pub supersedes_run_id: Option<i64>,
    pub period_from: String,
    pub period_until: String,
    pub status: BillingRunStatus,
    pub revision: Revision,
    pub event_count: i64,
    pub charge_count: i64,
    pub unmatched_event_count: i64,
    pub total_minor: u64,
    pub currency: String,
    pub generated_by: i64,
    pub generated_at: String,
    pub reviewed_by: Option<i64>,
    pub reviewed_at: Option<String>,
    pub review_note: Option<String>,
    pub exported_at: Option<String>,
    pub charges: Vec<BillingChargeResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingFinancialExportResponse {
    pub export_id: i64,
    pub run_id: i64,
    pub inventory_owner_id: i64,
    pub external_batch_key: String,
    pub content_sha256: String,
    pub line_count: i64,
    pub total_minor: u64,
    pub currency: String,
    pub csv_content: String,
    pub exported_by: i64,
    pub exported_at: String,
    pub resulting_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingWorkspaceResponse {
    pub contracts: Vec<BillingContractResponse>,
    pub rates: Vec<BillingRateResponse>,
    pub events: Vec<BillableEventResponse>,
    pub runs: Vec<BillingRunResponse>,
    pub next_cursor: Option<OpaqueCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BillingPageRequest {
    #[serde(default)]
    pub inventory_owner_id: Option<i64>,
    #[serde(default)]
    pub contract_id: Option<i64>,
    #[serde(default)]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}
