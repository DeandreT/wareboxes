//! Versioned item-substitution policy and supervisor execution contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    FacilityId, InventoryAllocationId, InventoryBalanceId, InventoryOwnerId,
    ItemSubstitutionDefinition, ItemSubstitutionDetails, ItemSubstitutionId,
    ItemSubstitutionPolicyId, ItemSubstitutionPolicyRevision, LocationId, OrderId, OrderLineId,
    OrderRevision, OrderStatus, PickContentId, PickShortageId, PickShortageRevision, PickTaskId,
    SubstitutionQuantity, Timestamp, UserId,
};

pub const CONFIGURE_ITEM_SUBSTITUTION_POLICY_OPERATION: &str =
    "outbound.item_substitution.policy.configure.v1";
pub const RETIRE_ITEM_SUBSTITUTION_POLICY_OPERATION: &str =
    "outbound.item_substitution.policy.retire.v1";
pub const SUBSTITUTE_PICK_SHORTAGE_OPERATION: &str = "picking.shortage.substitute.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureItemSubstitutionPolicyCommand {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub definition: ItemSubstitutionDefinition,
    pub expected_revision: Option<ItemSubstitutionPolicyRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetireItemSubstitutionPolicyCommand {
    pub policy_id: ItemSubstitutionPolicyId,
    pub expected_revision: ItemSubstitutionPolicyRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSubstitutionPolicyReadModel {
    pub policy_id: ItemSubstitutionPolicyId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub definition: ItemSubstitutionDefinition,
    pub revision: ItemSubstitutionPolicyRevision,
    pub active: bool,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
    pub retired_by: Option<UserId>,
    pub retired_at: Option<Timestamp>,
}

pub type ConfigureItemSubstitutionPolicyResult = ItemSubstitutionPolicyReadModel;
pub type RetireItemSubstitutionPolicyResult = ItemSubstitutionPolicyReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSubstitutionPolicyFilter {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub source_item_id: Option<i64>,
    pub active_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubstitutePickShortageCommand {
    pub shortage_id: PickShortageId,
    pub policy_id: ItemSubstitutionPolicyId,
    pub expected_policy_revision: ItemSubstitutionPolicyRevision,
    pub expected_shortage_revision: PickShortageRevision,
    pub expected_order_revision: OrderRevision,
    pub details: ItemSubstitutionDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubstitutePickWorkReadModel {
    pub task_id: PickTaskId,
    pub content_id: PickContentId,
    pub inventory_allocation_id: InventoryAllocationId,
    pub inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub quantity: SubstitutionQuantity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubstitutePickShortageResult {
    pub substitution_id: ItemSubstitutionId,
    pub shortage_id: PickShortageId,
    pub shortage_revision: PickShortageRevision,
    pub policy_id: ItemSubstitutionPolicyId,
    pub policy_revision: ItemSubstitutionPolicyRevision,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub order_id: OrderId,
    pub order_revision: OrderRevision,
    pub order_status: OrderStatus,
    pub source_order_line_id: OrderLineId,
    pub substitute_order_line_id: OrderLineId,
    pub substitute_reservation_id: i64,
    pub accepted_source_quantity: SubstitutionQuantity,
    pub substitute_quantity: SubstitutionQuantity,
    pub substitute_item_id: i64,
    pub substitute_uom: String,
    pub work: Vec<SubstitutePickWorkReadModel>,
    pub details: ItemSubstitutionDetails,
    pub substituted_by: UserId,
    pub substituted_at: Timestamp,
}
