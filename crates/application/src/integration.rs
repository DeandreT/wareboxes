use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    FacilityId, IntegrationInboxProcessingAttemptId, IntegrationInboxProcessingId,
    IntegrationInboxProcessingRevision, IntegrationInboxProcessingStatus, InventoryOwnerId,
    OrderId, OrderRevision, TenantId, Timestamp, UserId,
};

pub const STANDARD_ORDER_INTAKE_ADAPTER: &str = "wareboxes.fulfillment_order";
pub const STANDARD_ORDER_INTAKE_MAPPING_VERSION: i32 = 1;
pub const REPROCESS_INTEGRATION_ORDER_OPERATION: &str = "integration.order_intake.reprocess.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReprocessIntegrationOrderCommand {
    receipt_id: i64,
    expected_revision: IntegrationInboxProcessingRevision,
}

impl ReprocessIntegrationOrderCommand {
    pub fn new(
        receipt_id: i64,
        expected_revision: IntegrationInboxProcessingRevision,
    ) -> crate::ApplicationResult<Self> {
        if receipt_id <= 0 {
            return Err(crate::ApplicationError::InvalidRequest(
                "integration inbox receipt ID must be positive".into(),
            ));
        }
        Ok(Self {
            receipt_id,
            expected_revision,
        })
    }

    pub const fn receipt_id(&self) -> i64 {
        self.receipt_id
    }

    pub const fn expected_revision(&self) -> IntegrationInboxProcessingRevision {
        self.expected_revision
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationOrderProcessingResult {
    pub receipt_id: i64,
    pub processing_id: IntegrationInboxProcessingId,
    pub processing_attempt_id: IntegrationInboxProcessingAttemptId,
    pub inventory_owner_id: InventoryOwnerId,
    pub adapter_key: String,
    pub mapping_version: i32,
    pub status: IntegrationInboxProcessingStatus,
    pub revision: IntegrationInboxProcessingRevision,
    pub attempt_count: i32,
    pub order_id: Option<OrderId>,
    pub order_revision: Option<OrderRevision>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_by: UserId,
    pub attempted_at: Timestamp,
    pub processed_at: Option<Timestamp>,
}

pub struct NewIntegrationInboxReceipt<'a> {
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub source_key: &'a str,
    pub deduplication_key: &'a str,
    pub content_type: &'a str,
    pub raw_payload: &'a [u8],
    pub request_id: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegrationInboxReadScope {
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationInboxReceipt {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub received_at: Timestamp,
    pub source_key: String,
    pub deduplication_key: String,
    pub content_type: String,
    pub raw_payload: Vec<u8>,
    pub payload_sha256: Vec<u8>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveIntegrationInboxResult {
    pub receipt: IntegrationInboxReceipt,
    pub replayed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reprocess_command_requires_positive_receipt_and_revision() {
        let revision = IntegrationInboxProcessingRevision::new(3).unwrap();
        let command = ReprocessIntegrationOrderCommand::new(17, revision).unwrap();
        assert_eq!(command.receipt_id(), 17);
        assert_eq!(command.expected_revision(), revision);
        assert!(ReprocessIntegrationOrderCommand::new(0, revision).is_err());
    }
}
