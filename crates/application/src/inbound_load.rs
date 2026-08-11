use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    InboundLoadAppointmentId, InboundLoadArrivalId, InboundLoadClosureId, InboundLoadId,
    InboundLoadLineId, InboundLoadPreArrivalStatus, InboundLoadScanValue,
    InboundLoadUnloadingStartId, LocationId, NewInboundLoadPlan, Timestamp, UserId,
};

pub const PLAN_INBOUND_LOAD_OPERATION: &str = "inbound.load.plan.v1";
pub const SCHEDULE_INBOUND_LOAD_OPERATION: &str = "inbound.load.appointment.schedule.v1";
pub const ARRIVE_INBOUND_LOAD_OPERATION: &str = "inbound.load.arrive.v1";
pub const START_INBOUND_LOAD_UNLOADING_OPERATION: &str = "inbound.load.unloading.start.v1";
pub const CLOSE_INBOUND_LOAD_OPERATION: &str = "inbound.load.close.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanInboundLoadCommand {
    plan: NewInboundLoadPlan,
}

impl PlanInboundLoadCommand {
    pub const fn new(plan: NewInboundLoadPlan) -> Self {
        Self { plan }
    }

    pub const fn plan(&self) -> &NewInboundLoadPlan {
        &self.plan
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannedInboundLoadStatus {
    Planned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedInboundLoadLineResult {
    pub load_line_id: InboundLoadLineId,
    pub item_id: i64,
    pub expected_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanInboundLoadResult {
    pub load_id: InboundLoadId,
    pub execution_barcode: String,
    pub reference: String,
    pub status: PlannedInboundLoadStatus,
    pub lines: Vec<PlannedInboundLoadLineResult>,
    pub total_expected_quantity: i64,
    pub planned_by: UserId,
    pub planned_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScheduleInboundLoadCommand {
    load_id: InboundLoadId,
    scheduled_for: Timestamp,
}

impl ScheduleInboundLoadCommand {
    pub const fn new(load_id: InboundLoadId, scheduled_for: Timestamp) -> Self {
        Self {
            load_id,
            scheduled_for,
        }
    }

    pub const fn load_id(&self) -> InboundLoadId {
        self.load_id
    }

    pub const fn scheduled_for(&self) -> Timestamp {
        self.scheduled_for
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundLoadPlannedStatus {
    Planned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundLoadScheduledStatus {
    Scheduled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleInboundLoadResult {
    pub appointment_id: InboundLoadAppointmentId,
    pub load_id: InboundLoadId,
    pub previous_status: InboundLoadPlannedStatus,
    pub status: InboundLoadScheduledStatus,
    pub scheduled_for: Timestamp,
    pub scheduled_by: UserId,
    pub scheduled_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArriveInboundLoadCommand {
    load_id: InboundLoadId,
    load_scan: InboundLoadScanValue,
    receiving_location_scan: InboundLoadScanValue,
    arrived_at: Option<Timestamp>,
}

impl ArriveInboundLoadCommand {
    pub const fn new(
        load_id: InboundLoadId,
        load_scan: InboundLoadScanValue,
        receiving_location_scan: InboundLoadScanValue,
        arrived_at: Option<Timestamp>,
    ) -> Self {
        Self {
            load_id,
            load_scan,
            receiving_location_scan,
            arrived_at,
        }
    }

    pub const fn load_id(&self) -> InboundLoadId {
        self.load_id
    }

    pub const fn load_scan(&self) -> &InboundLoadScanValue {
        &self.load_scan
    }

    pub const fn receiving_location_scan(&self) -> &InboundLoadScanValue {
        &self.receiving_location_scan
    }

    pub const fn arrived_at(&self) -> Option<&Timestamp> {
        self.arrived_at.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrivedInboundLoadStatus {
    Arrived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArriveInboundLoadResult {
    pub arrival_id: InboundLoadArrivalId,
    pub load_id: InboundLoadId,
    pub previous_status: InboundLoadPreArrivalStatus,
    pub status: ArrivedInboundLoadStatus,
    pub receiving_location_id: LocationId,
    pub arrived_by: UserId,
    pub arrived_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartInboundLoadUnloadingCommand {
    load_id: InboundLoadId,
    load_scan: InboundLoadScanValue,
    receiving_location_scan: InboundLoadScanValue,
    seal_scan: Option<InboundLoadScanValue>,
    started_at: Option<Timestamp>,
}

impl StartInboundLoadUnloadingCommand {
    pub const fn new(
        load_id: InboundLoadId,
        load_scan: InboundLoadScanValue,
        receiving_location_scan: InboundLoadScanValue,
        seal_scan: Option<InboundLoadScanValue>,
        started_at: Option<Timestamp>,
    ) -> Self {
        Self {
            load_id,
            load_scan,
            receiving_location_scan,
            seal_scan,
            started_at,
        }
    }

    pub const fn load_id(&self) -> InboundLoadId {
        self.load_id
    }
    pub const fn load_scan(&self) -> &InboundLoadScanValue {
        &self.load_scan
    }
    pub const fn receiving_location_scan(&self) -> &InboundLoadScanValue {
        &self.receiving_location_scan
    }
    pub fn seal_scan(&self) -> Option<&InboundLoadScanValue> {
        self.seal_scan.as_ref()
    }
    pub const fn started_at(&self) -> Option<&Timestamp> {
        self.started_at.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundLoadReceivingStatus {
    Receiving,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartInboundLoadUnloadingResult {
    pub unloading_start_id: InboundLoadUnloadingStartId,
    pub load_id: InboundLoadId,
    pub status: InboundLoadReceivingStatus,
    pub receiving_location_id: LocationId,
    pub started_by: UserId,
    pub started_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloseInboundLoadCommand {
    load_id: InboundLoadId,
    load_scan: InboundLoadScanValue,
    receiving_location_scan: InboundLoadScanValue,
    closed_at: Option<Timestamp>,
}

impl CloseInboundLoadCommand {
    pub const fn new(
        load_id: InboundLoadId,
        load_scan: InboundLoadScanValue,
        receiving_location_scan: InboundLoadScanValue,
        closed_at: Option<Timestamp>,
    ) -> Self {
        Self {
            load_id,
            load_scan,
            receiving_location_scan,
            closed_at,
        }
    }

    pub const fn load_id(&self) -> InboundLoadId {
        self.load_id
    }
    pub const fn load_scan(&self) -> &InboundLoadScanValue {
        &self.load_scan
    }
    pub const fn receiving_location_scan(&self) -> &InboundLoadScanValue {
        &self.receiving_location_scan
    }
    pub const fn closed_at(&self) -> Option<&Timestamp> {
        self.closed_at.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundLoadReceivedStatus {
    Received,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundLoadClosedStatus {
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseInboundLoadResult {
    pub closure_id: InboundLoadClosureId,
    pub load_id: InboundLoadId,
    pub previous_status: InboundLoadReceivedStatus,
    pub status: InboundLoadClosedStatus,
    pub receiving_location_id: LocationId,
    pub closed_by: UserId,
    pub closed_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wareboxes_domain::{
        CatalogItemId, FacilityId, InboundExpectedQuantity, InboundLoadPlanLine,
        InboundLoadReference, InventoryOwnerId, LocationId,
    };

    use super::*;

    #[test]
    fn command_hash_contains_the_complete_authoritative_plan() {
        let plan = NewInboundLoadPlan::new(
            InventoryOwnerId::new(7).unwrap(),
            FacilityId::new(8).unwrap(),
            LocationId::new(9).unwrap(),
            InboundLoadReference::new("ASN-100").unwrap(),
            None,
            Some("Parcel Freight".into()),
            None,
            None,
            None,
            vec![InboundLoadPlanLine::new(
                CatalogItemId::new(10).unwrap(),
                InboundExpectedQuantity::new(12).unwrap(),
                Some("LOT-A".into()),
                None,
                None,
            )
            .unwrap()],
        )
        .unwrap();
        let value = serde_json::to_value(PlanInboundLoadCommand::new(plan)).unwrap();
        assert_eq!(value["plan"]["reference"], json!("ASN-100"));
        assert_eq!(value["plan"]["lines"][0]["expected_quantity"], json!(12));
        assert_eq!(value["plan"]["receiving_location_id"], json!(9));
    }

    #[test]
    fn arrival_hash_contains_exact_scan_evidence() {
        let value = serde_json::to_value(ArriveInboundLoadCommand::new(
            InboundLoadId::new(12).unwrap(),
            InboundLoadScanValue::new("WB-LOAD-12").unwrap(),
            InboundLoadScanValue::new("RECV-01").unwrap(),
            None,
        ))
        .unwrap();
        assert_eq!(value["load_id"], json!(12));
        assert_eq!(value["load_scan"], json!("WB-LOAD-12"));
        assert_eq!(value["receiving_location_scan"], json!("RECV-01"));
    }

    #[test]
    fn appointment_hash_contains_the_exact_load_and_time() {
        let value = serde_json::to_value(ScheduleInboundLoadCommand::new(
            InboundLoadId::new(12).unwrap(),
            "2027-08-12T17:00:00Z".parse().unwrap(),
        ))
        .unwrap();
        assert_eq!(value["load_id"], json!(12));
        assert_eq!(value["scheduled_for"], json!("2027-08-12T17:00:00Z"));
    }

    #[test]
    fn unloading_hash_contains_optional_seal_evidence() {
        let value = serde_json::to_value(StartInboundLoadUnloadingCommand::new(
            InboundLoadId::new(12).unwrap(),
            InboundLoadScanValue::new("WB-LOAD-12").unwrap(),
            InboundLoadScanValue::new("RECV-01").unwrap(),
            Some(InboundLoadScanValue::new("SEAL-12").unwrap()),
            None,
        ))
        .unwrap();
        assert_eq!(value["seal_scan"], json!("SEAL-12"));
        assert_eq!(value["load_id"], json!(12));
    }

    #[test]
    fn closure_hash_contains_exact_physical_evidence() {
        let value = serde_json::to_value(CloseInboundLoadCommand::new(
            InboundLoadId::new(12).unwrap(),
            InboundLoadScanValue::new("WB-LOAD-12").unwrap(),
            InboundLoadScanValue::new("RECV-01").unwrap(),
            None,
        ))
        .unwrap();
        assert_eq!(value["load_id"], json!(12));
        assert_eq!(value["load_scan"], json!("WB-LOAD-12"));
        assert_eq!(value["receiving_location_scan"], json!("RECV-01"));
    }
}
