use wareboxes_domain::{EmployeeId, FacilityId, InventoryOwnerId, UserId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkOrchestrationWorkerOptionReadModel {
    pub employee_id: EmployeeId,
    pub user_id: UserId,
    pub display_name: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkOrchestrationWorkerCursor {
    pub after_employee_id: EmployeeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkOrchestrationWorkerPageQuery {
    pub facility_id: FacilityId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub cursor: Option<WorkOrchestrationWorkerCursor>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkOrchestrationWorkerPage {
    pub items: Vec<WorkOrchestrationWorkerOptionReadModel>,
    pub next_cursor: Option<WorkOrchestrationWorkerCursor>,
}
