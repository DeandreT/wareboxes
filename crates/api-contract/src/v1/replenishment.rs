use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

const MAX_REPLENISHMENT_UOM_LENGTH: usize = 32;

/// Canonical nonempty set of explicit reserve-source location identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ReplenishmentReserveSourceLocationIds(Vec<i64>);

impl ReplenishmentReserveSourceLocationIds {
    pub fn new(mut location_ids: Vec<i64>) -> Result<Self, String> {
        if location_ids.iter().any(|location_id| *location_id <= 0) {
            return Err("reserve source location IDs must be positive".into());
        }
        location_ids.sort_unstable();
        location_ids.dedup();
        if location_ids.is_empty() {
            return Err("at least one reserve source location is required".into());
        }
        Ok(Self(location_ids))
    }

    pub fn as_slice(&self) -> &[i64] {
        &self.0
    }

    pub fn into_inner(self) -> Vec<i64> {
        self.0
    }
}

impl<'de> Deserialize<'de> for ReplenishmentReserveSourceLocationIds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Vec::<i64>::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentPolicyStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentPlanningOutcome {
    NotNeeded,
    InsufficientReserve,
    PartiallyPlanned,
    FullyPlanned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentWorkStatus {
    Pending,
    Claimed,
    Completed,
    Cancelled,
}

/// Creates or replaces the single active policy for the supplied natural key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureReplenishmentPolicyRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub pick_face_location_id: i64,
    pub minimum_quantity: i64,
    pub target_quantity: i64,
    pub reserve_source_location_ids: ReplenishmentReserveSourceLocationIds,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

impl<'de> Deserialize<'de> for ConfigureReplenishmentPolicyRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawRequest {
            inventory_owner_id: i64,
            facility_id: i64,
            item_id: i64,
            uom: String,
            pick_face_location_id: i64,
            minimum_quantity: i64,
            target_quantity: i64,
            reserve_source_location_ids: ReplenishmentReserveSourceLocationIds,
            #[serde(default)]
            expected_revision: Option<Revision>,
        }

        let raw = RawRequest::deserialize(deserializer)?;
        for (name, value) in [
            ("inventory_owner_id", raw.inventory_owner_id),
            ("facility_id", raw.facility_id),
            ("item_id", raw.item_id),
            ("pick_face_location_id", raw.pick_face_location_id),
        ] {
            if value <= 0 {
                return Err(D::Error::custom(format!("{name} must be positive")));
            }
        }
        if raw.uom.is_empty()
            || raw.uom.trim() != raw.uom
            || raw.uom.chars().count() > MAX_REPLENISHMENT_UOM_LENGTH
            || raw.uom.chars().any(char::is_control)
        {
            return Err(D::Error::custom("uom is invalid"));
        }
        if raw.minimum_quantity < 0 || raw.target_quantity <= raw.minimum_quantity {
            return Err(D::Error::custom(
                "minimum_quantity must be nonnegative and target_quantity must be greater",
            ));
        }
        if raw
            .reserve_source_location_ids
            .as_slice()
            .contains(&raw.pick_face_location_id)
        {
            return Err(D::Error::custom(
                "pick face cannot also be a reserve source location",
            ));
        }

        Ok(Self {
            inventory_owner_id: raw.inventory_owner_id,
            facility_id: raw.facility_id,
            item_id: raw.item_id,
            uom: raw.uom,
            pick_face_location_id: raw.pick_face_location_id,
            minimum_quantity: raw.minimum_quantity,
            target_quantity: raw.target_quantity,
            reserve_source_location_ids: raw.reserve_source_location_ids,
            expected_revision: raw.expected_revision,
        })
    }
}

/// Replay-stable active version returned after configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureReplenishmentPolicyResponse {
    pub policy_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub pick_face_location_id: i64,
    pub minimum_quantity: i64,
    pub target_quantity: i64,
    pub reserve_source_location_ids: ReplenishmentReserveSourceLocationIds,
    pub status: ReplenishmentPolicyStatus,
    pub previous_revision: Option<Revision>,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetireReplenishmentPolicyRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetireReplenishmentPolicyResponse {
    pub policy_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub pick_face_location_id: i64,
    pub revision: Revision,
    pub status: ReplenishmentPolicyStatus,
    pub retired_by: i64,
    pub retired_at: String,
}

/// Policy ID is supplied by the route; no client quantity projection is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanReplenishmentRequest {
    pub expected_policy_revision: Revision,
}

/// Quantity facts observed atomically by the server during planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentPlanningSnapshotResponse {
    pub pick_face_free: i64,
    pub active_inbound: i64,
    pub projected_free: i64,
    pub unallocated_demand: i64,
    pub reserve_free: i64,
}

/// One deterministic FEFO source task, ordered by the one-based sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentPlannedWorkResponse {
    pub work_id: i64,
    pub sequence: u32,
    pub source_inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub source_location_id: i64,
    pub source_location_barcode: String,
    pub source_location_name: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<String>,
    pub source_received_at: String,
    pub quantity: i64,
}

/// Replay-stable supervisor planning result, including zero-work outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanReplenishmentResponse {
    pub plan_id: i64,
    pub policy_id: i64,
    pub policy_revision: Revision,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub pick_face_location_id: i64,
    pub snapshot: ReplenishmentPlanningSnapshotResponse,
    pub required_level: i64,
    pub target_gap: i64,
    pub planned_quantity: i64,
    pub remaining_quantity: i64,
    pub outcome: ReplenishmentPlanningOutcome,
    pub work: Vec<ReplenishmentPlannedWorkResponse>,
    pub planned_by: i64,
    pub planned_at: String,
}

/// Scoped filters for active policies, independent of whether work exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentPolicyPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick_face_location_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentPolicyLatestPlanResponse {
    pub plan_id: i64,
    pub outcome: ReplenishmentPlanningOutcome,
    pub planned_quantity: i64,
    pub remaining_quantity: i64,
    pub planned_by: i64,
    pub planned_at: String,
}

/// Manager policy row with live decision facts and planning history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentPolicyReadinessEntryResponse {
    pub policy_id: i64,
    pub revision: Revision,
    pub status: ReplenishmentPolicyStatus,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub uom: String,
    pub pick_face: ReplenishmentLocationResponse,
    pub minimum_quantity: i64,
    pub target_quantity: i64,
    pub reserve_source_location_ids: ReplenishmentReserveSourceLocationIds,
    pub snapshot: ReplenishmentPlanningSnapshotResponse,
    pub required_level: i64,
    pub target_gap: i64,
    pub suggested_outcome: ReplenishmentPlanningOutcome,
    pub suggested_quantity: i64,
    pub suggested_remaining_quantity: i64,
    pub active_work_count: i64,
    pub active_work_quantity: i64,
    pub latest_plan: Option<ReplenishmentPolicyLatestPlanResponse>,
}

pub type ReplenishmentPolicyPage = CursorPage<ReplenishmentPolicyReadinessEntryResponse>;

/// Queue filters. Omitted status returns open pending and claimed work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentQueuePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick_face_location_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ReplenishmentWorkStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentLocationResponse {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

/// Dense supervisor queue row with enough source identity to investigate work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentQueueEntryResponse {
    pub work_id: i64,
    pub plan_id: i64,
    pub policy_id: i64,
    pub policy_revision: Revision,
    pub status: ReplenishmentWorkStatus,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub sequence: u32,
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
    pub item_batch_id: i64,
    pub source_location: ReplenishmentLocationResponse,
    pub destination_pick_face: ReplenishmentLocationResponse,
    pub claimed_by: Option<i64>,
    pub lease_expires_at: Option<String>,
    pub due_at: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

pub type ReplenishmentQueuePage = CursorPage<ReplenishmentQueueEntryResponse>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimNextReplenishmentWorkRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimReplenishmentWorkByIdRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatReplenishmentClaimRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentClaimReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    SourceBlocked,
    DestinationBlocked,
    InventoryMismatch,
    SafetyIssue,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseReplenishmentClaimRequest {
    pub reason: ReplenishmentClaimReleaseReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplenishmentWorkCancellationReason {
    DemandRemoved,
    PolicyReconfigured,
    SourceUnavailable,
    DestinationUnavailable,
    PlanningError,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelReplenishmentWorkRequest {
    pub reason: ReplenishmentWorkCancellationReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentWorkCancellationResponse {
    pub cancellation_id: i64,
    pub work_id: i64,
    pub plan_id: i64,
    pub policy_id: i64,
    pub policy_revision: Revision,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub pick_face_location_id: i64,
    pub source_inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub quantity: i64,
    pub previous_status: ReplenishmentWorkStatus,
    pub previous_assigned_user_id: Option<i64>,
    pub status: ReplenishmentWorkStatus,
    pub reason: ReplenishmentWorkCancellationReason,
    pub note: Option<String>,
    pub cancelled_by: i64,
    pub cancelled_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentClaimHeartbeatResponse {
    pub work_id: i64,
    pub heartbeat_at: String,
    pub lease_expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentClaimReleaseResponse {
    pub work_id: i64,
    pub status: ReplenishmentWorkStatus,
    pub released_at: String,
    pub release_count: i64,
    pub reason: ReplenishmentClaimReleaseReason,
    pub note: Option<String>,
}

/// Scanner-ready loose-stock work. Planned lot/serial determine required scans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentClaimResponse {
    pub work_id: i64,
    pub plan_id: i64,
    pub policy_id: i64,
    pub policy_revision: Revision,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub sequence: u32,
    pub priority: i64,
    pub instructions: Option<String>,
    pub due_at: Option<String>,
    pub lease_expires_at: String,
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
    pub source_location: ReplenishmentLocationResponse,
    pub destination_pick_face: ReplenishmentLocationResponse,
}

/// Exact physical scans; quantity and stock identities remain server authoritative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmReplenishmentWorkRequest {
    pub source_location_barcode: String,
    pub item_barcode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lot_scan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_scan: Option<String>,
    pub destination_pick_face_barcode: String,
}

/// Replay-stable result of the inventory movement and terminal work transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplenishmentConfirmationResponse {
    pub confirmation_id: i64,
    pub work_id: i64,
    pub plan_id: i64,
    pub policy_id: i64,
    pub inventory_transaction_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub item_id: i64,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub source_location_id: i64,
    pub destination_pick_face_location_id: i64,
    pub quantity: i64,
    pub work_status: ReplenishmentWorkStatus,
    pub confirmed_by: i64,
    pub confirmed_at: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn configure_request_is_strict_versioned_and_canonicalizes_sources() {
        let request = serde_json::from_value::<ConfigureReplenishmentPolicyRequest>(json!({
            "inventory_owner_id": 2,
            "facility_id": 3,
            "item_id": 4,
            "uom": "each",
            "pick_face_location_id": 20,
            "minimum_quantity": 5,
            "target_quantity": 20,
            "reserve_source_location_ids": [12, 10, 12]
        }))
        .unwrap();
        assert_eq!(request.reserve_source_location_ids.as_slice(), &[10, 12]);
        assert!(request.expected_revision.is_none());
        assert!(
            serde_json::from_value::<ConfigureReplenishmentPolicyRequest>(json!({
                "inventory_owner_id": 2,
                "facility_id": 3,
                "item_id": 4,
                "uom": "each",
                "pick_face_location_id": 20,
                "minimum_quantity": 5,
                "target_quantity": 5,
                "reserve_source_location_ids": [10]
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ConfigureReplenishmentPolicyRequest>(json!({
                "inventory_owner_id": 2,
                "facility_id": 3,
                "item_id": 4,
                "uom": "each",
                "pick_face_location_id": 20,
                "minimum_quantity": 5,
                "target_quantity": 20,
                "reserve_source_location_ids": [10],
                "active": true
            }))
            .is_err()
        );
    }

    #[test]
    fn retirement_and_planning_requests_accept_only_revision_preconditions() {
        assert_eq!(
            serde_json::from_value::<RetireReplenishmentPolicyRequest>(json!({
                "expected_revision": 4
            }))
            .unwrap()
            .expected_revision
            .get(),
            4
        );
        assert!(serde_json::from_value::<PlanReplenishmentRequest>(json!({
            "policy_id": 2,
            "expected_policy_revision": 4
        }))
        .is_err());
        assert_eq!(
            serde_json::from_value::<PlanReplenishmentRequest>(json!({
                "expected_policy_revision": 4
            }))
            .unwrap()
            .expected_policy_revision
            .get(),
            4
        );
    }

    #[test]
    fn queue_is_cursor_bounded_and_claim_lifecycle_is_strict() {
        let query = serde_json::from_value::<ReplenishmentQueuePageRequest>(json!({
            "facility_id": 3,
            "status": "claimed",
            "limit": 25
        }))
        .unwrap();
        assert_eq!(query.limit.get(), 25);
        assert_eq!(query.status, Some(ReplenishmentWorkStatus::Claimed));
        assert!(
            serde_json::from_value::<ReplenishmentQueuePageRequest>(json!({
                "limit": 0
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ClaimNextReplenishmentWorkRequest>(json!({
                "facility_id": 3
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<HeartbeatReplenishmentClaimRequest>(json!({
                "lease_seconds": 600
            }))
            .is_err()
        );
    }

    #[test]
    fn policy_readiness_page_is_independent_of_execution_work() {
        let page = ReplenishmentPolicyPage::new(
            vec![ReplenishmentPolicyReadinessEntryResponse {
                policy_id: 1,
                revision: Revision::new(2).unwrap(),
                status: ReplenishmentPolicyStatus::Active,
                inventory_owner_id: 3,
                inventory_owner_name: "Alpine".into(),
                facility_id: 4,
                facility_name: "Reno DC".into(),
                item_id: 5,
                item_description: Some("Widget".into()),
                primary_sku: Some("WIDGET-EA".into()),
                uom: "each".into(),
                pick_face: ReplenishmentLocationResponse {
                    location_id: 6,
                    barcode: "PICK-01".into(),
                    name: Some("Forward pick 01".into()),
                },
                minimum_quantity: 5,
                target_quantity: 20,
                reserve_source_location_ids: ReplenishmentReserveSourceLocationIds::new(vec![7])
                    .unwrap(),
                snapshot: ReplenishmentPlanningSnapshotResponse {
                    pick_face_free: 2,
                    active_inbound: 0,
                    projected_free: 2,
                    unallocated_demand: 4,
                    reserve_free: 18,
                },
                required_level: 20,
                target_gap: 18,
                suggested_outcome: ReplenishmentPlanningOutcome::FullyPlanned,
                suggested_quantity: 18,
                suggested_remaining_quantity: 0,
                active_work_count: 0,
                active_work_quantity: 0,
                latest_plan: None,
            }],
            None,
        );

        assert_eq!(page.items[0].active_work_count, 0);
        assert!(page.items[0].latest_plan.is_none());
    }

    #[test]
    fn confirmation_has_required_scans_and_rejects_quantity_or_license_plate() {
        let request = serde_json::from_value::<ConfirmReplenishmentWorkRequest>(json!({
            "source_location_barcode": "RES-01",
            "item_barcode": "SKU-1",
            "lot_scan": "LOT-1",
            "destination_pick_face_barcode": "PICK-01"
        }))
        .unwrap();
        assert_eq!(request.lot_scan.as_deref(), Some("LOT-1"));
        assert!(
            serde_json::from_value::<ConfirmReplenishmentWorkRequest>(json!({
                "source_location_barcode": "RES-01",
                "item_barcode": "SKU-1",
                "destination_pick_face_barcode": "PICK-01",
                "quantity": 8
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ConfirmReplenishmentWorkRequest>(json!({
                "source_location_barcode": "RES-01",
                "item_barcode": "SKU-1",
                "destination_pick_face_barcode": "PICK-01",
                "license_plate_barcode": "LP-1"
            }))
            .is_err()
        );
    }

    #[test]
    fn cancellation_request_is_strict_and_requires_typed_reason() {
        let request = serde_json::from_value::<CancelReplenishmentWorkRequest>(json!({
            "reason": "source_unavailable",
            "note": "reserve aisle is inaccessible"
        }))
        .unwrap();
        assert_eq!(
            request.reason,
            ReplenishmentWorkCancellationReason::SourceUnavailable
        );
        assert!(
            serde_json::from_value::<CancelReplenishmentWorkRequest>(json!({
                "reason": "source_unavailable",
                "quantity": 5
            }))
            .is_err()
        );
    }

    #[test]
    fn planning_outcomes_and_snapshot_fields_have_stable_wire_names() {
        let value = serde_json::to_value(PlanReplenishmentResponse {
            plan_id: 1,
            policy_id: 2,
            policy_revision: Revision::new(3).unwrap(),
            inventory_owner_id: 4,
            facility_id: 5,
            item_id: 6,
            uom: "each".into(),
            pick_face_location_id: 7,
            snapshot: ReplenishmentPlanningSnapshotResponse {
                pick_face_free: 2,
                active_inbound: 3,
                projected_free: 5,
                unallocated_demand: 8,
                reserve_free: 10,
            },
            required_level: 20,
            target_gap: 15,
            planned_quantity: 10,
            remaining_quantity: 5,
            outcome: ReplenishmentPlanningOutcome::PartiallyPlanned,
            work: Vec::new(),
            planned_by: 8,
            planned_at: "2026-08-08T12:00:00Z".into(),
        })
        .unwrap();

        assert_eq!(value["outcome"], "partially_planned");
        assert_eq!(value["snapshot"]["projected_free"], 5);
        assert_eq!(value["target_gap"], 15);
    }
}
