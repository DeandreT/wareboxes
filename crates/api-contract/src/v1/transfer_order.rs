use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

pub const MAX_TRANSFER_ORDER_CANCELLATION_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferOrderStatus {
    Draft,
    Released,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferOrderCancellationReason {
    DemandCancelled,
    DuplicateOrder,
    RouteCancelled,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTransferOrderLineRequest {
    pub item_id: i64,
    pub requested_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTransferOrderRequest {
    pub inventory_owner_id: i64,
    pub source_facility_id: i64,
    pub destination_facility_id: i64,
    pub number: String,
    pub expected_departure_at: Option<String>,
    pub expected_arrival_at: Option<String>,
    pub lines: Vec<CreateTransferOrderLineRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatedTransferOrderLineResponse {
    pub line_id: i64,
    pub item_id: i64,
    pub requested_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTransferOrderResponse {
    pub transfer_order_id: i64,
    pub number: String,
    pub status: TransferOrderStatus,
    pub revision: Revision,
    pub lines: Vec<CreatedTransferOrderLineResponse>,
    pub total_requested_quantity: i64,
    pub created_by: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTransferOrderRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTransferOrderResponse {
    pub release_id: i64,
    pub transfer_order_id: i64,
    pub previous_status: TransferOrderStatus,
    pub status: TransferOrderStatus,
    pub revision: Revision,
    pub released_by: i64,
    pub released_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelTransferOrderRequest {
    pub expected_revision: Revision,
    pub reason: TransferOrderCancellationReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl<'de> Deserialize<'de> for CancelTransferOrderRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            expected_revision: Revision,
            reason: TransferOrderCancellationReason,
            #[serde(default)]
            note: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.note.as_ref().is_some_and(|note| {
            note.is_empty()
                || note.trim() != note
                || note.chars().count() > MAX_TRANSFER_ORDER_CANCELLATION_NOTE_LENGTH
                || note.chars().any(char::is_control)
        }) || (raw.reason == TransferOrderCancellationReason::Other && raw.note.is_none())
        {
            return Err(serde::de::Error::custom(
                "transfer order cancellation note is invalid",
            ));
        }
        Ok(Self {
            expected_revision: raw.expected_revision,
            reason: raw.reason,
            note: raw.note,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelTransferOrderResponse {
    pub cancellation_id: i64,
    pub transfer_order_id: i64,
    pub previous_status: TransferOrderStatus,
    pub status: TransferOrderStatus,
    pub revision: Revision,
    pub reason: TransferOrderCancellationReason,
    pub note: Option<String>,
    pub cancelled_by: i64,
    pub cancelled_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferOrderLineResponse {
    pub line_id: i64,
    pub sequence: i64,
    pub item_id: i64,
    pub item_description: String,
    pub uom: String,
    pub requested_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferOrderSummaryResponse {
    pub transfer_order_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub source_facility_id: i64,
    pub source_facility_name: String,
    pub destination_facility_id: i64,
    pub destination_facility_name: String,
    pub number: String,
    pub expected_departure_at: Option<String>,
    pub expected_arrival_at: Option<String>,
    pub status: TransferOrderStatus,
    pub revision: Revision,
    pub line_count: i64,
    pub total_requested_quantity: i64,
    pub created_by: i64,
    pub created_at: String,
    pub released_by: Option<i64>,
    pub released_at: Option<String>,
    pub cancellation_id: Option<i64>,
    pub cancellation_reason: Option<TransferOrderCancellationReason>,
    pub cancellation_note: Option<String>,
    pub cancelled_by: Option<i64>,
    pub cancelled_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferOrderDetailResponse {
    #[serde(flatten)]
    pub summary: TransferOrderSummaryResponse,
    pub lines: Vec<TransferOrderLineResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TransferOrderPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TransferOrderStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type TransferOrderPage = CursorPage<TransferOrderSummaryResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_request_is_strict() {
        let value = serde_json::json!({
            "inventory_owner_id": 1, "source_facility_id": 2, "destination_facility_id": 3,
            "number": "TO-100", "expected_departure_at": null, "expected_arrival_at": null,
            "lines": [{"item_id": 4, "requested_quantity": 5}]
        });
        assert!(serde_json::from_value::<CreateTransferOrderRequest>(value.clone()).is_ok());
        let mut invalid = value;
        invalid["tenant_id"] = serde_json::json!(1);
        assert!(serde_json::from_value::<CreateTransferOrderRequest>(invalid).is_err());
    }

    #[test]
    fn cancellation_requires_other_note() {
        assert!(serde_json::from_value::<CancelTransferOrderRequest>(
            serde_json::json!({"expected_revision": 1, "reason": "other"})
        )
        .is_err());
        assert!(serde_json::from_value::<CancelTransferOrderRequest>(
            serde_json::json!({"expected_revision": 1, "reason": "route_cancelled"})
        )
        .is_ok());
    }
}
