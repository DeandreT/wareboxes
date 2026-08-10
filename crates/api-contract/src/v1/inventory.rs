use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use super::{CursorPage, InventorySortDirection, OpaqueCursor, PageLimit};

/// Maximum accepted inventory-balance search length.
pub const MAX_INVENTORY_BALANCE_QUERY_LENGTH: usize = 200;

/// Validated free-text search for inventory-balance discovery.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct InventoryBalanceSearchQuery(String);

impl InventoryBalanceSearchQuery {
    pub fn new(value: impl Into<String>) -> Result<Self, InventoryBalanceSearchQueryError> {
        let value = value.into();
        if value.is_empty() {
            return Err(InventoryBalanceSearchQueryError::Empty);
        }
        if value.trim() != value {
            return Err(InventoryBalanceSearchQueryError::NotTrimmed);
        }
        if value.chars().count() > MAX_INVENTORY_BALANCE_QUERY_LENGTH {
            return Err(InventoryBalanceSearchQueryError::TooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(InventoryBalanceSearchQueryError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for InventoryBalanceSearchQuery {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for InventoryBalanceSearchQuery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for InventoryBalanceSearchQuery {
    type Err = InventoryBalanceSearchQueryError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for InventoryBalanceSearchQuery {
    type Error = InventoryBalanceSearchQueryError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for InventoryBalanceSearchQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InventoryBalanceSearchQueryError {
    #[error("inventory balance query cannot be empty")]
    Empty,
    #[error("inventory balance query must be trimmed")]
    NotTrimmed,
    #[error(
        "inventory balance query cannot exceed {MAX_INVENTORY_BALANCE_QUERY_LENGTH} characters"
    )]
    TooLong,
    #[error("inventory balance query cannot contain control characters")]
    InvalidCharacter,
}

/// Public inventory disposition for a balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryBalanceStatus {
    Available,
    Hold,
    Damaged,
    Quarantine,
}

/// Quantities projected for one inventory balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryQuantity {
    pub on_hand: i64,
    pub reserved: i64,
    pub held: i64,
    pub available: i64,
}

/// Version 1 inventory-balance response without persistence metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryBalanceResponse {
    pub id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub uom: String,
    pub status: InventoryBalanceStatus,
    pub quantity: InventoryQuantity,
}

/// Cursor query for the version 1 inventory-balance collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryBalancePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<InventoryBalanceSearchQuery>,
    #[serde(default)]
    pub sort: InventoryBalanceSort,
    #[serde(default = "default_balance_direction")]
    pub direction: InventorySortDirection,
    #[serde(default)]
    pub movable_only: bool,
}

impl Default for InventoryBalancePageRequest {
    fn default() -> Self {
        Self {
            cursor: None,
            limit: PageLimit::default(),
            query: None,
            sort: InventoryBalanceSort::Position,
            direction: default_balance_direction(),
            movable_only: false,
        }
    }
}

const fn default_balance_direction() -> InventorySortDirection {
    InventorySortDirection::Ascending
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InventoryBalanceSort {
    #[default]
    Position,
    Facility,
    Client,
    Location,
    Item,
    Tracking,
    LicensePlate,
    Status,
    OnHand,
    Reserved,
    Held,
    Available,
}

/// Cursor page returned by the version 1 inventory-balance collection.
pub type InventoryBalancePage = CursorPage<InventoryBalanceResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_balance_response_excludes_persistence_metadata() {
        let response = InventoryBalanceResponse {
            id: 11,
            inventory_owner_id: 22,
            inventory_owner_name: "Acme".into(),
            facility_id: 33,
            facility_name: Some("North DC".into()),
            location_id: 44,
            location_name: Some("Reserve 01".into()),
            location_barcode: Some("R-01".into()),
            license_plate_id: None,
            license_plate_barcode: None,
            item_batch_id: 55,
            item_id: 66,
            item_description: Some("Widget".into()),
            primary_sku: Some("WIDGET-EA".into()),
            lot: Some("LOT-1".into()),
            serial: None,
            uom: "each".into(),
            status: InventoryBalanceStatus::Available,
            quantity: InventoryQuantity {
                on_hand: 12,
                reserved: 2,
                held: 1,
                available: 9,
            },
        };

        let value = serde_json::to_value(response).unwrap();
        assert!(value.get("tenant_id").is_none());
        assert!(value.get("deleted").is_none());
        assert!(value.get("created").is_none());
        assert!(value.get("modified").is_none());
        assert_eq!(value["status"], "available");
        assert_eq!(value["quantity"]["available"], 9);
    }

    #[test]
    fn inventory_balance_page_request_uses_validated_defaults() {
        let request = serde_json::from_str::<InventoryBalancePageRequest>("{}").unwrap();
        assert!(request.cursor.is_none());
        assert_eq!(request.limit, PageLimit::default());
        assert!(request.query.is_none());
        assert_eq!(request.sort, InventoryBalanceSort::Position);
        assert_eq!(request.direction, InventorySortDirection::Ascending);
        assert!(!request.movable_only);
        let movable = serde_json::from_str::<InventoryBalancePageRequest>(
            r#"{"sort":"available","movable_only":true}"#,
        )
        .unwrap();
        assert_eq!(movable.sort, InventoryBalanceSort::Available);
        assert!(movable.movable_only);
        assert!(serde_json::from_str::<InventoryBalancePageRequest>(r#"{"limit":1001}"#).is_err());
    }

    #[test]
    fn inventory_balance_search_queries_are_bounded_and_unambiguous() {
        let request = serde_json::from_str::<InventoryBalancePageRequest>(
            r#"{"query":"Reserve A / SKU-42"}"#,
        )
        .unwrap();
        assert_eq!(
            request
                .query
                .as_ref()
                .map(InventoryBalanceSearchQuery::as_str),
            Some("Reserve A / SKU-42")
        );
        assert!(serde_json::from_str::<InventoryBalancePageRequest>(r#"{"query":""}"#).is_err());
        assert!(
            serde_json::from_str::<InventoryBalancePageRequest>(r#"{"query":" padded "}"#).is_err()
        );
        assert!(serde_json::from_str::<InventoryBalancePageRequest>(
            &serde_json::json!({
                "query": "x".repeat(MAX_INVENTORY_BALANCE_QUERY_LENGTH + 1)
            })
            .to_string()
        )
        .is_err());
        assert!(
            serde_json::from_str::<InventoryBalancePageRequest>(r#"{"query":"line\nbreak"}"#)
                .is_err()
        );
    }
}
