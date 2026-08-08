use serde::{Deserialize, Serialize};

use super::Revision;

/// Parameters for an optimistic, replay-safe waveless order release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseOrderRequest {
    pub facility_id: i64,
    pub destination_location_id: i64,
    pub expected_revision: Revision,
}

/// Order state after a successful waveless release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderReleaseStatus {
    Processing,
}

/// Replay-stable result of creating allocation-backed pick tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseOrderResponse {
    pub release_id: i64,
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub destination_location_id: i64,
    pub status: OrderReleaseStatus,
    pub revision: Revision,
    pub allocation_count: i64,
    pub pick_task_count: i64,
    pub released_quantity: i64,
    pub released_at: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn release_request_is_strict_revisioned_and_location_directed() {
        let request = serde_json::from_value::<ReleaseOrderRequest>(json!({
            "facility_id": 3,
            "destination_location_id": 4,
            "expected_revision": 2
        }))
        .unwrap();
        assert_eq!(request.facility_id, 3);
        assert_eq!(request.destination_location_id, 4);
        assert_eq!(request.expected_revision.get(), 2);

        assert!(serde_json::from_value::<ReleaseOrderRequest>(json!({
            "facility_id": 3,
            "destination_location_id": 4,
            "expected_revision": 2,
            "force": true
        }))
        .is_err());
        assert!(serde_json::from_value::<ReleaseOrderRequest>(json!({
            "facility_id": 3,
            "destination_location_id": 4,
            "expected_revision": 0
        }))
        .is_err());
    }

    #[test]
    fn release_response_has_replay_stable_work_totals() {
        let response = ReleaseOrderResponse {
            release_id: 10,
            order_id: 11,
            inventory_owner_id: 12,
            facility_id: 13,
            destination_location_id: 14,
            status: OrderReleaseStatus::Processing,
            revision: Revision::new(3).unwrap(),
            allocation_count: 2,
            pick_task_count: 2,
            released_quantity: 7,
            released_at: "2026-08-08T20:00:00Z".into(),
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "release_id": 10,
                "order_id": 11,
                "inventory_owner_id": 12,
                "facility_id": 13,
                "destination_location_id": 14,
                "status": "processing",
                "revision": 3,
                "allocation_count": 2,
                "pick_task_count": 2,
                "released_quantity": 7,
                "released_at": "2026-08-08T20:00:00Z"
            })
        );
    }
}
