use chrono::Duration;
use serde::{Deserialize, Serialize};

use crate::Timestamp;

pub const MAX_LABOR_CODE_LENGTH: usize = 80;
pub const MAX_LABOR_NAME_LENGTH: usize = 160;
pub const MAX_LABOR_NOTE_LENGTH: usize = 500;
pub const MAX_LABOR_REFERENCE_TYPE_LENGTH: usize = 80;
pub const MAX_LABOR_STANDARD_SECONDS: i64 = 86_400;
pub const MAX_LABOR_QUANTITY: i64 = 1_000_000_000;
pub const MAX_LABOR_BASIS_POINTS: i64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaborError {
    #[error("{field} must be nonblank, trimmed, and control-free")]
    InvalidText { field: &'static str },
    #[error("{field} cannot exceed {maximum} characters")]
    TextTooLong { field: &'static str, maximum: usize },
    #[error("labor revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("labor revision cannot advance beyond its supported range")]
    RevisionExhausted,
    #[error("labor quantity must be between one and {MAX_LABOR_QUANTITY}")]
    InvalidQuantity,
    #[error("labor standard seconds are outside the supported range")]
    InvalidStandardSeconds,
    #[error("labor standard duration exceeds its supported range")]
    StandardDurationOverflow,
    #[error("labor activity interval must have positive duration")]
    InvalidActivityInterval,
    #[error("labor attendance interval must have positive duration")]
    InvalidAttendanceInterval,
    #[error("employee must have an open attendance interval")]
    AttendanceNotOpen,
    #[error("employee already has active labor")]
    LaborAlreadyActive,
    #[error("labor activity is not active")]
    LaborNotActive,
    #[error("direct labor requires a business reference")]
    DirectReferenceRequired,
    #[error("indirect labor cannot carry a business reference")]
    IndirectReferenceForbidden,
    #[error("direct labor completion requires completed quantity")]
    DirectQuantityRequired,
    #[error("indirect labor cannot carry completed quantity")]
    IndirectQuantityForbidden,
    #[error("certification expiry must be after issuance")]
    InvalidCertificationWindow,
    #[error("employee is not eligible: {0:?}")]
    Ineligible(EligibilityFailure),
    #[error("paid and direct labor seconds are inconsistent")]
    InvalidUtilizationInterval,
    #[error("actual labor seconds must be positive")]
    InvalidActualSeconds,
    #[error("only closed attendance can be corrected")]
    AttendanceCorrectionRequiresClosedInterval,
    #[error("only completed labor can be corrected")]
    ActivityCorrectionRequiresCompletedActivity,
    #[error("labor exception seconds must be between zero and actual seconds")]
    InvalidExceptionSeconds,
}

fn required_text(
    value: impl Into<String>,
    field: &'static str,
    maximum: usize,
) -> Result<String, LaborError> {
    let value = value.into();
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(LaborError::InvalidText { field });
    }
    if value.chars().count() > maximum {
        return Err(LaborError::TextTooLong { field, maximum });
    }
    Ok(value)
}

macro_rules! labor_text {
    ($name:ident, $field:literal, $maximum:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, LaborError> {
                required_text(value, $field, $maximum).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

labor_text!(LaborCode, "labor code", MAX_LABOR_CODE_LENGTH);
labor_text!(LaborName, "labor name", MAX_LABOR_NAME_LENGTH);
labor_text!(LaborNote, "labor note", MAX_LABOR_NOTE_LENGTH);
labor_text!(
    LaborReferenceType,
    "labor reference type",
    MAX_LABOR_REFERENCE_TYPE_LENGTH
);
labor_text!(EquipmentNumber, "equipment number", MAX_LABOR_CODE_LENGTH);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LaborRevision(i64);

impl LaborRevision {
    pub const fn new(value: i64) -> Result<Self, LaborError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(LaborError::InvalidRevision { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn next(self) -> Result<Self, LaborError> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(LaborError::RevisionExhausted),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LaborQuantity(i64);

impl LaborQuantity {
    pub const fn new(value: i64) -> Result<Self, LaborError> {
        if value > 0 && value <= MAX_LABOR_QUANTITY {
            Ok(Self(value))
        } else {
            Err(LaborError::InvalidQuantity)
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttendanceStatus {
    Open,
    Closed,
}

impl AttendanceStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaborActivityKind {
    Receiving,
    Putaway,
    Replenishment,
    Picking,
    Packing,
    Shipping,
    CycleCount,
    InventoryRelocation,
    CrossDock,
    Yard,
    CustomerReturn,
    VendorReturn,
    ValueAddedWork,
    Break,
    Meeting,
    Training,
    Maintenance,
    Delay,
    OtherIndirect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaborQuantityBasis {
    Unit,
    Line,
    Container,
    Task,
    WeightGram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaborExceptionReason {
    Equipment,
    Congestion,
    Inventory,
    Quality,
    Safety,
    System,
    Training,
    Personal,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaborCorrectionReason {
    MissedPunch,
    TimekeepingError,
    QuantityError,
    ExceptionError,
    SystemError,
    Other,
}

impl LaborCorrectionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissedPunch => "missed_punch",
            Self::TimekeepingError => "timekeeping_error",
            Self::QuantityError => "quantity_error",
            Self::ExceptionError => "exception_error",
            Self::SystemError => "system_error",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "missed_punch" => Some(Self::MissedPunch),
            "timekeeping_error" => Some(Self::TimekeepingError),
            "quantity_error" => Some(Self::QuantityError),
            "exception_error" => Some(Self::ExceptionError),
            "system_error" => Some(Self::SystemError),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    pub const fn supports_attendance(self) -> bool {
        matches!(
            self,
            Self::MissedPunch | Self::TimekeepingError | Self::SystemError | Self::Other
        )
    }

    pub const fn supports_activity(self) -> bool {
        matches!(
            self,
            Self::TimekeepingError
                | Self::QuantityError
                | Self::ExceptionError
                | Self::SystemError
                | Self::Other
        )
    }
}

impl LaborExceptionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Equipment => "equipment",
            Self::Congestion => "congestion",
            Self::Inventory => "inventory",
            Self::Quality => "quality",
            Self::Safety => "safety",
            Self::System => "system",
            Self::Training => "training",
            Self::Personal => "personal",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "equipment" => Some(Self::Equipment),
            "congestion" => Some(Self::Congestion),
            "inventory" => Some(Self::Inventory),
            "quality" => Some(Self::Quality),
            "safety" => Some(Self::Safety),
            "system" => Some(Self::System),
            "training" => Some(Self::Training),
            "personal" => Some(Self::Personal),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

impl LaborQuantityBasis {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Line => "line",
            Self::Container => "container",
            Self::Task => "task",
            Self::WeightGram => "weight_gram",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unit" => Some(Self::Unit),
            "line" => Some(Self::Line),
            "container" => Some(Self::Container),
            "task" => Some(Self::Task),
            "weight_gram" => Some(Self::WeightGram),
            _ => None,
        }
    }
}

impl LaborActivityKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Receiving => "receiving",
            Self::Putaway => "putaway",
            Self::Replenishment => "replenishment",
            Self::Picking => "picking",
            Self::Packing => "packing",
            Self::Shipping => "shipping",
            Self::CycleCount => "cycle_count",
            Self::InventoryRelocation => "inventory_relocation",
            Self::CrossDock => "cross_dock",
            Self::Yard => "yard",
            Self::CustomerReturn => "customer_return",
            Self::VendorReturn => "vendor_return",
            Self::ValueAddedWork => "value_added_work",
            Self::Break => "break",
            Self::Meeting => "meeting",
            Self::Training => "training",
            Self::Maintenance => "maintenance",
            Self::Delay => "delay",
            Self::OtherIndirect => "other_indirect",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "receiving" => Some(Self::Receiving),
            "putaway" => Some(Self::Putaway),
            "replenishment" => Some(Self::Replenishment),
            "picking" => Some(Self::Picking),
            "packing" => Some(Self::Packing),
            "shipping" => Some(Self::Shipping),
            "cycle_count" => Some(Self::CycleCount),
            "inventory_relocation" => Some(Self::InventoryRelocation),
            "cross_dock" => Some(Self::CrossDock),
            "yard" => Some(Self::Yard),
            "customer_return" => Some(Self::CustomerReturn),
            "vendor_return" => Some(Self::VendorReturn),
            "value_added_work" => Some(Self::ValueAddedWork),
            "break" => Some(Self::Break),
            "meeting" => Some(Self::Meeting),
            "training" => Some(Self::Training),
            "maintenance" => Some(Self::Maintenance),
            "delay" => Some(Self::Delay),
            "other_indirect" => Some(Self::OtherIndirect),
            _ => None,
        }
    }

    pub const fn is_direct(self) -> bool {
        matches!(
            self,
            Self::Receiving
                | Self::Putaway
                | Self::Replenishment
                | Self::Picking
                | Self::Packing
                | Self::Shipping
                | Self::CycleCount
                | Self::InventoryRelocation
                | Self::CrossDock
                | Self::Yard
                | Self::CustomerReturn
                | Self::VendorReturn
                | Self::ValueAddedWork
        )
    }

    /// Returns whether the basis can be reconciled to canonical evidence for this workflow.
    pub const fn supports_quantity_basis(self, basis: LaborQuantityBasis) -> bool {
        match self {
            Self::Receiving => matches!(
                basis,
                LaborQuantityBasis::Unit | LaborQuantityBasis::Line | LaborQuantityBasis::Task
            ),
            Self::Putaway | Self::InventoryRelocation => matches!(
                basis,
                LaborQuantityBasis::Unit
                    | LaborQuantityBasis::Line
                    | LaborQuantityBasis::Container
                    | LaborQuantityBasis::Task
            ),
            Self::Replenishment
            | Self::Picking
            | Self::Packing
            | Self::CrossDock
            | Self::CustomerReturn
            | Self::VendorReturn
            | Self::ValueAddedWork => matches!(
                basis,
                LaborQuantityBasis::Unit | LaborQuantityBasis::Line | LaborQuantityBasis::Task
            ),
            Self::Shipping => true,
            Self::CycleCount => {
                matches!(basis, LaborQuantityBasis::Line | LaborQuantityBasis::Task)
            }
            Self::Yard => matches!(
                basis,
                LaborQuantityBasis::Container | LaborQuantityBasis::Task
            ),
            Self::Break
            | Self::Meeting
            | Self::Training
            | Self::Maintenance
            | Self::Delay
            | Self::OtherIndirect => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaborActivityStatus {
    Active,
    Completed,
    Cancelled,
}

impl LaborActivityStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentStatus {
    Available,
    Assigned,
    OutOfService,
    Retired,
}

impl EquipmentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Assigned => "assigned",
            Self::OutOfService => "out_of_service",
            Self::Retired => "retired",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "available" => Some(Self::Available),
            "assigned" => Some(Self::Assigned),
            "out_of_service" => Some(Self::OutOfService),
            "retired" => Some(Self::Retired),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificationWindow {
    pub issued_at: Timestamp,
    pub expires_at: Option<Timestamp>,
}

impl CertificationWindow {
    pub fn new(issued_at: Timestamp, expires_at: Option<Timestamp>) -> Result<Self, LaborError> {
        if expires_at.is_some_and(|expires_at| expires_at <= issued_at) {
            return Err(LaborError::InvalidCertificationWindow);
        }
        Ok(Self {
            issued_at,
            expires_at,
        })
    }

    pub fn is_valid_at(self, at: Timestamp) -> bool {
        self.issued_at <= at && self.expires_at.is_none_or(|expires_at| expires_at > at)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaborStandard {
    pub setup_seconds: i64,
    pub seconds_per_unit: i64,
}

impl LaborStandard {
    pub const fn new(setup_seconds: i64, seconds_per_unit: i64) -> Result<Self, LaborError> {
        if setup_seconds < 0
            || setup_seconds > MAX_LABOR_STANDARD_SECONDS
            || seconds_per_unit <= 0
            || seconds_per_unit > MAX_LABOR_STANDARD_SECONDS
        {
            return Err(LaborError::InvalidStandardSeconds);
        }
        Ok(Self {
            setup_seconds,
            seconds_per_unit,
        })
    }

    pub fn expected_seconds(self, quantity: LaborQuantity) -> Result<i64, LaborError> {
        self.seconds_per_unit
            .checked_mul(quantity.get())
            .and_then(|seconds| seconds.checked_add(self.setup_seconds))
            .ok_or(LaborError::StandardDurationOverflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EligibilityFailure {
    EmployeeInactive,
    FacilityNotAssigned,
    AttendanceNotOpen,
    SkillMissing,
    CertificationExpired,
    EquipmentRequired,
    EquipmentUnavailable,
    EquipmentFacilityMismatch,
    EquipmentClassMismatch,
}

#[derive(Debug, Clone, Copy)]
pub struct EligibilityEvidence {
    pub employee_active: bool,
    pub facility_assigned: bool,
    pub attendance_open: bool,
    pub required_skill_present: bool,
    pub certification_valid: bool,
    pub equipment_required: bool,
    pub equipment_present: bool,
    pub equipment_available: bool,
    pub equipment_in_facility: bool,
    pub equipment_class_matches: bool,
}

pub fn assess_eligibility(evidence: EligibilityEvidence) -> Result<(), LaborError> {
    let failure = if !evidence.employee_active {
        Some(EligibilityFailure::EmployeeInactive)
    } else if !evidence.facility_assigned {
        Some(EligibilityFailure::FacilityNotAssigned)
    } else if !evidence.attendance_open {
        Some(EligibilityFailure::AttendanceNotOpen)
    } else if !evidence.required_skill_present {
        Some(EligibilityFailure::SkillMissing)
    } else if !evidence.certification_valid {
        Some(EligibilityFailure::CertificationExpired)
    } else if evidence.equipment_required && !evidence.equipment_present {
        Some(EligibilityFailure::EquipmentRequired)
    } else if evidence.equipment_present && !evidence.equipment_available {
        Some(EligibilityFailure::EquipmentUnavailable)
    } else if evidence.equipment_present && !evidence.equipment_in_facility {
        Some(EligibilityFailure::EquipmentFacilityMismatch)
    } else if evidence.equipment_present && !evidence.equipment_class_matches {
        Some(EligibilityFailure::EquipmentClassMismatch)
    } else {
        None
    };
    failure.map_or(Ok(()), |failure| Err(LaborError::Ineligible(failure)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartLaborActivity {
    pub kind: LaborActivityKind,
    pub reference_type: Option<LaborReferenceType>,
    pub reference_id: Option<i64>,
}

pub fn validate_labor_start(
    attendance_status: AttendanceStatus,
    has_active_labor: bool,
    activity: &StartLaborActivity,
) -> Result<(), LaborError> {
    if attendance_status != AttendanceStatus::Open {
        return Err(LaborError::AttendanceNotOpen);
    }
    if has_active_labor {
        return Err(LaborError::LaborAlreadyActive);
    }
    let has_reference =
        activity.reference_type.is_some() && activity.reference_id.is_some_and(|id| id > 0);
    if activity.kind.is_direct() && !has_reference {
        return Err(LaborError::DirectReferenceRequired);
    }
    if !activity.kind.is_direct()
        && (activity.reference_type.is_some() || activity.reference_id.is_some())
    {
        return Err(LaborError::IndirectReferenceForbidden);
    }
    Ok(())
}

pub fn validate_labor_completion(
    status: LaborActivityStatus,
    kind: LaborActivityKind,
    started_at: Timestamp,
    completed_at: Timestamp,
    quantity: Option<LaborQuantity>,
) -> Result<i64, LaborError> {
    if status != LaborActivityStatus::Active {
        return Err(LaborError::LaborNotActive);
    }
    let seconds = (completed_at - started_at).num_seconds();
    if seconds <= 0 {
        return Err(LaborError::InvalidActivityInterval);
    }
    if kind.is_direct() && quantity.is_none() {
        return Err(LaborError::DirectQuantityRequired);
    }
    if !kind.is_direct() && quantity.is_some() {
        return Err(LaborError::IndirectQuantityForbidden);
    }
    Ok(seconds)
}

pub fn validate_attendance_close(
    status: AttendanceStatus,
    clocked_in_at: Timestamp,
    clocked_out_at: Timestamp,
    has_active_labor: bool,
) -> Result<i64, LaborError> {
    if status != AttendanceStatus::Open {
        return Err(LaborError::AttendanceNotOpen);
    }
    if has_active_labor {
        return Err(LaborError::LaborAlreadyActive);
    }
    let seconds = (clocked_out_at - clocked_in_at).num_seconds();
    if seconds <= 0 {
        return Err(LaborError::InvalidAttendanceInterval);
    }
    Ok(seconds)
}

pub fn validate_attendance_correction(
    status: AttendanceStatus,
    corrected_clocked_in_at: Timestamp,
    corrected_clocked_out_at: Timestamp,
) -> Result<i64, LaborError> {
    if status != AttendanceStatus::Closed {
        return Err(LaborError::AttendanceCorrectionRequiresClosedInterval);
    }
    let seconds = (corrected_clocked_out_at - corrected_clocked_in_at).num_seconds();
    if seconds <= 0 {
        return Err(LaborError::InvalidAttendanceInterval);
    }
    Ok(seconds)
}

pub fn validate_activity_correction(
    status: LaborActivityStatus,
    kind: LaborActivityKind,
    actual_seconds: i64,
    quantity: Option<LaborQuantity>,
    exception_seconds: i64,
) -> Result<(), LaborError> {
    if status != LaborActivityStatus::Completed {
        return Err(LaborError::ActivityCorrectionRequiresCompletedActivity);
    }
    if actual_seconds <= 0 {
        return Err(LaborError::InvalidActualSeconds);
    }
    if exception_seconds < 0 || exception_seconds > actual_seconds {
        return Err(LaborError::InvalidExceptionSeconds);
    }
    if kind.is_direct() && quantity.is_none() {
        return Err(LaborError::DirectQuantityRequired);
    }
    if !kind.is_direct() && quantity.is_some() {
        return Err(LaborError::IndirectQuantityForbidden);
    }
    Ok(())
}

pub fn efficiency_basis_points(
    expected_seconds: i64,
    actual_seconds: i64,
) -> Result<i64, LaborError> {
    if actual_seconds <= 0 || expected_seconds < 0 {
        return Err(LaborError::InvalidActualSeconds);
    }
    expected_seconds
        .checked_mul(10_000)
        .map(|value| (value / actual_seconds).min(MAX_LABOR_BASIS_POINTS))
        .ok_or(LaborError::StandardDurationOverflow)
}

pub fn utilization_basis_points(direct_seconds: i64, paid_seconds: i64) -> Result<i64, LaborError> {
    if paid_seconds <= 0 || direct_seconds < 0 || direct_seconds > paid_seconds {
        return Err(LaborError::InvalidUtilizationInterval);
    }
    direct_seconds
        .checked_mul(10_000)
        .map(|value| value / paid_seconds)
        .ok_or(LaborError::StandardDurationOverflow)
}

pub fn elapsed_seconds(started_at: Timestamp, completed_at: Timestamp) -> Result<i64, LaborError> {
    let duration: Duration = completed_at - started_at;
    let seconds = duration.num_seconds();
    if seconds > 0 {
        Ok(seconds)
    } else {
        Err(LaborError::InvalidActivityInterval)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn at(minute: u32) -> Timestamp {
        Utc.with_ymd_and_hms(2026, 8, 14, 9, minute, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn direct_and_indirect_labor_have_distinct_evidence_shapes() {
        let direct = StartLaborActivity {
            kind: LaborActivityKind::Picking,
            reference_type: Some(LaborReferenceType::new("pick_task").unwrap()),
            reference_id: Some(41),
        };
        assert_eq!(
            validate_labor_start(AttendanceStatus::Open, false, &direct),
            Ok(())
        );
        assert_eq!(
            validate_labor_start(
                AttendanceStatus::Open,
                false,
                &StartLaborActivity {
                    kind: LaborActivityKind::Picking,
                    reference_type: None,
                    reference_id: None,
                }
            ),
            Err(LaborError::DirectReferenceRequired)
        );
        assert_eq!(
            validate_labor_start(
                AttendanceStatus::Open,
                false,
                &StartLaborActivity {
                    kind: LaborActivityKind::Break,
                    reference_type: direct.reference_type,
                    reference_id: direct.reference_id,
                }
            ),
            Err(LaborError::IndirectReferenceForbidden)
        );
    }

    #[test]
    fn labor_completion_and_attendance_close_require_positive_nonoverlapping_time() {
        assert_eq!(
            validate_labor_completion(
                LaborActivityStatus::Active,
                LaborActivityKind::Picking,
                at(0),
                at(10),
                Some(LaborQuantity::new(30).unwrap())
            ),
            Ok(600)
        );
        assert_eq!(
            validate_attendance_close(AttendanceStatus::Open, at(0), at(10), true),
            Err(LaborError::LaborAlreadyActive)
        );
    }

    #[test]
    fn standards_generate_exact_explainable_performance() {
        let standard = LaborStandard::new(60, 12).unwrap();
        let expected = standard
            .expected_seconds(LaborQuantity::new(20).unwrap())
            .unwrap();
        assert_eq!(expected, 300);
        assert_eq!(efficiency_basis_points(expected, 240), Ok(12_500));
        assert_eq!(utilization_basis_points(1_800, 2_400), Ok(7_500));
    }

    #[test]
    fn eligibility_fails_closed_in_priority_order() {
        let mut evidence = EligibilityEvidence {
            employee_active: true,
            facility_assigned: true,
            attendance_open: true,
            required_skill_present: true,
            certification_valid: true,
            equipment_required: true,
            equipment_present: true,
            equipment_available: true,
            equipment_in_facility: true,
            equipment_class_matches: true,
        };
        assert_eq!(assess_eligibility(evidence), Ok(()));
        evidence.certification_valid = false;
        assert_eq!(
            assess_eligibility(evidence),
            Err(LaborError::Ineligible(
                EligibilityFailure::CertificationExpired
            ))
        );
    }
}
