use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision, ShipmentStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ShippingQueueFacilityId(i64);

impl ShippingQueueFacilityId {
    pub const fn new(value: i64) -> Result<Self, ShippingQueueFacilityIdError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ShippingQueueFacilityIdError(value))
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ShippingQueueFacilityId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("shipping queue facility id must be positive, got {0}")]
pub struct ShippingQueueFacilityIdError(i64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ShippingQueuePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<ShippingQueueFacilityId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShippingQueueShipmentResponse {
    pub shipment_id: i64,
    pub status: ShipmentStatus,
    pub revision: Revision,
    pub carton_count: i64,
    pub shipped_quantity: i64,
    pub carrier_code: Option<String>,
    pub service_code: Option<String>,
    pub created_at: String,
    pub manifested_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShippingQueueEntryResponse {
    pub order_id: i64,
    pub order_key: String,
    pub order_revision: Revision,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub facility_revision: Revision,
    pub packing_session_id: i64,
    pub rush: bool,
    pub ship_by: Option<String>,
    pub origin_ready: bool,
    pub destination_ready: bool,
    pub shipment: Option<ShippingQueueShipmentResponse>,
}

pub type ShippingQueuePage = CursorPage<ShippingQueueEntryResponse>;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn shipping_queue_query_is_bounded_scoped_and_strict() {
        let defaulted = serde_json::from_value::<ShippingQueuePageRequest>(json!({})).unwrap();
        assert_eq!(defaulted.limit.get(), 100);
        assert!(defaulted.facility_id.is_none());
        assert!(serde_json::from_value::<ShippingQueuePageRequest>(json!({"limit": 0})).is_err());
        assert!(
            serde_json::from_value::<ShippingQueuePageRequest>(json!({"facility_id": 0})).is_err()
        );
        assert!(serde_json::from_value::<ShippingQueuePageRequest>(json!({
            "facility_id": 8,
            "unexpected": true
        }))
        .is_err());
    }

    #[test]
    fn queue_entry_preserves_blockers_and_resumable_shipment_state() {
        let page = ShippingQueuePage::new(
            vec![ShippingQueueEntryResponse {
                order_id: 4,
                order_key: "SO-4".into(),
                order_revision: Revision::new(12).unwrap(),
                inventory_owner_id: 5,
                inventory_owner_name: "Alpine Sporting Goods".into(),
                facility_id: 6,
                facility_name: "Reno DC".into(),
                facility_revision: Revision::new(3).unwrap(),
                packing_session_id: 7,
                rush: true,
                ship_by: Some("2026-08-09T04:00:00Z".into()),
                origin_ready: true,
                destination_ready: true,
                shipment: Some(ShippingQueueShipmentResponse {
                    shipment_id: 8,
                    status: ShipmentStatus::AwaitingManifest,
                    revision: Revision::new(1).unwrap(),
                    carton_count: 2,
                    shipped_quantity: 5,
                    carrier_code: None,
                    service_code: None,
                    created_at: "2026-08-08T21:00:00Z".into(),
                    manifested_at: None,
                }),
            }],
            None,
        );
        let entry = &page.items[0];
        assert!(entry.rush && entry.origin_ready && entry.destination_ready);
        assert_eq!(
            entry
                .shipment
                .as_ref()
                .map(|shipment| shipment.carton_count),
            Some(2)
        );
    }
}
