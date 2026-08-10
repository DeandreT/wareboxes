//! Versioned owner/facility item storage-policy commands and read contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryOwnerId, ItemStoragePolicyDefinition, ItemStoragePolicyId,
    ItemStoragePolicyRevision, ItemStoragePolicyStatus, StorageZonePurpose, Timestamp, UserId,
};

pub const CONFIGURE_ITEM_STORAGE_POLICY_OPERATION: &str =
    "topology.item_storage_policy.configure.v1";
pub const RETIRE_ITEM_STORAGE_POLICY_OPERATION: &str = "topology.item_storage_policy.retire.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureItemStoragePolicyCommand {
    pub definition: ItemStoragePolicyDefinition,
    pub expected_revision: Option<ItemStoragePolicyRevision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RetireItemStoragePolicyCommand {
    pub item_storage_policy_id: ItemStoragePolicyId,
    pub expected_revision: ItemStoragePolicyRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStoragePolicyReadModel {
    pub item_storage_policy_id: ItemStoragePolicyId,
    pub inventory_owner_name: String,
    pub facility_name: String,
    pub item_description: String,
    pub definition: ItemStoragePolicyDefinition,
    pub status: ItemStoragePolicyStatus,
    pub revision: ItemStoragePolicyRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
    pub retired_by: Option<UserId>,
    pub retired_at: Option<Timestamp>,
}

pub type ConfigureItemStoragePolicyResult = ItemStoragePolicyReadModel;
pub type RetireItemStoragePolicyResult = ItemStoragePolicyReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemStoragePolicyCursor {
    pub after_item_storage_policy_id: ItemStoragePolicyId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemStoragePolicyPageQuery {
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub item_id: Option<CatalogItemId>,
    pub purpose: Option<StorageZonePurpose>,
    pub status: Option<ItemStoragePolicyStatus>,
    pub cursor: Option<ItemStoragePolicyCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStoragePolicyPage {
    pub items: Vec<ItemStoragePolicyReadModel>,
    pub next_cursor: Option<ItemStoragePolicyCursor>,
}
