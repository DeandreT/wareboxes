use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

pub const MAX_CROSS_DOCK_NOTE_LENGTH: usize = 500;
pub const MAX_CROSS_DOCK_INSTRUCTIONS_LENGTH: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossDockWorkStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossDockClaimReleaseReason {
    WorkInterrupted,
    EndOfShift,
    EquipmentIssue,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossDockCancellationReason {
    DemandChanged,
    ReceiptReassigned,
    OperationalChange,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanCrossDockWorkRequest {
    pub order_line_id: i64,
    pub expected_order_revision: Revision,
    pub source_receipt_inventory_transaction_id: i64,
    pub destination_pick_face_location_id: i64,
    pub quantity: i64,
    pub priority: i64,
    pub assigned_user_id: Option<i64>,
    pub due_at: Option<String>,
    pub instructions: Option<String>,
}

impl<'de> Deserialize<'de> for PlanCrossDockWorkRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            order_line_id: i64,
            expected_order_revision: Revision,
            source_receipt_inventory_transaction_id: i64,
            destination_pick_face_location_id: i64,
            quantity: i64,
            #[serde(default)]
            priority: i64,
            #[serde(default)]
            assigned_user_id: Option<i64>,
            #[serde(default)]
            due_at: Option<String>,
            #[serde(default)]
            instructions: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.order_line_id <= 0
            || raw.source_receipt_inventory_transaction_id <= 0
            || raw.destination_pick_face_location_id <= 0
            || raw.quantity <= 0
            || raw.assigned_user_id.is_some_and(|id| id <= 0)
        {
            return Err(D::Error::custom(
                "cross-dock identifiers and quantity must be positive",
            ));
        }
        if raw.priority < 0 {
            return Err(D::Error::custom("priority must be nonnegative"));
        }
        validate_optional_text::<D::Error>(
            raw.instructions.as_deref(),
            MAX_CROSS_DOCK_INSTRUCTIONS_LENGTH,
            "instructions",
        )?;
        Ok(Self {
            order_line_id: raw.order_line_id,
            expected_order_revision: raw.expected_order_revision,
            source_receipt_inventory_transaction_id: raw.source_receipt_inventory_transaction_id,
            destination_pick_face_location_id: raw.destination_pick_face_location_id,
            quantity: raw.quantity,
            priority: raw.priority,
            assigned_user_id: raw.assigned_user_id,
            due_at: raw.due_at,
            instructions: raw.instructions,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanCrossDockWorkResponse {
    pub plan_id: i64,
    pub work_id: i64,
    pub order_id: i64,
    pub order_line_id: i64,
    pub reservation_id: i64,
    pub previous_order_revision: Revision,
    pub order_revision: Revision,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub inbound_load_id: i64,
    pub source_receipt_inventory_transaction_id: i64,
    pub source_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub destination_pick_face_location_id: i64,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub quantity: i64,
    pub remaining_unallocated_quantity: i64,
    pub status: CrossDockWorkStatus,
    pub planned_by: i64,
    pub planned_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimNextCrossDockWorkRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimCrossDockWorkByIdRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatCrossDockClaimRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseCrossDockClaimRequest {
    pub reason: CrossDockClaimReleaseReason,
    pub note: Option<String>,
}

impl<'de> Deserialize<'de> for ReleaseCrossDockClaimRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            reason: CrossDockClaimReleaseReason,
            #[serde(default)]
            note: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        validate_optional_text::<D::Error>(
            raw.note.as_deref(),
            MAX_CROSS_DOCK_NOTE_LENGTH,
            "note",
        )?;
        if raw.reason == CrossDockClaimReleaseReason::Other && raw.note.is_none() {
            return Err(D::Error::custom("other release reason requires note"));
        }
        Ok(Self {
            reason: raw.reason,
            note: raw.note,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelCrossDockWorkRequest {
    pub expected_order_revision: Revision,
    pub reason: CrossDockCancellationReason,
    pub note: Option<String>,
}

impl<'de> Deserialize<'de> for CancelCrossDockWorkRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            expected_order_revision: Revision,
            reason: CrossDockCancellationReason,
            #[serde(default)]
            note: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        validate_optional_text::<D::Error>(
            raw.note.as_deref(),
            MAX_CROSS_DOCK_NOTE_LENGTH,
            "note",
        )?;
        if raw.reason == CrossDockCancellationReason::Other && raw.note.is_none() {
            return Err(D::Error::custom("other cancellation reason requires note"));
        }
        Ok(Self {
            expected_order_revision: raw.expected_order_revision,
            reason: raw.reason,
            note: raw.note,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfirmCrossDockWorkRequest {
    pub source_receiving_location_barcode: String,
    pub item_barcode: String,
    pub lot_scan: Option<String>,
    pub serial_scan: Option<String>,
    pub destination_pick_face_barcode: String,
}

impl<'de> Deserialize<'de> for ConfirmCrossDockWorkRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            source_receiving_location_barcode: String,
            item_barcode: String,
            #[serde(default)]
            lot_scan: Option<String>,
            #[serde(default)]
            serial_scan: Option<String>,
            destination_pick_face_barcode: String,
        }

        let raw = Raw::deserialize(deserializer)?;
        validate_scan::<D::Error>(
            &raw.source_receiving_location_barcode,
            "source receiving location barcode",
        )?;
        validate_scan::<D::Error>(&raw.item_barcode, "item barcode")?;
        validate_optional_scan::<D::Error>(raw.lot_scan.as_deref(), "lot scan")?;
        validate_optional_scan::<D::Error>(raw.serial_scan.as_deref(), "serial scan")?;
        validate_scan::<D::Error>(
            &raw.destination_pick_face_barcode,
            "destination pick face barcode",
        )?;
        Ok(Self {
            source_receiving_location_barcode: raw.source_receiving_location_barcode,
            item_barcode: raw.item_barcode,
            lot_scan: raw.lot_scan,
            serial_scan: raw.serial_scan,
            destination_pick_face_barcode: raw.destination_pick_face_barcode,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDockLocationResponse {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDockClaimResponse {
    pub work_id: i64,
    pub plan_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub order_id: i64,
    pub order_key: String,
    pub order_line_id: i64,
    pub order_line_key: String,
    pub reservation_id: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub due_at: Option<String>,
    pub lease_expires_at: String,
    pub source_receipt_inventory_transaction_id: i64,
    pub source_inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub item_barcodes: Vec<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub quantity: i64,
    pub source_receiving_location: CrossDockLocationResponse,
    pub destination_pick_face: CrossDockLocationResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDockClaimHeartbeatResponse {
    pub work_id: i64,
    pub heartbeat_at: String,
    pub lease_expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDockClaimReleaseResponse {
    pub work_id: i64,
    pub status: CrossDockWorkStatus,
    pub released_at: String,
    pub release_count: i64,
    pub reason: CrossDockClaimReleaseReason,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmCrossDockWorkResponse {
    pub confirmation_id: i64,
    pub work_id: i64,
    pub plan_id: i64,
    pub order_id: i64,
    pub order_line_id: i64,
    pub reservation_id: i64,
    pub inventory_transaction_id: i64,
    pub inventory_allocation_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub destination_pick_face_location_id: i64,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub quantity: i64,
    pub status: CrossDockWorkStatus,
    pub confirmed_by: i64,
    pub confirmed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelCrossDockWorkResponse {
    pub cancellation_id: i64,
    pub work_id: i64,
    pub plan_id: i64,
    pub order_id: i64,
    pub order_line_id: i64,
    pub previous_order_revision: Revision,
    pub order_revision: Revision,
    pub quantity: i64,
    pub previous_status: CrossDockWorkStatus,
    pub status: CrossDockWorkStatus,
    pub reason: CrossDockCancellationReason,
    pub note: Option<String>,
    pub cancelled_by: i64,
    pub cancelled_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CrossDockWorkPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<CrossDockWorkStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CrossDockPlanningOptionPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDockPlanningOptionResponse {
    pub order_id: i64,
    pub order_key: String,
    pub order_revision: Revision,
    pub order_line_id: i64,
    pub order_line_key: String,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub reservation_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub unallocated_quantity: i64,
    pub source_receipt_inventory_transaction_id: i64,
    pub inbound_load_id: i64,
    pub inbound_load_reference: Option<String>,
    pub source_inventory_balance_id: i64,
    pub source_receiving_location: CrossDockLocationResponse,
    pub source_free_quantity: i64,
    pub receipt_remaining_quantity: i64,
    pub maximum_plan_quantity: i64,
    pub destination_pick_faces: Vec<CrossDockLocationResponse>,
}

pub type CrossDockPlanningOptionPage = CursorPage<CrossDockPlanningOptionResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossDockWorkResponse {
    pub work_id: i64,
    pub plan_id: i64,
    pub status: CrossDockWorkStatus,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub inbound_load_id: i64,
    pub order_id: i64,
    pub order_key: String,
    pub order_revision: Revision,
    pub order_line_id: i64,
    pub order_line_key: String,
    pub reservation_id: i64,
    pub priority: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub quantity: i64,
    pub source_inventory_balance_id: i64,
    pub source_receiving_location: CrossDockLocationResponse,
    pub destination_pick_face: CrossDockLocationResponse,
    pub claimed_by: Option<i64>,
    pub lease_expires_at: Option<String>,
    pub due_at: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

pub type CrossDockWorkPage = CursorPage<CrossDockWorkResponse>;

fn validate_optional_text<E: serde::de::Error>(
    value: Option<&str>,
    max_chars: usize,
    field: &str,
) -> Result<(), E> {
    if value.is_some_and(|value| {
        value.is_empty()
            || value.trim() != value
            || value.chars().count() > max_chars
            || value.chars().any(char::is_control)
    }) {
        Err(E::custom(format!(
            "{field} must be trimmed, nonempty, and at most {max_chars} characters"
        )))
    } else {
        Ok(())
    }
}

fn validate_scan<E: serde::de::Error>(value: &str, field: &str) -> Result<(), E> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > 255
        || value.chars().any(char::is_control)
    {
        Err(E::custom(format!(
            "{field} must be trimmed, nonempty, and at most 255 characters"
        )))
    } else {
        Ok(())
    }
}

fn validate_optional_scan<E: serde::de::Error>(value: Option<&str>, field: &str) -> Result<(), E> {
    value.map_or(Ok(()), |value| validate_scan::<E>(value, field))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_request_is_strict_and_rejects_negative_priority() {
        let valid = serde_json::json!({
            "order_line_id": 2,
            "expected_order_revision": 3,
            "source_receipt_inventory_transaction_id": 4,
            "destination_pick_face_location_id": 5,
            "quantity": 6,
            "priority": 10
        });
        assert!(serde_json::from_value::<PlanCrossDockWorkRequest>(valid.clone()).is_ok());
        let mut invalid = valid;
        invalid["priority"] = serde_json::json!(-1);
        assert!(serde_json::from_value::<PlanCrossDockWorkRequest>(invalid).is_err());
    }

    #[test]
    fn other_cancellation_requires_a_note() {
        assert!(
            serde_json::from_value::<CancelCrossDockWorkRequest>(serde_json::json!({
                "expected_order_revision": 4,
                "reason": "other"
            }))
            .is_err()
        );
    }

    #[test]
    fn scanner_confirmation_rejects_unknown_fields() {
        assert!(
            serde_json::from_value::<ConfirmCrossDockWorkRequest>(serde_json::json!({
                "source_receiving_location_barcode": "DOCK-01",
                "item_barcode": "SKU-01",
                "lot_scan": null,
                "serial_scan": null,
                "destination_pick_face_barcode": "PICK-01",
                "quantity": 4
            }))
            .is_err()
        );
    }
}
