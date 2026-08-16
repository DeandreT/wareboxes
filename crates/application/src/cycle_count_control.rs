//! Application contracts for count-policy configuration and variance review.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CatalogItemId, CycleCountDisposition, CycleCountPolicyId, CycleCountPolicyRevision,
    CycleCountTolerancePolicy, CycleCountVarianceDecisionDetails, CycleCountVarianceDecisionId,
    CycleCountVarianceId, CycleCountVarianceRevision, CycleCountVarianceStatus, FacilityId,
    InventoryBalanceId, InventoryOwnerId, LocationId, Timestamp, UserId,
};

use crate::count_decision_policy::CountDecisionPolicyReadModel;

pub const CONFIGURE_CYCLE_COUNT_POLICY_OPERATION: &str = "cycle_count.policy.configure.v1";
pub const DECIDE_CYCLE_COUNT_VARIANCE_OPERATION: &str = "cycle_count.variance.decide.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureCycleCountPolicyCommand {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub policy: CycleCountTolerancePolicy,
    pub expected_revision: Option<CycleCountPolicyRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureCycleCountPolicyResult {
    pub policy_id: CycleCountPolicyId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub policy: CycleCountTolerancePolicy,
    pub previous_revision: Option<CycleCountPolicyRevision>,
    pub revision: CycleCountPolicyRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleCountPolicyPageQuery {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub after_policy_id: Option<CycleCountPolicyId>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleCountPolicyReadModel {
    pub policy_id: CycleCountPolicyId,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub policy: CycleCountTolerancePolicy,
    pub revision: CycleCountPolicyRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleCountPolicyPage {
    pub items: Vec<CycleCountPolicyReadModel>,
    pub next_after_policy_id: Option<CycleCountPolicyId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleCountVariancePageQuery {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub status: Option<CycleCountVarianceStatus>,
    pub after_variance_id: Option<CycleCountVarianceId>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleCountVarianceStockReadModel {
    pub inventory_balance_id: InventoryBalanceId,
    pub location_id: LocationId,
    pub location_barcode: String,
    pub location_name: Option<String>,
    pub item_id: CatalogItemId,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub license_plate_barcode: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub inventory_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleCountVarianceReadModel {
    pub variance_id: CycleCountVarianceId,
    pub revision: CycleCountVarianceRevision,
    pub status: CycleCountVarianceStatus,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub stock: CycleCountVarianceStockReadModel,
    pub policy_id: CycleCountPolicyId,
    pub policy_revision: CycleCountPolicyRevision,
    pub policy: CycleCountTolerancePolicy,
    pub decision_policy: CountDecisionPolicyReadModel,
    pub latest_task_id: i64,
    pub latest_attempt_sequence: u16,
    pub automatic_recounts_used: u16,
    pub system_quantity: i64,
    pub counted_quantity: i64,
    pub variance_quantity: i64,
    pub allowed_variance_quantity: i64,
    pub inventory_transaction_id: Option<i64>,
    pub created_at: Timestamp,
    pub modified_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleCountVariancePage {
    pub items: Vec<CycleCountVarianceReadModel>,
    pub next_after_variance_id: Option<CycleCountVarianceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecideCycleCountVarianceCommand {
    pub variance_id: CycleCountVarianceId,
    pub expected_revision: CycleCountVarianceRevision,
    pub details: CycleCountVarianceDecisionDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecideCycleCountVarianceResult {
    pub decision_id: CycleCountVarianceDecisionId,
    pub variance_id: CycleCountVarianceId,
    pub previous_status: CycleCountVarianceStatus,
    pub status: CycleCountVarianceStatus,
    pub previous_revision: CycleCountVarianceRevision,
    pub revision: CycleCountVarianceRevision,
    pub disposition: CycleCountDisposition,
    pub next_task_id: Option<i64>,
    pub inventory_transaction_id: Option<i64>,
    pub decided_by: UserId,
    pub decided_at: Timestamp,
}
