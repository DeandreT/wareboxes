use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

/// Pack-session state derived from allocation and carton conservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackSessionStatus {
    Open,
    ReadyToManifest,
}

/// Order states observable during the packing workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackingOrderStatus {
    Packing,
    AwaitingShipment,
}

/// Order states eligible for display in the packing work queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackingQueueOrderStatus {
    AwaitingPacking,
    Packing,
}

/// Positive facility selector for a station-specific packing queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PackingQueueFacilityId(i64);

impl PackingQueueFacilityId {
    pub const fn new(value: i64) -> Result<Self, PackingQueueFacilityIdError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PackingQueueFacilityIdError(value))
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for PackingQueueFacilityId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("packing queue facility id must be positive, got {0}")]
pub struct PackingQueueFacilityIdError(i64);

/// Cursor query for the scoped packing work queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PackingQueuePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<PackingQueueFacilityId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

/// Existing station session facts included for resumable queue entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackingQueueSessionResponse {
    pub session_id: i64,
    pub station_location_id: i64,
    pub station_location_barcode: String,
    pub station_location_name: Option<String>,
    pub status: PackSessionStatus,
    pub started_at: String,
}

/// One order currently eligible to start or resume packing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackingQueueEntryResponse {
    pub order_id: i64,
    pub order_key: String,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub status: PackingQueueOrderStatus,
    pub revision: Revision,
    pub rush: bool,
    pub ship_by: Option<String>,
    pub session: Option<PackingQueueSessionResponse>,
}

/// Cursor page returned by the scoped packing work queue.
pub type PackingQueuePage = CursorPage<PackingQueueEntryResponse>;

/// Strict positive carton weight represented in whole grams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct WeightGrams(i64);

impl WeightGrams {
    pub const fn new(value: i64) -> Result<Self, PackingMeasurementError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PackingMeasurementError::InvalidWeight(value))
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for WeightGrams {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Strict positive carton dimension represented in whole millimeters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DimensionMillimeters(i64);

impl DimensionMillimeters {
    pub const fn new(value: i64) -> Result<Self, PackingMeasurementError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PackingMeasurementError::InvalidDimension(value))
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for DimensionMillimeters {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PackingMeasurementError {
    #[error("carton weight must be a positive number of grams, got {0}")]
    InvalidWeight(i64),
    #[error("carton dimension must be a positive number of millimeters, got {0}")]
    InvalidDimension(i64),
}

/// An all-or-none carton dimension triplet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CartonDimensions {
    pub length_mm: DimensionMillimeters,
    pub width_mm: DimensionMillimeters,
    pub height_mm: DimensionMillimeters,
}

/// Optional measured facts accepted when a carton is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CartonMeasurements {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_grams: Option<WeightGrams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<CartonDimensions>,
}

/// Conserved allocation and carton counts returned after each operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackingProgressResponse {
    pub expected_allocation_count: i64,
    pub packed_allocation_count: i64,
    pub expected_quantity: i64,
    pub packed_quantity: i64,
    pub open_carton_count: i64,
    pub closed_carton_count: i64,
    pub status: PackSessionStatus,
}

/// Starts a station session for an order at its observed revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenPackSessionRequest {
    pub facility_id: i64,
    pub station_location_id: i64,
    pub expected_revision: Revision,
}

/// Creates the single open carton in a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCartonRequest {
    pub carton_barcode: String,
    pub expected_revision: Revision,
}

/// Packs the full server-selected quantity of one immutable picked allocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackPickedAllocationRequest {
    pub inventory_allocation_id: i64,
    pub item_barcode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lot_scan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_scan: Option<String>,
    pub source_license_plate_barcode: String,
    pub carton_barcode: String,
    pub expected_revision: Revision,
}

/// Closes one nonempty carton at the aggregate revision observed by the station.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseCartonRequest {
    pub carton_barcode: String,
    #[serde(default)]
    pub measurements: CartonMeasurements,
    pub expected_revision: Revision,
}

/// Permanently abandons one empty open carton.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoidCartonRequest {
    pub carton_barcode: String,
    pub expected_revision: Revision,
}

/// State-specific fields of one carton in a session read model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PackCartonLifecycleResponse {
    Open,
    Closed {
        measurements: CartonMeasurements,
        closed_by: i64,
        closed_at: String,
    },
    Voided {
        voided_by: i64,
        voided_at: String,
    },
}

/// One carton in a resumable pack session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackCartonResponse {
    pub carton_id: i64,
    pub carton_barcode: String,
    pub lifecycle: PackCartonLifecycleResponse,
    pub content_count: i64,
    pub created_by: i64,
    pub created_at: String,
}

/// State-specific packing disposition of one picked allocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PackAllocationDispositionResponse {
    Available,
    Packed {
        content_id: i64,
        carton_id: i64,
        packed_by: i64,
        packed_at: String,
    },
}

/// One full picked allocation visible at the pack station.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackableAllocationResponse {
    pub inventory_allocation_id: i64,
    pub order_line_id: i64,
    pub inventory_balance_id: i64,
    pub source_location_id: i64,
    pub source_location_barcode: String,
    pub source_location_name: Option<String>,
    pub license_plate_id: i64,
    pub license_plate_barcode: String,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub item_barcodes: Vec<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub quantity: i64,
    pub disposition: PackAllocationDispositionResponse,
}

/// Complete station read model used for first render and reconnect recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackSessionResponse {
    pub session_id: i64,
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub station_location_id: i64,
    pub station_location_barcode: String,
    pub station_location_name: Option<String>,
    pub order_key: String,
    pub revision: Revision,
    pub progress: PackingProgressResponse,
    pub cartons: Vec<PackCartonResponse>,
    pub allocations: Vec<PackableAllocationResponse>,
    pub started_by: i64,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenPackSessionResponse {
    pub session: PackSessionResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCartonResponse {
    pub session_id: i64,
    pub order_id: i64,
    pub carton: PackCartonResponse,
    pub revision: Revision,
    pub progress: PackingProgressResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackPickedAllocationResponse {
    pub content_id: i64,
    pub session_id: i64,
    pub carton_id: i64,
    pub order_id: i64,
    pub order_line_id: i64,
    pub inventory_allocation_id: i64,
    pub inventory_transaction_id: i64,
    pub source_inventory_allocation_id: i64,
    pub destination_inventory_allocation_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_inventory_balance_id: i64,
    pub source_location_id: i64,
    pub destination_location_id: i64,
    pub source_license_plate_id: i64,
    pub destination_license_plate_id: i64,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub quantity: i64,
    pub uom: String,
    pub packed_by: i64,
    pub packed_at: String,
    pub revision: Revision,
    pub progress: PackingProgressResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseCartonResponse {
    pub session_id: i64,
    pub carton_id: i64,
    pub order_id: i64,
    pub lifecycle: PackCartonLifecycleResponse,
    pub order_status: PackingOrderStatus,
    pub revision: Revision,
    pub progress: PackingProgressResponse,
    pub ready_to_manifest: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoidCartonResponse {
    pub session_id: i64,
    pub carton_id: i64,
    pub order_id: i64,
    pub lifecycle: PackCartonLifecycleResponse,
    pub revision: Revision,
    pub progress: PackingProgressResponse,
}

impl fmt::Display for PackSessionStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Open => "open",
            Self::ReadyToManifest => "ready_to_manifest",
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn packing_queue_query_is_bounded_and_strict() {
        let defaulted = serde_json::from_value::<PackingQueuePageRequest>(json!({})).unwrap();
        assert_eq!(defaulted.limit, PageLimit::default());
        assert!(defaulted.cursor.is_none());
        assert!(defaulted.facility_id.is_none());

        assert!(serde_json::from_value::<PackingQueuePageRequest>(json!({"limit": 0})).is_err());
        assert!(serde_json::from_value::<PackingQueuePageRequest>(json!({"limit": 1001})).is_err());
        assert!(
            serde_json::from_value::<PackingQueuePageRequest>(json!({"facility_id": 0})).is_err()
        );
        let scoped =
            serde_json::from_value::<PackingQueuePageRequest>(json!({"facility_id": 8})).unwrap();
        assert_eq!(scoped.facility_id.map(PackingQueueFacilityId::get), Some(8));
        assert!(serde_json::from_value::<PackingQueuePageRequest>(json!({
            "limit": 50,
            "status": "packing"
        }))
        .is_err());
    }

    #[test]
    fn packing_queue_page_preserves_scope_and_session_facts() {
        let page = PackingQueuePage::new(
            vec![PackingQueueEntryResponse {
                order_id: 41,
                order_key: "SO-0041".into(),
                inventory_owner_id: 7,
                inventory_owner_name: "Northwind".into(),
                facility_id: 9,
                facility_name: "Reno DC".into(),
                status: PackingQueueOrderStatus::Packing,
                revision: Revision::new(6).unwrap(),
                rush: true,
                ship_by: Some("2026-08-09T16:00:00Z".into()),
                session: Some(PackingQueueSessionResponse {
                    session_id: 12,
                    station_location_id: 15,
                    station_location_barcode: "PACK-01".into(),
                    station_location_name: Some("Pack station 1".into()),
                    status: PackSessionStatus::Open,
                    started_at: "2026-08-08T20:00:00Z".into(),
                }),
            }],
            Some(
                OpaqueCursor::new("pq1.0000000000000009.0.t8000000000000000.0000000000000029")
                    .unwrap(),
            ),
        );
        let value = serde_json::to_value(page).unwrap();
        assert_eq!(value["items"][0]["status"], "packing");
        assert_eq!(value["items"][0]["inventory_owner_name"], "Northwind");
        assert_eq!(
            value["items"][0]["session"]["station_location_barcode"],
            "PACK-01"
        );
        assert_eq!(
            value["next_cursor"],
            "pq1.0000000000000009.0.t8000000000000000.0000000000000029"
        );
    }

    #[test]
    fn every_mutation_request_is_strict_and_revisioned() {
        assert!(serde_json::from_value::<OpenPackSessionRequest>(json!({
            "facility_id": 1,
            "station_location_id": 2,
            "expected_revision": 3,
            "force": true
        }))
        .is_err());
        assert!(serde_json::from_value::<CreateCartonRequest>(json!({
            "carton_barcode": "CARTON-1",
            "expected_revision": 0
        }))
        .is_err());
        assert!(serde_json::from_value::<VoidCartonRequest>(json!({
            "carton_barcode": "CARTON-1",
            "expected_revision": 4,
            "delete": true
        }))
        .is_err());
        assert!(
            serde_json::from_value::<PackPickedAllocationRequest>(json!({
                "inventory_allocation_id": 4,
                "item_barcode": "SKU-1",
                "source_license_plate_barcode": "TOTE-1",
                "carton_barcode": "CARTON-1",
                "expected_revision": 5,
                "quantity": 2
            }))
            .is_err()
        );
    }

    #[test]
    fn close_measurements_are_optional_but_positive_and_complete() {
        let request = serde_json::from_value::<CloseCartonRequest>(json!({
            "carton_barcode": "CARTON-1",
            "measurements": {
                "weight_grams": 1250,
                "dimensions": {
                    "length_mm": 300,
                    "width_mm": 200,
                    "height_mm": 150
                }
            },
            "expected_revision": 6
        }))
        .unwrap();
        assert_eq!(
            request.measurements.weight_grams.map(WeightGrams::get),
            Some(1250)
        );
        assert!(serde_json::from_value::<CloseCartonRequest>(json!({
            "carton_barcode": "CARTON-1",
            "measurements": {"weight_grams": -1},
            "expected_revision": 6
        }))
        .is_err());
        assert!(serde_json::from_value::<CloseCartonRequest>(json!({
            "carton_barcode": "CARTON-1",
            "measurements": {
                "dimensions": {"length_mm": 300, "width_mm": 200}
            },
            "expected_revision": 6
        }))
        .is_err());
    }

    #[test]
    fn allocation_confirmation_never_accepts_a_caller_quantity() {
        let request = serde_json::from_value::<PackPickedAllocationRequest>(json!({
            "inventory_allocation_id": 4,
            "item_barcode": "SKU-1",
            "lot_scan": "LOT-1",
            "serial_scan": "SERIAL-1",
            "source_license_plate_barcode": "TOTE-1",
            "carton_barcode": "CARTON-1",
            "expected_revision": 5
        }))
        .unwrap();
        assert_eq!(request.inventory_allocation_id, 4);
        assert_eq!(request.lot_scan.as_deref(), Some("LOT-1"));
        assert_eq!(request.serial_scan.as_deref(), Some("SERIAL-1"));
        assert!(
            serde_json::from_value::<PackPickedAllocationRequest>(json!({
                "inventory_allocation_id": 4,
                "item_barcode": "SKU-1",
                "lot": "LOT-1",
                "serial": "SERIAL-1",
                "source_license_plate_barcode": "TOTE-1",
                "carton_barcode": "CARTON-1",
                "expected_revision": 5
            }))
            .is_err()
        );
    }

    #[test]
    fn carton_and_allocation_states_have_state_specific_fields() {
        assert_eq!(
            serde_json::to_value(PackCartonLifecycleResponse::Open).unwrap(),
            json!({"status": "open"})
        );
        assert_eq!(
            serde_json::to_value(PackCartonLifecycleResponse::Voided {
                voided_by: 7,
                voided_at: "2026-08-08T20:00:00Z".into(),
            })
            .unwrap(),
            json!({
                "status": "voided",
                "voided_by": 7,
                "voided_at": "2026-08-08T20:00:00Z"
            })
        );
        assert_eq!(
            serde_json::to_value(PackAllocationDispositionResponse::Packed {
                content_id: 1,
                carton_id: 2,
                packed_by: 3,
                packed_at: "2026-08-08T20:00:00Z".into(),
            })
            .unwrap(),
            json!({
                "state": "packed",
                "content_id": 1,
                "carton_id": 2,
                "packed_by": 3,
                "packed_at": "2026-08-08T20:00:00Z"
            })
        );
    }
}
