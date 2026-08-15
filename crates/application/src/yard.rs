//! Replay-safe yard appointments, gate control, movements, dock work, and detention.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    BillableEventId, FacilityId, InboundLoadId, InventoryOwnerId, OutboundLoadId, Timestamp,
    UserId, YardAppointmentId, YardAppointmentNumber, YardAppointmentStatus, YardAppointmentWindow,
    YardAssetId, YardAssetKind, YardAssetNumber, YardDetentionId, YardDirection, YardFreeMinutes,
    YardLocationCode, YardLocationId, YardLocationKind, YardName, YardNote, YardOperation,
    YardRevision, YardVisitEventId, YardVisitId, YardVisitStatus,
};

pub const CONFIGURE_YARD_LOCATION_OPERATION: &str = "yard.location.configure.v1";
pub const REGISTER_YARD_ASSET_OPERATION: &str = "yard.asset.register.v1";
pub const CREATE_YARD_APPOINTMENT_OPERATION: &str = "yard.appointment.create.v1";
pub const CANCEL_YARD_APPOINTMENT_OPERATION: &str = "yard.appointment.cancel.v1";
pub const MARK_YARD_APPOINTMENT_NO_SHOW_OPERATION: &str = "yard.appointment.no_show.v1";
pub const GATE_IN_YARD_VISIT_OPERATION: &str = "yard.visit.gate_in.v1";
pub const SPOT_YARD_VISIT_OPERATION: &str = "yard.visit.spot.v1";
pub const ASSIGN_YARD_VISIT_DOOR_OPERATION: &str = "yard.visit.assign_door.v1";
pub const START_YARD_OPERATION: &str = "yard.visit.operation.start.v1";
pub const COMPLETE_YARD_OPERATION: &str = "yard.visit.operation.complete.v1";
pub const REJECT_YARD_VISIT_OPERATION: &str = "yard.visit.reject.v1";
pub const GATE_OUT_YARD_VISIT_OPERATION: &str = "yard.visit.gate_out.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureYardLocationCommand {
    pub facility_id: FacilityId,
    pub code: YardLocationCode,
    pub name: YardName,
    pub kind: YardLocationKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegisterYardAssetCommand {
    pub kind: YardAssetKind,
    pub asset_number: YardAssetNumber,
    pub carrier: YardName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateYardAppointmentCommand {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub direction: YardDirection,
    pub appointment_number: YardAppointmentNumber,
    pub window: YardAppointmentWindow,
    pub carrier: YardName,
    pub expected_asset_kind: YardAssetKind,
    pub expected_asset_number: Option<YardAssetNumber>,
    pub inbound_load_id: Option<InboundLoadId>,
    pub outbound_load_id: Option<OutboundLoadId>,
    pub free_minutes: YardFreeMinutes,
    pub note: Option<YardNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct YardAppointmentLifecycleCommand {
    pub appointment_id: YardAppointmentId,
    pub expected_revision: YardRevision,
    pub note: YardNote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateInYardVisitCommand {
    pub appointment_id: Option<YardAppointmentId>,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub direction: YardDirection,
    pub asset_id: YardAssetId,
    pub driver_name: YardName,
    pub gate_location_id: YardLocationId,
    pub note: Option<YardNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MoveYardVisitCommand {
    pub visit_id: YardVisitId,
    pub expected_revision: YardRevision,
    pub destination_location_id: YardLocationId,
    pub note: YardNote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssignYardVisitDoorCommand {
    pub visit_id: YardVisitId,
    pub expected_revision: YardRevision,
    pub door_location_id: YardLocationId,
    pub note: YardNote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct YardDockOperationCommand {
    pub visit_id: YardVisitId,
    pub expected_revision: YardRevision,
    pub operation: YardOperation,
    pub note: YardNote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct YardVisitLifecycleCommand {
    pub visit_id: YardVisitId,
    pub expected_revision: YardRevision,
    pub note: YardNote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardVisitEventKind {
    GatedIn,
    Spotted,
    DoorAssigned,
    OperationStarted,
    OperationCompleted,
    Rejected,
    GatedOut,
}

impl YardVisitEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GatedIn => "gated_in",
            Self::Spotted => "spotted",
            Self::DoorAssigned => "door_assigned",
            Self::OperationStarted => "operation_started",
            Self::OperationCompleted => "operation_completed",
            Self::Rejected => "rejected",
            Self::GatedOut => "gated_out",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "gated_in" => Some(Self::GatedIn),
            "spotted" => Some(Self::Spotted),
            "door_assigned" => Some(Self::DoorAssigned),
            "operation_started" => Some(Self::OperationStarted),
            "operation_completed" => Some(Self::OperationCompleted),
            "rejected" => Some(Self::Rejected),
            "gated_out" => Some(Self::GatedOut),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardLocationReadModel {
    pub location_id: YardLocationId,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub code: String,
    pub name: String,
    pub kind: YardLocationKind,
    pub active: bool,
    pub revision: YardRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardAssetReadModel {
    pub asset_id: YardAssetId,
    pub kind: YardAssetKind,
    pub asset_number: String,
    pub carrier: String,
    pub active: bool,
    pub revision: YardRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardAppointmentReadModel {
    pub appointment_id: YardAppointmentId,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub direction: YardDirection,
    pub appointment_number: String,
    pub window: YardAppointmentWindow,
    pub carrier: String,
    pub expected_asset_kind: YardAssetKind,
    pub expected_asset_number: Option<String>,
    pub inbound_load_id: Option<InboundLoadId>,
    pub outbound_load_id: Option<OutboundLoadId>,
    pub free_minutes: YardFreeMinutes,
    pub status: YardAppointmentStatus,
    pub revision: YardRevision,
    pub note: Option<String>,
    pub visit_id: Option<YardVisitId>,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub updated_by: Option<UserId>,
    pub updated_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardVisitEventReadModel {
    pub event_id: YardVisitEventId,
    pub kind: YardVisitEventKind,
    pub from_status: Option<YardVisitStatus>,
    pub to_status: YardVisitStatus,
    pub from_location_id: Option<YardLocationId>,
    pub to_location_id: Option<YardLocationId>,
    pub operation: Option<YardOperation>,
    pub note: Option<String>,
    pub resulting_revision: YardRevision,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardDetentionReadModel {
    pub detention_id: YardDetentionId,
    pub total_minutes: u64,
    pub free_minutes: u32,
    pub detention_minutes: u64,
    pub billable_hours: u64,
    pub billable_event_id: Option<BillableEventId>,
    pub calculated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardVisitReadModel {
    pub visit_id: YardVisitId,
    pub appointment_id: Option<YardAppointmentId>,
    pub appointment_number: Option<String>,
    pub inventory_owner_id: InventoryOwnerId,
    pub inventory_owner_name: String,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub direction: YardDirection,
    pub asset_id: YardAssetId,
    pub asset_kind: YardAssetKind,
    pub asset_number: String,
    pub carrier: String,
    pub driver_name: String,
    pub status: YardVisitStatus,
    pub revision: YardRevision,
    pub current_location_id: Option<YardLocationId>,
    pub current_location_code: Option<String>,
    pub dock_door_location_id: Option<YardLocationId>,
    pub dock_door_code: Option<String>,
    pub inbound_load_id: Option<InboundLoadId>,
    pub outbound_load_id: Option<OutboundLoadId>,
    pub gated_in_at: Timestamp,
    pub operation_started_at: Option<Timestamp>,
    pub operation_completed_at: Option<Timestamp>,
    pub gated_out_at: Option<Timestamp>,
    pub rejected_at: Option<Timestamp>,
    pub detention: Option<YardDetentionReadModel>,
    pub events: Vec<YardVisitEventReadModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YardWorkspaceFilter {
    pub facility_id: Option<FacilityId>,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub include_completed: bool,
    pub before_visit_id: Option<YardVisitId>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YardWorkspace {
    pub locations: Vec<YardLocationReadModel>,
    pub assets: Vec<YardAssetReadModel>,
    pub appointments: Vec<YardAppointmentReadModel>,
    pub visits: Vec<YardVisitReadModel>,
    pub next_visit_id: Option<YardVisitId>,
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use wareboxes_domain::TenantId;

    use super::*;
    use crate::idempotency::PreparedCommand;
    use crate::CommandContext;

    #[test]
    fn appointment_hash_includes_scope_schedule_and_load_binding() {
        let command = CreateYardAppointmentCommand {
            inventory_owner_id: InventoryOwnerId::new(2).unwrap(),
            facility_id: FacilityId::new(3).unwrap(),
            direction: YardDirection::Inbound,
            appointment_number: YardAppointmentNumber::new("APT-42").unwrap(),
            window: YardAppointmentWindow::new(
                Utc.with_ymd_and_hms(2026, 8, 1, 8, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 8, 1, 9, 0, 0).unwrap(),
            )
            .unwrap(),
            carrier: YardName::new("Example Freight").unwrap(),
            expected_asset_kind: YardAssetKind::Trailer,
            expected_asset_number: Some(YardAssetNumber::new("TRL-42").unwrap()),
            inbound_load_id: Some(InboundLoadId::new(4).unwrap()),
            outbound_load_id: None,
            free_minutes: YardFreeMinutes::new(120).unwrap(),
            note: None,
        };
        let context = CommandContext {
            tenant_id: TenantId::new(1).unwrap(),
            actor_id: UserId::new(5).unwrap(),
            request_id: "request-yard-42".into(),
            idempotency_key: Some("yard-42".into()),
        };
        let prepared =
            PreparedCommand::new_v1(&context, CREATE_YARD_APPOINTMENT_OPERATION, &command).unwrap();
        assert_eq!(prepared.request_hash().len(), 64);
    }
}
