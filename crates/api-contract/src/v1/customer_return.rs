use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

pub const MAX_CUSTOMER_RETURN_NOTE_LENGTH: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomerReturnStatus {
    Open,
    Planned,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomerReturnReason {
    CustomerRequest,
    Damaged,
    RefusedDelivery,
    Recall,
    Warranty,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomerReturnCancellationReason {
    CustomerCancelled,
    DuplicateAuthorization,
    ReturnWindowExpired,
    Other,
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
#[serde(deny_unknown_fields)]
pub struct CreateCustomerReturnLineRequest {
    pub item_id: i64,
    pub authorized_quantity: i64,
    pub reason: CustomerReturnReason,
    pub note: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCustomerReturnRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub number: String,
    pub customer_reference: String,
    pub expected_at: Option<String>,
    pub lines: Vec<CreateCustomerReturnLineRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatedCustomerReturnLineResponse {
    pub line_id: i64,
    pub item_id: i64,
    pub authorized_quantity: i64,
    pub reason: CustomerReturnReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCustomerReturnResponse {
    pub customer_return_id: i64,
    pub number: String,
    pub status: CustomerReturnStatus,
    pub revision: Revision,
    pub lines: Vec<CreatedCustomerReturnLineResponse>,
    pub total_authorized_quantity: i64,
    pub created_by: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanCustomerReturnLoadRequest {
    pub expected_revision: Revision,
    pub receiving_location_id: i64,
    pub carrier: Option<String>,
    pub trailer_number: Option<String>,
    pub seal_number: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedCustomerReturnLoadLineResponse {
    pub customer_return_line_id: i64,
    pub load_line_id: i64,
    pub item_id: i64,
    pub authorized_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanCustomerReturnLoadResponse {
    pub plan_id: i64,
    pub customer_return_id: i64,
    pub status: CustomerReturnStatus,
    pub revision: Revision,
    pub load_id: i64,
    pub execution_barcode: String,
    pub lines: Vec<PlannedCustomerReturnLoadLineResponse>,
    pub total_authorized_quantity: i64,
    pub planned_by: i64,
    pub planned_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelCustomerReturnRequest {
    pub expected_revision: Revision,
    pub reason: CustomerReturnCancellationReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl<'de> Deserialize<'de> for CancelCustomerReturnRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            expected_revision: Revision,
            reason: CustomerReturnCancellationReason,
            #[serde(default)]
            note: Option<String>,
        }

        let raw = Raw::deserialize(deserializer)?;
        if raw.note.as_ref().is_some_and(|note| {
            note.is_empty()
                || note.trim() != note
                || note.chars().count() > MAX_CUSTOMER_RETURN_NOTE_LENGTH
                || note.chars().any(char::is_control)
        }) || (raw.reason == CustomerReturnCancellationReason::Other && raw.note.is_none())
        {
            return Err(serde::de::Error::custom(
                "customer return cancellation note is invalid",
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
pub struct CancelCustomerReturnResponse {
    pub cancellation_id: i64,
    pub customer_return_id: i64,
    pub previous_status: CustomerReturnStatus,
    pub status: CustomerReturnStatus,
    pub revision: Revision,
    pub reason: CustomerReturnCancellationReason,
    pub note: Option<String>,
    pub cancelled_by: i64,
    pub cancelled_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomerReturnLineResponse {
    pub line_id: i64,
    pub sequence: i64,
    pub item_id: i64,
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
    pub inspection_hold_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomerReturnSummaryResponse {
    pub customer_return_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub number: String,
    pub customer_reference: String,
    pub expected_at: Option<String>,
    pub status: CustomerReturnStatus,
    pub revision: Revision,
    pub line_count: i64,
    pub total_authorized_quantity: i64,
    pub total_received_quantity: i64,
    pub total_rejected_quantity: i64,
    pub total_missing_quantity: i64,
    pub total_remaining_quantity: i64,
    pub load_id: Option<i64>,
    pub execution_status: Option<CustomerReturnExecutionStatus>,
    pub created_by: i64,
    pub created_at: String,
    pub planned_by: Option<i64>,
    pub planned_at: Option<String>,
    pub cancellation_id: Option<i64>,
    pub cancellation_reason: Option<CustomerReturnCancellationReason>,
    pub cancellation_note: Option<String>,
    pub cancelled_by: Option<i64>,
    pub cancelled_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomerReturnDetailResponse {
    #[serde(flatten)]
    pub summary: CustomerReturnSummaryResponse,
    pub lines: Vec<CustomerReturnLineResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CustomerReturnPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<CustomerReturnStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type CustomerReturnPage = CursorPage<CustomerReturnSummaryResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_request_is_strict_and_keeps_return_evidence() {
        let value = serde_json::json!({
            "inventory_owner_id": 7,
            "facility_id": 8,
            "number": "RMA-100",
            "customer_reference": "WEB-ORDER-100",
            "expected_at": null,
            "lines": [{
                "item_id": 41,
                "authorized_quantity": 2,
                "reason": "damaged",
                "note": "Outer carton crushed",
                "lot": "LOT-A",
                "serial": null
            }]
        });
        let request = serde_json::from_value::<CreateCustomerReturnRequest>(value.clone()).unwrap();
        assert_eq!(request.lines[0].authorized_quantity, 2);
        let mut invalid = value;
        invalid["tenant_id"] = serde_json::json!(1);
        assert!(serde_json::from_value::<CreateCustomerReturnRequest>(invalid).is_err());
    }

    #[test]
    fn cancellation_is_strict_and_other_requires_note() {
        assert!(
            serde_json::from_value::<CancelCustomerReturnRequest>(serde_json::json!({
                "expected_revision": 1,
                "reason": "customer_cancelled"
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<CancelCustomerReturnRequest>(serde_json::json!({
                "expected_revision": 1,
                "reason": "other"
            }))
            .is_err()
        );
    }

    #[test]
    fn planning_is_revisioned_and_server_derives_the_line_set() {
        let mut value = serde_json::json!({
            "expected_revision": 1,
            "receiving_location_id": 9,
            "carrier": null,
            "trailer_number": null,
            "seal_number": null
        });
        assert!(serde_json::from_value::<PlanCustomerReturnLoadRequest>(value.clone()).is_ok());
        value["lines"] = serde_json::json!([]);
        assert!(serde_json::from_value::<PlanCustomerReturnLoadRequest>(value).is_err());
    }
}
