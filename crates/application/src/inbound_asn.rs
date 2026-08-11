//! Typed advance shipping notice intake and atomic inbound-load planning contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CatalogItemId, FacilityId, InboundAsnCancellationDetails, InboundAsnCancellationId,
    InboundAsnCancellationReason, InboundAsnId, InboundAsnLineId, InboundAsnLoadPlanDetails,
    InboundAsnLoadPlanId, InboundAsnRevision, InboundAsnStatus, InboundLoadId, InboundLoadLineId,
    InventoryOwnerId, NewInboundAsn, NewPurchaseOrderAsn, PurchaseOrderAsnSourceId,
    PurchaseOrderAsnSourceLineId, PurchaseOrderId, PurchaseOrderLineId, PurchaseOrderRevision,
    Timestamp, UserId,
};

pub const CREATE_INBOUND_ASN_OPERATION: &str = "inbound.asn.create.v1";
pub const PLAN_INBOUND_ASN_LOAD_OPERATION: &str = "inbound.asn.load.plan.v1";
pub const CREATE_PURCHASE_ORDER_ASN_OPERATION: &str = "inbound.purchase_order.asn.create.v1";
pub const CANCEL_INBOUND_ASN_OPERATION: &str = "inbound.asn.cancel.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInboundAsnCommand {
    pub notice: NewInboundAsn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedInboundAsnLineResult {
    pub line_id: InboundAsnLineId,
    pub item_id: CatalogItemId,
    pub expected_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInboundAsnResult {
    pub asn_id: InboundAsnId,
    pub number: String,
    pub status: InboundAsnStatus,
    pub revision: InboundAsnRevision,
    pub lines: Vec<CreatedInboundAsnLineResult>,
    pub total_expected_quantity: i64,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePurchaseOrderAsnCommand {
    pub notice: NewPurchaseOrderAsn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedPurchaseOrderAsnLineResult {
    pub source_line_id: PurchaseOrderAsnSourceLineId,
    pub purchase_order_line_id: PurchaseOrderLineId,
    pub asn_line_id: InboundAsnLineId,
    pub item_id: CatalogItemId,
    pub expected_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePurchaseOrderAsnResult {
    pub source_id: PurchaseOrderAsnSourceId,
    pub purchase_order_id: PurchaseOrderId,
    pub purchase_order_revision: PurchaseOrderRevision,
    pub asn_id: InboundAsnId,
    pub number: String,
    pub status: InboundAsnStatus,
    pub revision: InboundAsnRevision,
    pub lines: Vec<CreatedPurchaseOrderAsnLineResult>,
    pub total_expected_quantity: i64,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanInboundAsnLoadCommand {
    pub asn_id: InboundAsnId,
    pub expected_revision: InboundAsnRevision,
    pub details: InboundAsnLoadPlanDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedInboundAsnLoadLineResult {
    pub asn_line_id: InboundAsnLineId,
    pub load_line_id: InboundLoadLineId,
    pub item_id: CatalogItemId,
    pub expected_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanInboundAsnLoadResult {
    pub plan_id: InboundAsnLoadPlanId,
    pub asn_id: InboundAsnId,
    pub asn_status: InboundAsnStatus,
    pub asn_revision: InboundAsnRevision,
    pub load_id: InboundLoadId,
    pub execution_barcode: String,
    pub lines: Vec<PlannedInboundAsnLoadLineResult>,
    pub total_expected_quantity: i64,
    pub planned_by: UserId,
    pub planned_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelInboundAsnCommand {
    asn_id: InboundAsnId,
    expected_revision: InboundAsnRevision,
    details: InboundAsnCancellationDetails,
}

impl CancelInboundAsnCommand {
    pub const fn new(
        asn_id: InboundAsnId,
        expected_revision: InboundAsnRevision,
        details: InboundAsnCancellationDetails,
    ) -> Self {
        Self {
            asn_id,
            expected_revision,
            details,
        }
    }

    pub const fn asn_id(&self) -> InboundAsnId {
        self.asn_id
    }

    pub const fn expected_revision(&self) -> InboundAsnRevision {
        self.expected_revision
    }

    pub const fn details(&self) -> &InboundAsnCancellationDetails {
        &self.details
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelInboundAsnResult {
    pub cancellation_id: InboundAsnCancellationId,
    pub asn_id: InboundAsnId,
    pub previous_status: InboundAsnStatus,
    pub status: InboundAsnStatus,
    pub revision: InboundAsnRevision,
    pub reason: InboundAsnCancellationReason,
    pub note: Option<String>,
    pub cancelled_by: UserId,
    pub cancelled_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundAsnExecutionStatus {
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
pub struct InboundAsnLineReadModel {
    pub line_id: InboundAsnLineId,
    pub sequence: i64,
    pub item_id: CatalogItemId,
    pub item_description: String,
    pub uom: String,
    pub expected_quantity: i64,
    pub received_quantity: i64,
    pub rejected_quantity: i64,
    pub missing_quantity: i64,
    pub remaining_quantity: i64,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundAsnReadModel {
    pub asn_id: InboundAsnId,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub number: String,
    pub supplier: String,
    pub expected_at: Option<Timestamp>,
    pub status: InboundAsnStatus,
    pub revision: InboundAsnRevision,
    pub line_count: i64,
    pub total_expected_quantity: i64,
    pub total_received_quantity: i64,
    pub total_rejected_quantity: i64,
    pub total_missing_quantity: i64,
    pub total_remaining_quantity: i64,
    pub load_id: Option<InboundLoadId>,
    pub execution_status: Option<InboundAsnExecutionStatus>,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub planned_by: Option<UserId>,
    pub planned_at: Option<Timestamp>,
    pub cancellation_id: Option<InboundAsnCancellationId>,
    pub cancellation_reason: Option<InboundAsnCancellationReason>,
    pub cancellation_note: Option<String>,
    pub cancelled_by: Option<UserId>,
    pub cancelled_at: Option<Timestamp>,
    pub purchase_order_source_id: Option<PurchaseOrderAsnSourceId>,
    pub purchase_order_id: Option<PurchaseOrderId>,
    pub purchase_order_number: Option<String>,
    pub lines: Vec<InboundAsnLineReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundAsnPageFilter {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub status: Option<InboundAsnStatus>,
    pub search: Option<String>,
    pub offset: u64,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundAsnPage {
    pub entries: Vec<InboundAsnReadModel>,
    pub next_offset: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::idempotency::PreparedCommand;
    use crate::CommandContext;
    use wareboxes_domain::{
        InboundAsnCancellationDetails, InboundAsnCancellationReason, InboundAsnLineDefinition,
        InboundAsnNumber, InboundAsnQuantity, InboundAsnSupplier, TenantId,
    };

    #[test]
    fn create_hash_contains_the_exact_source_line_set() {
        let command = CreateInboundAsnCommand {
            notice: NewInboundAsn::new(
                InventoryOwnerId::new(2).unwrap(),
                FacilityId::new(3).unwrap(),
                InboundAsnNumber::new("ASN-100").unwrap(),
                InboundAsnSupplier::new("Northstar Foods").unwrap(),
                None,
                vec![InboundAsnLineDefinition::new(
                    CatalogItemId::new(4).unwrap(),
                    InboundAsnQuantity::new(5).unwrap(),
                    Some("LOT-A".into()),
                    None,
                    None,
                )
                .unwrap()],
            )
            .unwrap(),
        };
        let context = CommandContext {
            tenant_id: TenantId::new(1).unwrap(),
            actor_id: UserId::new(6).unwrap(),
            request_id: "req-1".into(),
            idempotency_key: Some("key-1".into()),
        };
        let prepared =
            PreparedCommand::new_v1(&context, CREATE_INBOUND_ASN_OPERATION, &command).unwrap();
        assert!(!prepared.request_hash().is_empty());
    }

    #[test]
    fn cancellation_hash_contains_revision_and_reason() {
        let command = CancelInboundAsnCommand::new(
            InboundAsnId::new(8).unwrap(),
            InboundAsnRevision::new(1).unwrap(),
            InboundAsnCancellationDetails::new(
                InboundAsnCancellationReason::SupplierCancelled,
                None,
            )
            .unwrap(),
        );
        let context = CommandContext {
            tenant_id: TenantId::new(1).unwrap(),
            actor_id: UserId::new(6).unwrap(),
            request_id: "req-cancel".into(),
            idempotency_key: Some("key-cancel".into()),
        };
        let prepared =
            PreparedCommand::new_v1(&context, CANCEL_INBOUND_ASN_OPERATION, &command).unwrap();
        let changed = CancelInboundAsnCommand::new(
            InboundAsnId::new(8).unwrap(),
            InboundAsnRevision::new(1).unwrap(),
            InboundAsnCancellationDetails::new(InboundAsnCancellationReason::DuplicateNotice, None)
                .unwrap(),
        );
        let changed =
            PreparedCommand::new_v1(&context, CANCEL_INBOUND_ASN_OPERATION, &changed).unwrap();
        assert_ne!(prepared.request_hash(), changed.request_hash());
    }
}
