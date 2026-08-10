//! Versioned partner order item mapping commands and read contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CatalogItemId, IntegrationOrderItemMappingDefinition, IntegrationOrderItemMappingId,
    IntegrationOrderItemMappingRevision, IntegrationOrderItemMappingStatus,
    IntegrationOrderOwnerMappingDefinition, IntegrationOrderOwnerMappingId,
    IntegrationOrderOwnerMappingRevision, IntegrationOrderOwnerMappingStatus, InventoryOwnerId,
    Timestamp, UserId,
};

pub const CONFIGURE_INTEGRATION_ORDER_ITEM_MAPPING_OPERATION: &str =
    "integration.order_item_mapping.configure.v1";
pub const RETIRE_INTEGRATION_ORDER_ITEM_MAPPING_OPERATION: &str =
    "integration.order_item_mapping.retire.v1";
pub const CONFIGURE_INTEGRATION_ORDER_OWNER_MAPPING_OPERATION: &str =
    "integration.order_owner_mapping.configure.v1";
pub const RETIRE_INTEGRATION_ORDER_OWNER_MAPPING_OPERATION: &str =
    "integration.order_owner_mapping.retire.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureIntegrationOrderItemMappingCommand {
    pub definition: IntegrationOrderItemMappingDefinition,
    pub expected_revision: Option<IntegrationOrderItemMappingRevision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RetireIntegrationOrderItemMappingCommand {
    pub mapping_id: IntegrationOrderItemMappingId,
    pub expected_revision: IntegrationOrderItemMappingRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationOrderItemMappingReadModel {
    pub mapping_id: IntegrationOrderItemMappingId,
    pub inventory_owner_name: String,
    pub item_description: String,
    pub definition: IntegrationOrderItemMappingDefinition,
    pub status: IntegrationOrderItemMappingStatus,
    pub revision: IntegrationOrderItemMappingRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
    pub retired_by: Option<UserId>,
    pub retired_at: Option<Timestamp>,
}

pub type ConfigureIntegrationOrderItemMappingResult = IntegrationOrderItemMappingReadModel;
pub type RetireIntegrationOrderItemMappingResult = IntegrationOrderItemMappingReadModel;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureIntegrationOrderOwnerMappingCommand {
    pub definition: IntegrationOrderOwnerMappingDefinition,
    pub expected_revision: Option<IntegrationOrderOwnerMappingRevision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RetireIntegrationOrderOwnerMappingCommand {
    pub mapping_id: IntegrationOrderOwnerMappingId,
    pub expected_revision: IntegrationOrderOwnerMappingRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationOrderOwnerMappingReadModel {
    pub mapping_id: IntegrationOrderOwnerMappingId,
    pub inventory_owner_name: String,
    pub definition: IntegrationOrderOwnerMappingDefinition,
    pub status: IntegrationOrderOwnerMappingStatus,
    pub revision: IntegrationOrderOwnerMappingRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
    pub retired_by: Option<UserId>,
    pub retired_at: Option<Timestamp>,
}

pub type ConfigureIntegrationOrderOwnerMappingResult = IntegrationOrderOwnerMappingReadModel;
pub type RetireIntegrationOrderOwnerMappingResult = IntegrationOrderOwnerMappingReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegrationOrderItemMappingCursor {
    pub after_mapping_id: IntegrationOrderItemMappingId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationOrderItemMappingPageQuery {
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub source_key: Option<String>,
    pub item_id: Option<CatalogItemId>,
    pub status: Option<IntegrationOrderItemMappingStatus>,
    pub cursor: Option<IntegrationOrderItemMappingCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationOrderItemMappingPage {
    pub items: Vec<IntegrationOrderItemMappingReadModel>,
    pub next_cursor: Option<IntegrationOrderItemMappingCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegrationOrderOwnerMappingCursor {
    pub after_mapping_id: IntegrationOrderOwnerMappingId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationOrderOwnerMappingPageQuery {
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub source_key: Option<String>,
    pub status: Option<IntegrationOrderOwnerMappingStatus>,
    pub cursor: Option<IntegrationOrderOwnerMappingCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationOrderOwnerMappingPage {
    pub items: Vec<IntegrationOrderOwnerMappingReadModel>,
    pub next_cursor: Option<IntegrationOrderOwnerMappingCursor>,
}
