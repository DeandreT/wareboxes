use serde::{Deserialize, Serialize};

use super::{
    CursorPage, InventoryBalanceSearchQuery, InventoryBalanceStatus, OpaqueCursor, PageLimit,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InventorySortDirection {
    Ascending,
    #[default]
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InventoryJournalSort {
    #[default]
    OccurredAt,
    Transaction,
    Type,
    Client,
    NetQuantity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InventoryIntegritySort {
    #[default]
    Severity,
    Facility,
    Client,
    Item,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryIntegrityIssueKind {
    JournalProjection,
    Commitments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryAgingBucket {
    Expired,
    #[serde(rename = "due_within_7_days")]
    DueWithin7Days,
    #[serde(rename = "due_within_30_days")]
    DueWithin30Days,
    #[serde(rename = "due_within_90_days")]
    DueWithin90Days,
    #[serde(rename = "beyond_90_days")]
    Beyond90Days,
    NoExpiration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InventoryAgingSort {
    #[default]
    Age,
    Expiration,
    Quantity,
    Facility,
    Client,
    Item,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct InventoryAgingPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<InventoryBalanceSearchQuery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket: Option<InventoryAgingBucket>,
    #[serde(default)]
    pub sort: InventoryAgingSort,
    #[serde(default)]
    pub direction: InventorySortDirection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryAgingResponse {
    pub inventory_balance_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub primary_sku: Option<String>,
    pub item_description: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub received_at: String,
    pub age_days: i64,
    pub expiration: Option<String>,
    pub days_to_expiration: Option<i64>,
    pub bucket: InventoryAgingBucket,
    pub status: InventoryBalanceStatus,
    pub on_hand_quantity: i64,
    pub reserved_quantity: i64,
    pub held_quantity: i64,
    pub available_quantity: i64,
}

pub type InventoryAgingPage = CursorPage<InventoryAgingResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct InventoryJournalPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<InventoryBalanceSearchQuery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_batch_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_plate_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<i64>,
    #[serde(default)]
    pub sort: InventoryJournalSort,
    #[serde(default)]
    pub direction: InventorySortDirection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryJournalTransactionResponse {
    pub id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub occurred_at: String,
    pub actor_user_id: Option<i64>,
    pub transaction_type: String,
    pub reason: Option<String>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
    pub correlation_id: Option<String>,
    pub operation: String,
    pub entry_count: u32,
    pub net_quantity: i64,
    pub entries: Vec<InventoryJournalEntryResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryJournalEntryResponse {
    pub id: i64,
    pub facility_id: i64,
    pub facility_name: String,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub primary_sku: Option<String>,
    pub item_description: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<String>,
    pub serial: Option<String>,
    pub status: InventoryBalanceStatus,
    pub quantity_delta: i64,
}

pub type InventoryJournalPage = CursorPage<InventoryJournalTransactionResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct InventoryIntegrityPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<InventoryIntegrityIssueKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    #[serde(default)]
    pub sort: InventoryIntegritySort,
    #[serde(default)]
    pub direction: InventorySortDirection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryIntegrityIssueResponse {
    pub issue_key: String,
    pub kind: InventoryIntegrityIssueKind,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub primary_sku: Option<String>,
    pub item_description: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub uom: String,
    pub status: InventoryBalanceStatus,
    pub journal_quantity: Option<i64>,
    pub projected_quantity: Option<i64>,
    pub variance_quantity: Option<i64>,
    pub on_hand_quantity: Option<i64>,
    pub reserved_quantity: Option<i64>,
    pub allocated_quantity: Option<i64>,
    pub held_quantity: Option<i64>,
    pub hold_ledger_quantity: Option<i64>,
    pub overcommitted_quantity: Option<i64>,
    pub severity_quantity: i64,
    pub issue_codes: Vec<String>,
}

pub type InventoryIntegrityPage = CursorPage<InventoryIntegrityIssueResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrity_queries_have_operational_defaults_and_reject_unknown_fields() {
        let journal: InventoryJournalPageRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(journal.sort, InventoryJournalSort::OccurredAt);
        assert_eq!(journal.direction, InventorySortDirection::Descending);
        let issues: InventoryIntegrityPageRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(issues.sort, InventoryIntegritySort::Severity);
        assert!(serde_json::from_str::<InventoryJournalPageRequest>(r#"{"offset":2}"#).is_err());
        assert!(
            serde_json::from_str::<InventoryIntegrityPageRequest>(r#"{"tenant_id":1}"#).is_err()
        );
    }

    #[test]
    fn trace_filters_and_sort_values_are_strictly_typed() {
        let request: InventoryJournalPageRequest = serde_json::from_str(
            r#"{"facility_id":7,"item_batch_id":9,"sort":"net_quantity","direction":"ascending"}"#,
        )
        .unwrap();
        assert_eq!(request.facility_id, Some(7));
        assert_eq!(request.item_batch_id, Some(9));
        assert_eq!(request.sort, InventoryJournalSort::NetQuantity);
        assert_eq!(request.direction, InventorySortDirection::Ascending);
        assert!(
            serde_json::from_str::<InventoryJournalPageRequest>(r#"{"sort":"created"}"#).is_err()
        );
    }

    #[test]
    fn aging_defaults_to_oldest_inventory_and_rejects_unknown_fields() {
        let request: InventoryAgingPageRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(request.sort, InventoryAgingSort::Age);
        assert_eq!(request.direction, InventorySortDirection::Descending);
        let filtered: InventoryAgingPageRequest = serde_json::from_str(
            r#"{"bucket":"due_within_30_days","sort":"expiration","direction":"ascending"}"#,
        )
        .unwrap();
        assert_eq!(filtered.bucket, Some(InventoryAgingBucket::DueWithin30Days));
        assert_eq!(filtered.sort, InventoryAgingSort::Expiration);
        assert!(serde_json::from_str::<InventoryAgingPageRequest>(r#"{"age_days":30}"#).is_err());
    }
}
