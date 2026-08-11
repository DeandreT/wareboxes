//! Typed customer-return authorization and inbound-load planning contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CatalogItemId, CustomerReturnCancellationDetails, CustomerReturnCancellationId,
    CustomerReturnCancellationReason, CustomerReturnId, CustomerReturnLineId,
    CustomerReturnLoadPlanDetails, CustomerReturnLoadPlanId, CustomerReturnReason,
    CustomerReturnRevision, CustomerReturnStatus, FacilityId, InboundLoadId, InboundLoadLineId,
    InventoryHoldId, InventoryOwnerId, NewCustomerReturn, Timestamp, UserId,
};

pub const CREATE_CUSTOMER_RETURN_OPERATION: &str = "inbound.customer_return.create.v1";
pub const PLAN_CUSTOMER_RETURN_LOAD_OPERATION: &str = "inbound.customer_return.load.plan.v1";
pub const CANCEL_CUSTOMER_RETURN_OPERATION: &str = "inbound.customer_return.cancel.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCustomerReturnCommand {
    pub authorization: NewCustomerReturn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedCustomerReturnLineResult {
    pub line_id: CustomerReturnLineId,
    pub item_id: CatalogItemId,
    pub authorized_quantity: i64,
    pub reason: CustomerReturnReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCustomerReturnResult {
    pub customer_return_id: CustomerReturnId,
    pub number: String,
    pub status: CustomerReturnStatus,
    pub revision: CustomerReturnRevision,
    pub lines: Vec<CreatedCustomerReturnLineResult>,
    pub total_authorized_quantity: i64,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanCustomerReturnLoadCommand {
    pub customer_return_id: CustomerReturnId,
    pub expected_revision: CustomerReturnRevision,
    pub details: CustomerReturnLoadPlanDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedCustomerReturnLoadLineResult {
    pub customer_return_line_id: CustomerReturnLineId,
    pub load_line_id: InboundLoadLineId,
    pub item_id: CatalogItemId,
    pub authorized_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanCustomerReturnLoadResult {
    pub plan_id: CustomerReturnLoadPlanId,
    pub customer_return_id: CustomerReturnId,
    pub status: CustomerReturnStatus,
    pub revision: CustomerReturnRevision,
    pub load_id: InboundLoadId,
    pub execution_barcode: String,
    pub lines: Vec<PlannedCustomerReturnLoadLineResult>,
    pub total_authorized_quantity: i64,
    pub planned_by: UserId,
    pub planned_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelCustomerReturnCommand {
    customer_return_id: CustomerReturnId,
    expected_revision: CustomerReturnRevision,
    details: CustomerReturnCancellationDetails,
}

impl CancelCustomerReturnCommand {
    pub const fn new(
        customer_return_id: CustomerReturnId,
        expected_revision: CustomerReturnRevision,
        details: CustomerReturnCancellationDetails,
    ) -> Self {
        Self {
            customer_return_id,
            expected_revision,
            details,
        }
    }

    pub const fn customer_return_id(&self) -> CustomerReturnId {
        self.customer_return_id
    }

    pub const fn expected_revision(&self) -> CustomerReturnRevision {
        self.expected_revision
    }

    pub const fn details(&self) -> &CustomerReturnCancellationDetails {
        &self.details
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelCustomerReturnResult {
    pub cancellation_id: CustomerReturnCancellationId,
    pub customer_return_id: CustomerReturnId,
    pub previous_status: CustomerReturnStatus,
    pub status: CustomerReturnStatus,
    pub revision: CustomerReturnRevision,
    pub reason: CustomerReturnCancellationReason,
    pub note: Option<String>,
    pub cancelled_by: UserId,
    pub cancelled_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomerReturnExecutionStatus {
    Planned,
    Scheduled,
    Arrived,
    Receiving,
    Received,
    Rejected,
    Closed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerReturnLineReadModel {
    pub line_id: CustomerReturnLineId,
    pub sequence: i64,
    pub item_id: CatalogItemId,
    pub item_description: String,
    pub uom: String,
    pub authorized_quantity: i64,
    pub received_quantity: i64,
    pub rejected_quantity: i64,
    pub missing_quantity: i64,
    pub remaining_quantity: i64,
    pub reason: CustomerReturnReason,
    pub note: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub inspection_hold_ids: Vec<InventoryHoldId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerReturnReadModel {
    pub customer_return_id: CustomerReturnId,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub number: String,
    pub customer_reference: String,
    pub expected_at: Option<Timestamp>,
    pub status: CustomerReturnStatus,
    pub revision: CustomerReturnRevision,
    pub line_count: i64,
    pub total_authorized_quantity: i64,
    pub total_received_quantity: i64,
    pub total_rejected_quantity: i64,
    pub total_missing_quantity: i64,
    pub total_remaining_quantity: i64,
    pub load_id: Option<InboundLoadId>,
    pub execution_status: Option<CustomerReturnExecutionStatus>,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub planned_by: Option<UserId>,
    pub planned_at: Option<Timestamp>,
    pub cancellation_id: Option<CustomerReturnCancellationId>,
    pub cancellation_reason: Option<CustomerReturnCancellationReason>,
    pub cancellation_note: Option<String>,
    pub cancelled_by: Option<UserId>,
    pub cancelled_at: Option<Timestamp>,
    pub lines: Vec<CustomerReturnLineReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerReturnPageFilter {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub status: Option<CustomerReturnStatus>,
    pub search: Option<String>,
    pub offset: u64,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerReturnPage {
    pub entries: Vec<CustomerReturnReadModel>,
    pub next_offset: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::idempotency::PreparedCommand;
    use crate::CommandContext;
    use wareboxes_domain::{
        CustomerReturnLineDefinition, CustomerReturnNumber, CustomerReturnQuantity,
        CustomerReturnReference, TenantId,
    };

    #[test]
    fn create_hash_contains_the_exact_return_line_evidence() {
        let command = CreateCustomerReturnCommand {
            authorization: NewCustomerReturn::new(
                InventoryOwnerId::new(2).unwrap(),
                FacilityId::new(3).unwrap(),
                CustomerReturnNumber::new("RMA-42").unwrap(),
                CustomerReturnReference::new("ORDER-42").unwrap(),
                None,
                vec![CustomerReturnLineDefinition::new(
                    CatalogItemId::new(4).unwrap(),
                    CustomerReturnQuantity::new(2).unwrap(),
                    CustomerReturnReason::Damaged,
                    Some("Outer carton crushed".into()),
                    Some("LOT-42".into()),
                    None,
                )
                .unwrap()],
            )
            .unwrap(),
        };
        let context = CommandContext {
            tenant_id: TenantId::new(1).unwrap(),
            actor_id: UserId::new(5).unwrap(),
            request_id: "request-return-create-1".into(),
            idempotency_key: Some("return-create-1".into()),
        };
        let prepared =
            PreparedCommand::new_v1(&context, CREATE_CUSTOMER_RETURN_OPERATION, &command).unwrap();
        assert_eq!(
            prepared.operation().as_str(),
            CREATE_CUSTOMER_RETURN_OPERATION
        );
        assert_eq!(prepared.request_hash().len(), 64);
    }
}
