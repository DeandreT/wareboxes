//! Versioned decision-table configuration commands, queries, and operator projections.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    ConfigurationEffectiveWindow, ConfigurationScope, ConfigurationStatus, ConfigurationVersionId,
    DecisionRuleDefinition, DecisionRuleKind, FacilityId, InventoryOwnerId, Timestamp, UserId,
};

pub const CREATE_CONFIGURATION_OPERATION: &str = "configuration.version.create.v1";
pub const SUBMIT_CONFIGURATION_OPERATION: &str = "configuration.version.submit.v1";
pub const APPROVE_CONFIGURATION_OPERATION: &str = "configuration.version.approve.v1";
pub const ACTIVATE_CONFIGURATION_OPERATION: &str = "configuration.version.activate.v1";
pub const RETIRE_CONFIGURATION_OPERATION: &str = "configuration.version.retire.v1";
pub const ROLLBACK_CONFIGURATION_OPERATION: &str = "configuration.version.rollback.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateConfigurationCommand {
    pub scope: ConfigurationScope,
    pub effective_window: ConfigurationEffectiveWindow,
    pub definition: DecisionRuleDefinition,
    pub expected_revision: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ConfigurationLifecycleCommand {
    pub configuration_id: ConfigurationVersionId,
    pub expected_revision: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ActivateConfigurationCommand {
    pub configuration_id: ConfigurationVersionId,
    pub expected_revision: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RollbackConfigurationCommand {
    pub source_configuration_id: ConfigurationVersionId,
    pub expected_source_revision: i64,
    pub effective_window: ConfigurationEffectiveWindow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigurationReadModel {
    pub configuration_id: ConfigurationVersionId,
    pub revision: i64,
    pub scope: ConfigurationScope,
    pub status: ConfigurationStatus,
    pub effective_window: ConfigurationEffectiveWindow,
    pub definition: DecisionRuleDefinition,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub submitted_by: Option<UserId>,
    pub submitted_at: Option<Timestamp>,
    pub approved_by: Option<UserId>,
    pub approved_at: Option<Timestamp>,
    pub activated_by: Option<UserId>,
    pub activated_at: Option<Timestamp>,
    pub retired_by: Option<UserId>,
    pub retired_at: Option<Timestamp>,
    pub rollback_of_configuration_id: Option<ConfigurationVersionId>,
}

pub type CreateConfigurationResult = ConfigurationReadModel;
pub type SubmitConfigurationResult = ConfigurationReadModel;
pub type ApproveConfigurationResult = ConfigurationReadModel;
pub type ActivateConfigurationResult = ConfigurationReadModel;
pub type RetireConfigurationResult = ConfigurationReadModel;
pub type RollbackConfigurationResult = ConfigurationReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigurationCursor {
    pub after_configuration_id: ConfigurationVersionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigurationPageQuery {
    pub kind: Option<DecisionRuleKind>,
    pub status: Option<ConfigurationStatus>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub cursor: Option<ConfigurationCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationPage {
    pub items: Vec<ConfigurationReadModel>,
    pub next_cursor: Option<ConfigurationCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimulateConfigurationQuery {
    pub kind: DecisionRuleKind,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub effective_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigurationSimulationResult {
    pub kind: DecisionRuleKind,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub effective_at: Timestamp,
    pub matched_configuration: Option<ConfigurationReadModel>,
    pub evaluated_candidate_count: u32,
}

#[cfg(test)]
mod tests {
    use wareboxes_domain::{InventoryRotation, TenantId};

    use super::*;

    #[test]
    fn create_command_contains_no_tenant_identity_and_retains_typed_definition() {
        let command = CreateConfigurationCommand {
            scope: ConfigurationScope::OwnerFacility {
                inventory_owner_id: InventoryOwnerId::new(2).unwrap(),
                facility_id: FacilityId::new(3).unwrap(),
            },
            effective_window: ConfigurationEffectiveWindow::new(
                "2026-09-01T00:00:00Z".parse().unwrap(),
                None,
            )
            .unwrap(),
            definition: DecisionRuleDefinition::Allocation {
                rotation: InventoryRotation::Fefo,
                allow_partial: true,
                require_complete_line: false,
            },
            expected_revision: None,
        };
        assert_eq!(command.definition.kind(), DecisionRuleKind::Allocation);
        let serialized = serde_json::to_value(command).unwrap();
        assert!(serialized.get("tenant_id").is_none());
        let _tenant_type_remains_separate = TenantId::new(1).unwrap();
    }
}
