//! Versioned owner/facility item traceability-policy commands and reads.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryOwnerId, ItemTraceabilityPolicyDefinition,
    ItemTraceabilityPolicyId, ItemTraceabilityPolicyRevision, ItemTraceabilityPolicyStatus,
    Timestamp, TraceabilityRequirement, UserId,
};

pub const CONFIGURE_ITEM_TRACEABILITY_POLICY_OPERATION: &str =
    "inventory.item_traceability_policy.configure.v1";
pub const RETIRE_ITEM_TRACEABILITY_POLICY_OPERATION: &str =
    "inventory.item_traceability_policy.retire.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureItemTraceabilityPolicyCommand {
    pub definition: ItemTraceabilityPolicyDefinition,
    pub expected_revision: Option<ItemTraceabilityPolicyRevision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RetireItemTraceabilityPolicyCommand {
    pub item_traceability_policy_id: ItemTraceabilityPolicyId,
    pub expected_revision: ItemTraceabilityPolicyRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemTraceabilityPolicyReadModel {
    pub item_traceability_policy_id: ItemTraceabilityPolicyId,
    pub inventory_owner_name: String,
    pub facility_name: String,
    pub item_description: String,
    pub definition: ItemTraceabilityPolicyDefinition,
    pub status: ItemTraceabilityPolicyStatus,
    pub revision: ItemTraceabilityPolicyRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
    pub retired_by: Option<UserId>,
    pub retired_at: Option<Timestamp>,
}

pub type ConfigureItemTraceabilityPolicyResult = ItemTraceabilityPolicyReadModel;
pub type RetireItemTraceabilityPolicyResult = ItemTraceabilityPolicyReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemTraceabilityPolicyCursor {
    pub after_item_traceability_policy_id: ItemTraceabilityPolicyId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemTraceabilityPolicyPageQuery {
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub item_id: Option<CatalogItemId>,
    pub lot: Option<TraceabilityRequirement>,
    pub serial: Option<TraceabilityRequirement>,
    pub expiration: Option<TraceabilityRequirement>,
    pub status: Option<ItemTraceabilityPolicyStatus>,
    pub cursor: Option<ItemTraceabilityPolicyCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemTraceabilityPolicyPage {
    pub items: Vec<ItemTraceabilityPolicyReadModel>,
    pub next_cursor: Option<ItemTraceabilityPolicyCursor>,
}
