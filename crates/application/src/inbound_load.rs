use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    InboundLoadArrivalId, InboundLoadId, InboundLoadLineId, InboundLoadPreArrivalStatus,
    InboundLoadScanValue, LocationId, NewInboundLoadPlan, Timestamp, UserId,
};

pub const PLAN_INBOUND_LOAD_OPERATION: &str = "inbound.load.plan.v1";
pub const ARRIVE_INBOUND_LOAD_OPERATION: &str = "inbound.load.arrive.v1";

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
}
