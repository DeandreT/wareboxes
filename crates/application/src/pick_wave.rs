//! Application contracts for multi-order wave planning and atomic release.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, LocationId, OrderId, OrderReleaseId, OrderRevision, OrderStatus,
    PickWaveCancellationNote, PickWaveCancellationReason, PickWaveId, PickWaveName,
    PickWaveRevision, PickWaveStatus, Timestamp, UserId,
};

pub const PLAN_PICK_WAVE_OPERATION: &str = "outbound.pick_wave.plan.v1";
pub const RELEASE_PICK_WAVE_OPERATION: &str = "outbound.pick_wave.release.v1";
pub const CANCEL_PICK_WAVE_OPERATION: &str = "outbound.pick_wave.cancel.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanPickWaveOrder {
    pub order_id: OrderId,
    pub expected_revision: OrderRevision,
    pub sequence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanPickWaveCommand {
    pub facility_id: FacilityId,
    pub destination_location_id: LocationId,
    pub name: PickWaveName,
    pub orders: Vec<PlanPickWaveOrder>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleasePickWaveCommand {
    pub wave_id: PickWaveId,
    pub expected_revision: PickWaveRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelPickWaveCommand {
    pub wave_id: PickWaveId,
    pub expected_revision: PickWaveRevision,
    pub reason: PickWaveCancellationReason,
    pub note: Option<PickWaveCancellationNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickWaveOrderReadModel {
    pub order_id: OrderId,
    pub inventory_owner_id: InventoryOwnerId,
    pub order_key: String,
    pub sequence: u32,
    pub expected_revision: OrderRevision,
    pub resulting_revision: Option<OrderRevision>,
    pub release_id: Option<OrderReleaseId>,
    pub status: OrderStatus,
    pub allocation_count: i64,
    pub pick_task_count: i64,
    pub released_quantity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickWaveReadModel {
    pub wave_id: PickWaveId,
    pub facility_id: FacilityId,
    pub facility_name: String,
    pub destination_location_id: LocationId,
    pub destination_location_name: String,
    pub name: PickWaveName,
    pub status: PickWaveStatus,
    pub revision: PickWaveRevision,
    pub order_count: i64,
    pub allocation_count: i64,
    pub pick_task_count: i64,
    pub released_quantity: i64,
    pub orders: Vec<PickWaveOrderReadModel>,
    pub planned_by: UserId,
    pub planned_at: Timestamp,
    pub released_by: Option<UserId>,
    pub released_at: Option<Timestamp>,
    pub cancelled_by: Option<UserId>,
    pub cancelled_at: Option<Timestamp>,
    pub cancellation_reason: Option<PickWaveCancellationReason>,
    pub cancellation_note: Option<PickWaveCancellationNote>,
}

impl PickWaveReadModel {
    pub fn is_consistent(&self) -> bool {
        self.order_count > 0
            && usize::try_from(self.order_count) == Ok(self.orders.len())
            && self.orders.iter().enumerate().all(|(index, order)| {
                u32::try_from(index + 1) == Ok(order.sequence)
                    && match self.status {
                        PickWaveStatus::Planned | PickWaveStatus::Cancelled => {
                            order.release_id.is_none()
                                && order.resulting_revision.is_none()
                                && order.allocation_count == 0
                                && order.pick_task_count == 0
                                && order.released_quantity == 0
                        }
                        PickWaveStatus::Released => {
                            order.release_id.is_some()
                                && order.resulting_revision.is_some()
                                && order.status == OrderStatus::Processing
                                && order.allocation_count > 0
                                && order.allocation_count == order.pick_task_count
                                && order.released_quantity > 0
                        }
                    }
            })
    }
}

pub type PlanPickWaveResult = PickWaveReadModel;
pub type ReleasePickWaveResult = PickWaveReadModel;
pub type CancelPickWaveResult = PickWaveReadModel;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PickWaveSort {
    Name,
    Status,
    Orders,
    Tasks,
    Units,
    #[default]
    PlannedAt,
}

impl PickWaveSort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Status => "status",
            Self::Orders => "orders",
            Self::Tasks => "tasks",
            Self::Units => "units",
            Self::PlannedAt => "planned_at",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PickWaveSortDirection {
    Ascending,
    #[default]
    Descending,
}

impl PickWaveSortDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ascending => "asc",
            Self::Descending => "desc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickWaveQuery {
    pub facility_id: Option<FacilityId>,
    pub status: Option<PickWaveStatus>,
    pub limit: u16,
    pub sort: PickWaveSort,
    pub direction: PickWaveSortDirection,
    pub cursor: Option<PickWaveCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickWaveCursor {
    pub offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickWavePage {
    pub entries: Vec<PickWaveReadModel>,
    pub next_cursor: Option<PickWaveCursor>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_names_are_versioned() {
        assert_eq!(PLAN_PICK_WAVE_OPERATION, "outbound.pick_wave.plan.v1");
        assert_eq!(RELEASE_PICK_WAVE_OPERATION, "outbound.pick_wave.release.v1");
        assert_eq!(CANCEL_PICK_WAVE_OPERATION, "outbound.pick_wave.cancel.v1");
    }
}
