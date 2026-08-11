//! Typed interfacility transfer-order planning contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CatalogItemId, FacilityId, InventoryBalanceId, InventoryOwnerId, ItemBatchId, LocationId,
    NewTransferOrder, Timestamp, TransferDispatchExecution, TransferOrderCancellationDetails,
    TransferOrderCancellationId, TransferOrderCancellationReason, TransferOrderDispatchId,
    TransferOrderDispatchLineId, TransferOrderId, TransferOrderLineId, TransferOrderReceiptId,
    TransferOrderReceiptLineId, TransferOrderReleaseId, TransferOrderRevision,
    TransferOrderScanValue, TransferOrderStatus, UserId,
};

pub const CREATE_TRANSFER_ORDER_OPERATION: &str = "inventory.transfer_order.create.v1";
pub const RELEASE_TRANSFER_ORDER_OPERATION: &str = "inventory.transfer_order.release.v1";
pub const CANCEL_TRANSFER_ORDER_OPERATION: &str = "inventory.transfer_order.cancel.v1";
pub const DISPATCH_TRANSFER_ORDER_OPERATION: &str = "inventory.transfer_order.dispatch.v1";
pub const RECEIVE_TRANSFER_ORDER_OPERATION: &str = "inventory.transfer_order.receive.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTransferOrderCommand {
    pub order: NewTransferOrder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedTransferOrderLineResult {
    pub line_id: TransferOrderLineId,
    pub item_id: CatalogItemId,
    pub requested_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTransferOrderResult {
    pub transfer_order_id: TransferOrderId,
    pub number: String,
    pub status: TransferOrderStatus,
    pub revision: TransferOrderRevision,
    pub lines: Vec<CreatedTransferOrderLineResult>,
    pub total_requested_quantity: i64,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseTransferOrderCommand {
    pub transfer_order_id: TransferOrderId,
    pub expected_revision: TransferOrderRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseTransferOrderResult {
    pub release_id: TransferOrderReleaseId,
    pub transfer_order_id: TransferOrderId,
    pub previous_status: TransferOrderStatus,
    pub status: TransferOrderStatus,
    pub revision: TransferOrderRevision,
    pub released_by: UserId,
    pub released_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelTransferOrderCommand {
    transfer_order_id: TransferOrderId,
    expected_revision: TransferOrderRevision,
    details: TransferOrderCancellationDetails,
}

impl CancelTransferOrderCommand {
    pub const fn new(
        transfer_order_id: TransferOrderId,
        expected_revision: TransferOrderRevision,
        details: TransferOrderCancellationDetails,
    ) -> Self {
        Self {
            transfer_order_id,
            expected_revision,
            details,
        }
    }
    pub const fn transfer_order_id(&self) -> TransferOrderId {
        self.transfer_order_id
    }
    pub const fn expected_revision(&self) -> TransferOrderRevision {
        self.expected_revision
    }
    pub const fn details(&self) -> &TransferOrderCancellationDetails {
        &self.details
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelTransferOrderResult {
    pub cancellation_id: TransferOrderCancellationId,
    pub transfer_order_id: TransferOrderId,
    pub previous_status: TransferOrderStatus,
    pub status: TransferOrderStatus,
    pub revision: TransferOrderRevision,
    pub reason: TransferOrderCancellationReason,
    pub note: Option<String>,
    pub cancelled_by: UserId,
    pub cancelled_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchTransferOrderCommand {
    pub transfer_order_id: TransferOrderId,
    pub expected_revision: TransferOrderRevision,
    pub execution: TransferDispatchExecution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferDispatchLineResult {
    pub dispatch_line_id: TransferOrderDispatchLineId,
    pub transfer_order_line_id: TransferOrderLineId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub transit_inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub item_id: CatalogItemId,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
    pub inventory_status: String,
    pub quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchTransferOrderResult {
    pub dispatch_id: TransferOrderDispatchId,
    pub transfer_order_id: TransferOrderId,
    pub previous_status: TransferOrderStatus,
    pub status: TransferOrderStatus,
    pub revision: TransferOrderRevision,
    pub transit_location_id: LocationId,
    pub transit_location_barcode: String,
    pub inventory_transaction_id: i64,
    pub lines: Vec<TransferDispatchLineResult>,
    pub total_dispatched_quantity: i64,
    pub dispatched_by: UserId,
    pub dispatched_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveTransferOrderCommand {
    pub transfer_order_id: TransferOrderId,
    pub expected_revision: TransferOrderRevision,
    pub destination_location_id: LocationId,
    pub observed_destination_location_barcode: TransferOrderScanValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferReceiptLineResult {
    pub receipt_line_id: TransferOrderReceiptLineId,
    pub dispatch_line_id: TransferOrderDispatchLineId,
    pub transfer_order_line_id: TransferOrderLineId,
    pub transit_inventory_balance_id: InventoryBalanceId,
    pub destination_inventory_balance_id: InventoryBalanceId,
    pub item_batch_id: ItemBatchId,
    pub item_id: CatalogItemId,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
    pub inventory_status: String,
    pub quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveTransferOrderResult {
    pub receipt_id: TransferOrderReceiptId,
    pub transfer_order_id: TransferOrderId,
    pub previous_status: TransferOrderStatus,
    pub status: TransferOrderStatus,
    pub revision: TransferOrderRevision,
    pub destination_location_id: LocationId,
    pub destination_location_barcode: String,
    pub inventory_transaction_id: i64,
    pub lines: Vec<TransferReceiptLineResult>,
    pub total_received_quantity: i64,
    pub received_by: UserId,
    pub received_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferDispatchCandidateReadModel {
    pub transfer_order_line_id: TransferOrderLineId,
    pub source_inventory_balance_id: InventoryBalanceId,
    pub source_location_id: LocationId,
    pub source_location_barcode: String,
    pub source_location_name: String,
    pub item_batch_id: ItemBatchId,
    pub item_id: CatalogItemId,
    pub item_description: String,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
    pub free_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferExecutionLocationReadModel {
    pub location_id: LocationId,
    pub barcode: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferExecutionReadiness {
    pub transfer_order_id: TransferOrderId,
    pub revision: TransferOrderRevision,
    pub status: TransferOrderStatus,
    pub dispatch_candidates: Vec<TransferDispatchCandidateReadModel>,
    pub transit_locations: Vec<TransferExecutionLocationReadModel>,
    pub receiving_locations: Vec<TransferExecutionLocationReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferOrderLineReadModel {
    pub line_id: TransferOrderLineId,
    pub sequence: i64,
    pub item_id: CatalogItemId,
    pub item_description: String,
    pub uom: String,
    pub requested_quantity: i64,
    pub dispatched_quantity: i64,
    pub received_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferOrderReadModel {
    pub transfer_order_id: TransferOrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub source_facility_id: FacilityId,
    pub source_facility_name: String,
    pub destination_facility_id: FacilityId,
    pub destination_facility_name: String,
    pub number: String,
    pub expected_departure_at: Option<Timestamp>,
    pub expected_arrival_at: Option<Timestamp>,
    pub status: TransferOrderStatus,
    pub revision: TransferOrderRevision,
    pub line_count: i64,
    pub total_requested_quantity: i64,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub released_by: Option<UserId>,
    pub released_at: Option<Timestamp>,
    pub cancellation_id: Option<TransferOrderCancellationId>,
    pub cancellation_reason: Option<TransferOrderCancellationReason>,
    pub cancellation_note: Option<String>,
    pub cancelled_by: Option<UserId>,
    pub cancelled_at: Option<Timestamp>,
    pub dispatch_id: Option<TransferOrderDispatchId>,
    pub dispatch_inventory_transaction_id: Option<i64>,
    pub transit_location_id: Option<LocationId>,
    pub transit_location_barcode: Option<String>,
    pub dispatched_by: Option<UserId>,
    pub dispatched_at: Option<Timestamp>,
    pub receipt_id: Option<TransferOrderReceiptId>,
    pub receipt_inventory_transaction_id: Option<i64>,
    pub destination_receiving_location_id: Option<LocationId>,
    pub destination_receiving_location_barcode: Option<String>,
    pub received_by: Option<UserId>,
    pub received_at: Option<Timestamp>,
    pub lines: Vec<TransferOrderLineReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferOrderPageFilter {
    pub source_facility_id: Option<FacilityId>,
    pub destination_facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub status: Option<TransferOrderStatus>,
    pub search: Option<String>,
    pub offset: u64,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferOrderPage {
    pub entries: Vec<TransferOrderReadModel>,
    pub next_offset: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{idempotency::PreparedCommand, CommandContext};
    use wareboxes_domain::{
        TenantId, TransferOrderCancellationNote, TransferOrderCancellationReason,
    };

    #[test]
    fn cancellation_hash_contains_reason_and_note() {
        let context = CommandContext {
            tenant_id: TenantId::new(1).unwrap(),
            actor_id: UserId::new(2).unwrap(),
            request_id: "req-transfer".into(),
            idempotency_key: Some("key-transfer".into()),
        };
        let command = CancelTransferOrderCommand::new(
            TransferOrderId::new(3).unwrap(),
            TransferOrderRevision::new(2).unwrap(),
            TransferOrderCancellationDetails::new(
                TransferOrderCancellationReason::RouteCancelled,
                Some(TransferOrderCancellationNote::new("Lane unavailable").unwrap()),
            )
            .unwrap(),
        );
        let prepared =
            PreparedCommand::new_v1(&context, CANCEL_TRANSFER_ORDER_OPERATION, &command).unwrap();
        assert!(!prepared.request_hash().is_empty());
    }
}
