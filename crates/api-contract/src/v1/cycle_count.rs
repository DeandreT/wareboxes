use serde::{Deserialize, Serialize};

use super::{CursorPage, InventoryBalanceStatus, OpaqueCursor, PageLimit, Revision};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCycleCountTaskRequest {
    pub inventory_balance_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCycleCountTaskResponse {
    pub task_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountWorkStatus {
    Pending,
    Claimed,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountCandidateSort {
    #[default]
    LastCounted,
    Client,
    Facility,
    Location,
    Item,
    Quantity,
    InventoryStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountWorkSort {
    Priority,
    #[default]
    CreatedAt,
    Client,
    Facility,
    Location,
    Item,
    Quantity,
    Variance,
    Status,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountSortDirection {
    #[default]
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CycleCountCandidatePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_status: Option<InventoryBalanceStatus>,
    #[serde(default)]
    pub sort: CycleCountCandidateSort,
    #[serde(default)]
    pub direction: CycleCountSortDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CycleCountWorkPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<CycleCountWorkStatus>,
    #[serde(default)]
    pub sort: CycleCountWorkSort,
    #[serde(default)]
    pub direction: CycleCountSortDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleCountQuantityResponse {
    pub on_hand: i64,
    pub reserved: i64,
    pub held: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleCountCandidateResponse {
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub location: CycleCountLocation,
    pub item: CycleCountItem,
    pub stock: CycleCountStock,
    pub quantity: CycleCountQuantityResponse,
    pub last_counted_at: Option<String>,
    pub last_variance_quantity: Option<i64>,
}

pub type CycleCountCandidatePage = CursorPage<CycleCountCandidateResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleCountWorkResponse {
    pub task_id: i64,
    pub status: CycleCountWorkStatus,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub location: CycleCountLocation,
    pub item: CycleCountItem,
    pub stock: CycleCountStock,
    pub current_quantity: Option<CycleCountQuantityResponse>,
    pub system_quantity: Option<CycleCountQuantityResponse>,
    pub counted_quantity: Option<i64>,
    pub variance_quantity: Option<i64>,
    pub inventory_transaction_id: Option<i64>,
    pub priority: i64,
    pub note: Option<String>,
    pub assigned_user_id: Option<i64>,
    pub lease_expires_at: Option<String>,
    pub due_at: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub confirmed_by: Option<i64>,
    pub confirmed_at: Option<String>,
}

pub type CycleCountWorkPage = CursorPage<CycleCountWorkResponse>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimNextCycleCountRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimCycleCountByIdRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatCycleCountClaimRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountClaimReleaseReason {
    WorkInterrupted,
    EquipmentUnavailable,
    SafetyIssue,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseCycleCountClaimRequest {
    pub reason: CycleCountClaimReleaseReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleCountClaimHeartbeatResponse {
    pub task_id: i64,
    pub heartbeat_at: String,
    pub lease_expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleCountClaimReleaseResponse {
    pub task_id: i64,
    pub released_at: String,
    pub release_count: i64,
    pub reason: CycleCountClaimReleaseReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleCountLocation {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleCountItem {
    pub item_id: i64,
    pub description: Option<String>,
    pub barcodes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleCountStock {
    pub inventory_balance_id: i64,
    pub license_plate_barcode: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<String>,
    pub serial: Option<String>,
    pub inventory_status: InventoryBalanceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleCountClaimResponse {
    pub task_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub priority: i64,
    pub instructions: Option<String>,
    pub due_at: Option<String>,
    pub lease_expires_at: String,
    pub location: CycleCountLocation,
    pub item: CycleCountItem,
    pub stock: CycleCountStock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmCycleCountRequest {
    pub location_barcode: String,
    pub item_barcode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_plate_barcode: Option<String>,
    pub counted_quantity: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleCountConfirmationResponse {
    pub task_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub location_id: i64,
    pub inventory_balance_id: i64,
    pub counted_quantity: i64,
    pub variance_quantity: i64,
    pub inventory_transaction_id: Option<i64>,
    pub disposition: CycleCountDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variance_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variance_revision: Option<Revision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_recount_task_id: Option<i64>,
    pub confirmed_by: i64,
    pub confirmed_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountDisposition {
    Posted,
    RecountRequired,
    ApprovalRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountVarianceStatus {
    AwaitingRecount,
    AwaitingApproval,
    Posted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountVarianceDecision {
    ApproveAdjustment,
    RequestRecount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCountVarianceReason {
    VerifiedPhysicalCount,
    PackagingOrUomIssue,
    ReceivingOrShippingTiming,
    SuspectedMiscount,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureCycleCountPolicyRequest {
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub absolute_tolerance_quantity: i64,
    pub percentage_tolerance_basis_points: u32,
    pub automatic_recount_limit: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureCycleCountPolicyResponse {
    pub policy_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub absolute_tolerance_quantity: i64,
    pub percentage_tolerance_basis_points: u32,
    pub automatic_recount_limit: u16,
    pub previous_revision: Option<Revision>,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CycleCountPolicyPageRequest {
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
pub struct CycleCountPolicyResponse {
    pub policy_id: i64,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub absolute_tolerance_quantity: i64,
    pub percentage_tolerance_basis_points: u32,
    pub automatic_recount_limit: u16,
    pub revision: Revision,
    pub configured_by: i64,
    pub configured_at: String,
}

pub type CycleCountPolicyPage = CursorPage<CycleCountPolicyResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CycleCountVariancePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<CycleCountVarianceStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleCountVarianceStockResponse {
    pub inventory_balance_id: i64,
    pub location_id: i64,
    pub location_barcode: String,
    pub location_name: Option<String>,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub license_plate_barcode: Option<String>,
    pub uom: String,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub inventory_status: InventoryBalanceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleCountVarianceResponse {
    pub variance_id: i64,
    pub revision: Revision,
    pub status: CycleCountVarianceStatus,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub stock: CycleCountVarianceStockResponse,
    pub policy_id: i64,
    pub policy_revision: Revision,
    pub absolute_tolerance_quantity: i64,
    pub percentage_tolerance_basis_points: u32,
    pub automatic_recount_limit: u16,
    pub latest_task_id: i64,
    pub latest_attempt_sequence: u16,
    pub automatic_recounts_used: u16,
    pub system_quantity: i64,
    pub counted_quantity: i64,
    pub variance_quantity: i64,
    pub allowed_variance_quantity: i64,
    pub inventory_transaction_id: Option<i64>,
    pub created_at: String,
    pub modified_at: String,
}

pub type CycleCountVariancePage = CursorPage<CycleCountVarianceResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideCycleCountVarianceRequest {
    pub expected_revision: Revision,
    pub decision: CycleCountVarianceDecision,
    pub reason: CycleCountVarianceReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideCycleCountVarianceResponse {
    pub decision_id: i64,
    pub variance_id: i64,
    pub previous_status: CycleCountVarianceStatus,
    pub status: CycleCountVarianceStatus,
    pub previous_revision: Revision,
    pub revision: Revision,
    pub disposition: CycleCountDisposition,
    pub next_task_id: Option<i64>,
    pub inventory_transaction_id: Option<i64>,
    pub decided_by: i64,
    pub decided_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_requires_scans_and_never_accepts_expected_quantity() {
        let request = serde_json::from_str::<ConfirmCycleCountRequest>(
            r#"{"location_barcode":"A-01","item_barcode":"SKU-1","counted_quantity":7}"#,
        )
        .unwrap();
        assert_eq!(request.counted_quantity, 7);
        assert!(serde_json::from_str::<ConfirmCycleCountRequest>(
            r#"{"location_barcode":"A-01","item_barcode":"SKU-1","counted_quantity":7,"expected_quantity":7}"#
        )
        .is_err());
    }

    #[test]
    fn claim_does_not_disclose_expected_quantity() {
        let claim = CycleCountClaimResponse {
            task_id: 1,
            inventory_owner_id: 2,
            facility_id: 3,
            priority: 90,
            instructions: None,
            due_at: None,
            lease_expires_at: "2026-07-30T01:00:00Z".into(),
            location: CycleCountLocation {
                location_id: 4,
                barcode: "A-01".into(),
                name: None,
            },
            item: CycleCountItem {
                item_id: 5,
                description: Some("Widget".into()),
                barcodes: vec!["SKU-1".into()],
            },
            stock: CycleCountStock {
                inventory_balance_id: 6,
                license_plate_barcode: None,
                uom: "EA".into(),
                lot: None,
                expiration: None,
                serial: None,
                inventory_status: InventoryBalanceStatus::Available,
            },
        };
        let value = serde_json::to_value(claim).unwrap();
        assert!(value.get("expected_quantity").is_none());
        assert!(value["stock"].get("quantity").is_none());
    }

    #[test]
    fn supervisor_pages_are_strict_and_sortable() {
        let request = serde_json::from_str::<CycleCountCandidatePageRequest>(
            r#"{"inventory_status":"quarantine","sort":"quantity","direction":"desc"}"#,
        )
        .unwrap();
        assert_eq!(request.sort, CycleCountCandidateSort::Quantity);
        assert_eq!(request.direction, CycleCountSortDirection::Desc);
        assert!(serde_json::from_str::<CycleCountWorkPageRequest>(
            r#"{"status":"completed","unknown":true}"#
        )
        .is_err());
    }

    #[test]
    fn task_creation_accepts_only_the_server_derived_balance_target() {
        let request = serde_json::from_str::<CreateCycleCountTaskRequest>(
            r#"{"inventory_balance_id":41,"note":"Quarterly blind count"}"#,
        )
        .unwrap();
        assert_eq!(request.inventory_balance_id, 41);
        assert!(serde_json::from_str::<CreateCycleCountTaskRequest>(
            r#"{"inventory_balance_id":41,"location_id":9}"#
        )
        .is_err());
    }

    #[test]
    fn policy_and_variance_decision_contracts_are_strict() {
        let policy = serde_json::from_str::<ConfigureCycleCountPolicyRequest>(
            r#"{"inventory_owner_id":1,"facility_id":2,"absolute_tolerance_quantity":1,"percentage_tolerance_basis_points":250,"automatic_recount_limit":1}"#,
        )
        .unwrap();
        assert_eq!(policy.percentage_tolerance_basis_points, 250);
        assert!(serde_json::from_str::<ConfigureCycleCountPolicyRequest>(
            r#"{"inventory_owner_id":1,"facility_id":2,"absolute_tolerance_quantity":1,"percentage_tolerance_basis_points":250,"automatic_recount_limit":1,"unknown":true}"#,
        )
        .is_err());

        let decision = serde_json::from_str::<DecideCycleCountVarianceRequest>(
            r#"{"expected_revision":1,"decision":"request_recount","reason":"suspected_miscount"}"#,
        )
        .unwrap();
        assert_eq!(
            decision.decision,
            CycleCountVarianceDecision::RequestRecount
        );
    }
}
