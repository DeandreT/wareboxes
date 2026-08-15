//! Replay-safe 3PL billing contracts, operational events, reconciliation, and export.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    BillableEventId, BillableEventType, BillingChargeId, BillingContractId, BillingContractNumber,
    BillingContractStatus, BillingEffectiveWindow, BillingFinancialExportId, BillingQuantity,
    BillingRateDefinition, BillingRateId, BillingReconciliationRunId, BillingRunStatus,
    BillingStorageSnapshotId, BillingUnit, CurrencyCode, FacilityId, InventoryOwnerId, Timestamp,
    UserId,
};

pub const CREATE_BILLING_CONTRACT_OPERATION: &str = "billing.contract.create.v1";
pub const ACTIVATE_BILLING_CONTRACT_OPERATION: &str = "billing.contract.activate.v1";
pub const CLOSE_BILLING_CONTRACT_OPERATION: &str = "billing.contract.close.v1";
pub const CONFIGURE_BILLING_RATE_OPERATION: &str = "billing.rate.configure.v1";
pub const CAPTURE_BILLABLE_EVENT_OPERATION: &str = "billing.event.capture.v1";
pub const CAPTURE_STORAGE_SNAPSHOT_OPERATION: &str = "billing.storage_snapshot.capture.v1";
pub const GENERATE_BILLING_RUN_OPERATION: &str = "billing.reconciliation.generate.v1";
pub const REVIEW_BILLING_RUN_OPERATION: &str = "billing.reconciliation.review.v1";
pub const EXPORT_BILLING_RUN_OPERATION: &str = "billing.financial_export.create.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateBillingContractCommand {
    pub inventory_owner_id: InventoryOwnerId,
    pub contract_number: BillingContractNumber,
    pub currency: CurrencyCode,
    pub effective_window: BillingEffectiveWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BillingContractLifecycleCommand {
    pub contract_id: BillingContractId,
    pub expected_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureBillingRateCommand {
    pub contract_id: BillingContractId,
    pub definition: BillingRateDefinition,
    pub effective_window: BillingEffectiveWindow,
    pub expected_revision: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaptureBillableEventCommand {
    pub contract_id: BillingContractId,
    pub facility_id: FacilityId,
    pub event_type: BillableEventType,
    pub unit: BillingUnit,
    pub quantity: BillingQuantity,
    pub source_reference: String,
    pub description: String,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CaptureStorageSnapshotCommand {
    pub contract_id: BillingContractId,
    pub facility_id: FacilityId,
    pub snapshot_date: chrono::NaiveDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GenerateBillingRunCommand {
    pub contract_id: BillingContractId,
    pub facility_id: Option<FacilityId>,
    pub period_from: Timestamp,
    pub period_until: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingReviewDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewBillingRunCommand {
    pub run_id: BillingReconciliationRunId,
    pub expected_revision: i64,
    pub decision: BillingReviewDecision,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportBillingRunCommand {
    pub run_id: BillingReconciliationRunId,
    pub expected_revision: i64,
    pub external_batch_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingContractReadModel {
    pub contract_id: BillingContractId,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub contract_number: String,
    pub currency: String,
    pub effective_window: BillingEffectiveWindow,
    pub status: BillingContractStatus,
    pub revision: i64,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub activated_by: Option<UserId>,
    pub activated_at: Option<Timestamp>,
    pub closed_by: Option<UserId>,
    pub closed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingRateReadModel {
    pub rate_id: BillingRateId,
    pub contract_id: BillingContractId,
    pub inventory_owner_id: InventoryOwnerId,
    pub definition: BillingRateDefinition,
    pub effective_window: BillingEffectiveWindow,
    pub revision: i64,
    pub active: bool,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillableEventReadModel {
    pub event_id: BillableEventId,
    pub contract_id: BillingContractId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub event_type: BillableEventType,
    pub unit: BillingUnit,
    pub quantity: u64,
    pub source_type: String,
    pub source_reference: String,
    pub description: Option<String>,
    pub occurred_at: Timestamp,
    pub captured_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingStorageSnapshotReadModel {
    pub snapshot_id: BillingStorageSnapshotId,
    pub contract_id: BillingContractId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub snapshot_date: chrono::NaiveDate,
    pub pallet_count: i64,
    pub unit_count: i64,
    pub captured_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingChargeReadModel {
    pub charge_id: BillingChargeId,
    pub event_id: BillableEventId,
    pub rate_id: BillingRateId,
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
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingRunReadModel {
    pub run_id: BillingReconciliationRunId,
    pub contract_id: BillingContractId,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub contract_number: String,
    pub facility_id: Option<FacilityId>,
    pub attempt: i64,
    pub supersedes_run_id: Option<BillingReconciliationRunId>,
    pub period_from: Timestamp,
    pub period_until: Timestamp,
    pub status: BillingRunStatus,
    pub revision: i64,
    pub event_count: i64,
    pub charge_count: i64,
    pub unmatched_event_count: i64,
    pub total_minor: u64,
    pub currency: String,
    pub generated_by: UserId,
    pub generated_at: Timestamp,
    pub reviewed_by: Option<UserId>,
    pub reviewed_at: Option<Timestamp>,
    pub review_note: Option<String>,
    pub exported_at: Option<Timestamp>,
    pub charges: Vec<BillingChargeReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingFinancialExportReadModel {
    pub export_id: BillingFinancialExportId,
    pub run_id: BillingReconciliationRunId,
    pub inventory_owner_id: InventoryOwnerId,
    pub external_batch_key: String,
    pub content_sha256: String,
    pub line_count: i64,
    pub total_minor: u64,
    pub currency: String,
    pub csv_content: String,
    pub exported_by: UserId,
    pub exported_at: Timestamp,
    pub resulting_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillingWorkspace {
    pub contracts: Vec<BillingContractReadModel>,
    pub rates: Vec<BillingRateReadModel>,
    pub events: Vec<BillableEventReadModel>,
    pub runs: Vec<BillingRunReadModel>,
    pub next_run_id: Option<BillingReconciliationRunId>,
}
