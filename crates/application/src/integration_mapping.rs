//! Versioned partner order item mapping commands and read contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CatalogItemId, IntegrationOrderItemMappingDefinition, IntegrationOrderItemMappingId,
    IntegrationOrderItemMappingRevision, IntegrationOrderItemMappingStatus, InventoryOwnerId,
    Timestamp, UserId,
};

pub const CONFIGURE_INTEGRATION_ORDER_ITEM_MAPPING_OPERATION: &str =
    "integration.order_item_mapping.configure.v1";
pub const RETIRE_INTEGRATION_ORDER_ITEM_MAPPING_OPERATION: &str =
    "integration.order_item_mapping.retire.v1";

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
