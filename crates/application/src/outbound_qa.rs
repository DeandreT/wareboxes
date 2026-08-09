//! Application contracts for policy-driven outbound carton verification.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    CartonId, FacilityId, InventoryOwnerId, LicensePlateId, OrderId, OrderRevision,
    OutboundQaCancellationDetails, OutboundQaCancellationId, OutboundQaCartonVerificationId,
    OutboundQaPolicyId, OutboundQaPolicyRevision, OutboundQaProgress, OutboundQaRequirement,
    OutboundQaScanValue, OutboundQaSessionId, OutboundQaSessionRevision, OutboundQaSessionStatus,
    PackSessionId, Timestamp, UserId,
};

pub const CONFIGURE_OUTBOUND_QA_POLICY_OPERATION: &str = "outbound_qa.policy.configure.v1";
pub const START_OUTBOUND_QA_OPERATION: &str = "outbound_qa.session.start.v1";
pub const VERIFY_OUTBOUND_QA_CARTON_OPERATION: &str = "outbound_qa.carton.verify.v1";
pub const COMPLETE_OUTBOUND_QA_OPERATION: &str = "outbound_qa.session.complete.v1";
pub const CANCEL_OUTBOUND_QA_OPERATION: &str = "outbound_qa.session.cancel.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureOutboundQaPolicyCommand {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub requirement: OutboundQaRequirement,
    pub expected_revision: Option<OutboundQaPolicyRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundQaPolicyReadModel {
    pub policy_id: OutboundQaPolicyId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub requirement: OutboundQaRequirement,
    pub revision: OutboundQaPolicyRevision,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
}

pub type ConfigureOutboundQaPolicyResult = OutboundQaPolicyReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOutboundQaCommand {
    pub packing_session_id: PackSessionId,
    pub expected_order_revision: OrderRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundQaCartonVerificationReadModel {
    pub verification_id: OutboundQaCartonVerificationId,
    pub carton_id: CartonId,
    pub license_plate_id: LicensePlateId,
    pub sequence: i64,
    pub carton_barcode: OutboundQaScanValue,
    pub content_count: i64,
    pub packed_quantity: i64,
    pub verified_by: UserId,
    pub verified_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundQaCancellationReadModel {
    pub cancellation_id: OutboundQaCancellationId,
    pub previous_status: OutboundQaSessionStatus,
    pub details: OutboundQaCancellationDetails,
    pub cancelled_by: UserId,
    pub cancelled_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundQaSessionReadModel {
    pub session_id: OutboundQaSessionId,
    pub packing_session_id: PackSessionId,
    pub order_id: OrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub policy_id: OutboundQaPolicyId,
    pub policy_revision: OutboundQaPolicyRevision,
    pub attempt: i64,
    pub status: OutboundQaSessionStatus,
    pub revision: OutboundQaSessionRevision,
    pub progress: OutboundQaProgress,
    pub started_by: UserId,
    pub started_at: Timestamp,
    pub passed_by: Option<UserId>,
    pub passed_at: Option<Timestamp>,
    pub cancellation: Option<OutboundQaCancellationReadModel>,
    pub verifications: Vec<OutboundQaCartonVerificationReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundQaSessionSummaryReadModel {
    pub session_id: OutboundQaSessionId,
    pub policy_id: OutboundQaPolicyId,
    pub policy_revision: OutboundQaPolicyRevision,
    pub attempt: i64,
    pub status: OutboundQaSessionStatus,
    pub revision: OutboundQaSessionRevision,
    pub progress: OutboundQaProgress,
    pub started_at: Timestamp,
    pub passed_at: Option<Timestamp>,
    pub cancelled_at: Option<Timestamp>,
}

pub type StartOutboundQaResult = OutboundQaSessionReadModel;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyOutboundQaCartonCommand {
    pub session_id: OutboundQaSessionId,
    pub expected_revision: OutboundQaSessionRevision,
    pub carton_barcode: OutboundQaScanValue,
}

pub type VerifyOutboundQaCartonResult = OutboundQaSessionReadModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteOutboundQaCommand {
    pub session_id: OutboundQaSessionId,
    pub expected_revision: OutboundQaSessionRevision,
}

pub type CompleteOutboundQaResult = OutboundQaSessionReadModel;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelOutboundQaCommand {
    pub session_id: OutboundQaSessionId,
    pub expected_revision: OutboundQaSessionRevision,
    pub details: OutboundQaCancellationDetails,
}

pub type CancelOutboundQaResult = OutboundQaSessionReadModel;

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_domain::{OutboundQaCancellationNote, OutboundQaCancellationReason};

    #[test]
    fn cancellation_operation_and_command_are_typed() {
        assert_eq!(
            CANCEL_OUTBOUND_QA_OPERATION,
            "outbound_qa.session.cancel.v1"
        );
        let command = CancelOutboundQaCommand {
            session_id: OutboundQaSessionId::new(4).unwrap(),
            expected_revision: OutboundQaSessionRevision::new(3).unwrap(),
            details: OutboundQaCancellationDetails::new(
                OutboundQaCancellationReason::Other,
                Some(OutboundQaCancellationNote::new("Supervisor correction").unwrap()),
            )
            .unwrap(),
        };
        assert_eq!(command.expected_revision.get(), 3);
    }
}
