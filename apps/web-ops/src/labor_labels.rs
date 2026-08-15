use wareboxes_api_contract::v1::{
    AttendanceStatus, EquipmentStatus, LaborActivityKind, LaborActivityStatus,
    LaborExceptionReason, LaborQuantityBasis,
};

pub(super) fn activity_kind_label(kind: LaborActivityKind) -> &'static str {
    match kind {
        LaborActivityKind::Receiving => "Receiving",
        LaborActivityKind::Putaway => "Putaway",
        LaborActivityKind::Replenishment => "Replenishment",
        LaborActivityKind::Picking => "Picking",
        LaborActivityKind::Packing => "Packing",
        LaborActivityKind::Shipping => "Shipping",
        LaborActivityKind::CycleCount => "Cycle count",
        LaborActivityKind::InventoryRelocation => "Inventory relocation",
        LaborActivityKind::CrossDock => "Cross-dock",
        LaborActivityKind::Yard => "Yard",
        LaborActivityKind::CustomerReturn => "Customer return",
        LaborActivityKind::VendorReturn => "Vendor return",
        LaborActivityKind::ValueAddedWork => "Value-added work",
        LaborActivityKind::Break => "Break",
        LaborActivityKind::Meeting => "Meeting",
        LaborActivityKind::Training => "Training",
        LaborActivityKind::Maintenance => "Maintenance",
        LaborActivityKind::Delay => "Delay",
        LaborActivityKind::OtherIndirect => "Other indirect",
    }
}

pub(super) fn is_direct(kind: LaborActivityKind) -> bool {
    matches!(
        kind,
        LaborActivityKind::Receiving
            | LaborActivityKind::Putaway
            | LaborActivityKind::Replenishment
            | LaborActivityKind::Picking
            | LaborActivityKind::Packing
            | LaborActivityKind::Shipping
            | LaborActivityKind::CycleCount
            | LaborActivityKind::InventoryRelocation
            | LaborActivityKind::CrossDock
            | LaborActivityKind::Yard
            | LaborActivityKind::CustomerReturn
            | LaborActivityKind::VendorReturn
            | LaborActivityKind::ValueAddedWork
    )
}

pub(super) fn quantity_basis_label(basis: LaborQuantityBasis) -> &'static str {
    match basis {
        LaborQuantityBasis::Unit => "Unit",
        LaborQuantityBasis::Line => "Line",
        LaborQuantityBasis::Container => "Container",
        LaborQuantityBasis::Task => "Task",
        LaborQuantityBasis::WeightGram => "Weight (g)",
    }
}

pub(super) fn attendance_status_label(status: AttendanceStatus) -> &'static str {
    match status {
        AttendanceStatus::Open => "Open",
        AttendanceStatus::Closed => "Closed",
    }
}

pub(super) fn attendance_status_class(status: AttendanceStatus) -> &'static str {
    match status {
        AttendanceStatus::Open => "labor-status active",
        AttendanceStatus::Closed => "labor-status neutral",
    }
}

pub(super) fn activity_status_label(status: LaborActivityStatus) -> &'static str {
    match status {
        LaborActivityStatus::Active => "Active",
        LaborActivityStatus::Completed => "Completed",
        LaborActivityStatus::Cancelled => "Cancelled",
    }
}

pub(super) fn activity_status_class(status: LaborActivityStatus) -> &'static str {
    match status {
        LaborActivityStatus::Active => "labor-status active",
        LaborActivityStatus::Completed => "labor-status success",
        LaborActivityStatus::Cancelled => "labor-status danger",
    }
}

pub(super) fn equipment_status_label(status: EquipmentStatus) -> &'static str {
    match status {
        EquipmentStatus::Available => "Available",
        EquipmentStatus::Assigned => "Assigned",
        EquipmentStatus::OutOfService => "Out of service",
        EquipmentStatus::Retired => "Retired",
    }
}

pub(super) fn equipment_status_class(status: EquipmentStatus) -> &'static str {
    match status {
        EquipmentStatus::Available => "labor-status success",
        EquipmentStatus::Assigned => "labor-status active",
        EquipmentStatus::OutOfService => "labor-status danger",
        EquipmentStatus::Retired => "labor-status neutral",
    }
}

pub(super) fn equipment_status_value(status: EquipmentStatus) -> &'static str {
    match status {
        EquipmentStatus::Available => "available",
        EquipmentStatus::Assigned => "assigned",
        EquipmentStatus::OutOfService => "out_of_service",
        EquipmentStatus::Retired => "retired",
    }
}

pub(super) fn parse_equipment_status(value: &str) -> EquipmentStatus {
    match value {
        "available" => EquipmentStatus::Available,
        "retired" => EquipmentStatus::Retired,
        _ => EquipmentStatus::OutOfService,
    }
}

pub(super) fn exception_reason_value(reason: Option<LaborExceptionReason>) -> &'static str {
    match reason {
        None => "",
        Some(LaborExceptionReason::Equipment) => "equipment",
        Some(LaborExceptionReason::Congestion) => "congestion",
        Some(LaborExceptionReason::Inventory) => "inventory",
        Some(LaborExceptionReason::Quality) => "quality",
        Some(LaborExceptionReason::Safety) => "safety",
        Some(LaborExceptionReason::System) => "system",
        Some(LaborExceptionReason::Training) => "training",
        Some(LaborExceptionReason::Personal) => "personal",
        Some(LaborExceptionReason::Other) => "other",
    }
}

pub(super) fn exception_reason_label(reason: LaborExceptionReason) -> &'static str {
    match reason {
        LaborExceptionReason::Equipment => "Equipment",
        LaborExceptionReason::Congestion => "Congestion",
        LaborExceptionReason::Inventory => "Inventory",
        LaborExceptionReason::Quality => "Quality",
        LaborExceptionReason::Safety => "Safety",
        LaborExceptionReason::System => "System",
        LaborExceptionReason::Training => "Training",
        LaborExceptionReason::Personal => "Personal",
        LaborExceptionReason::Other => "Other",
    }
}

pub(super) fn parse_exception_reason(value: &str) -> Option<LaborExceptionReason> {
    match value {
        "equipment" => Some(LaborExceptionReason::Equipment),
        "congestion" => Some(LaborExceptionReason::Congestion),
        "inventory" => Some(LaborExceptionReason::Inventory),
        "quality" => Some(LaborExceptionReason::Quality),
        "safety" => Some(LaborExceptionReason::Safety),
        "system" => Some(LaborExceptionReason::System),
        "training" => Some(LaborExceptionReason::Training),
        "personal" => Some(LaborExceptionReason::Personal),
        "other" => Some(LaborExceptionReason::Other),
        _ => None,
    }
}
