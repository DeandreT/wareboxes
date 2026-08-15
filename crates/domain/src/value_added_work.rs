use serde::{Deserialize, Serialize};

pub const MAX_VALUE_ADDED_WORK_NUMBER_LENGTH: usize = 120;
pub const MAX_VALUE_ADDED_WORK_NOTE_LENGTH: usize = 500;
pub const MAX_VALUE_ADDED_WORK_LINES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValueAddedWorkError {
    #[error("{field} must be nonblank, trimmed, and control-free")]
    InvalidText { field: &'static str },
    #[error("{field} cannot exceed {maximum} characters")]
    TextTooLong { field: &'static str, maximum: usize },
    #[error("value-added quantity must be positive")]
    InvalidQuantity,
    #[error("value-added revision must be positive, got {value}")]
    InvalidRevision { value: i64 },
    #[error("value-added revision cannot advance beyond its supported range")]
    RevisionExhausted,
    #[error("value-added work requires at least one input and one output")]
    MissingLines,
    #[error(
        "value-added work cannot contain more than {MAX_VALUE_ADDED_WORK_LINES} inputs or outputs"
    )]
    TooManyLines,
    #[error("value-added work cannot repeat an input inventory balance")]
    DuplicateInput,
    #[error("relabeling and refurbishment must conserve total quantity")]
    QuantityNotConserved,
    #[error("{kind:?} requires {requirement}")]
    InvalidShape {
        kind: ValueAddedWorkKind,
        requirement: &'static str,
    },
    #[error("value-added transition from {from:?} to {to:?} is not allowed")]
    InvalidTransition {
        from: ValueAddedWorkStatus,
        to: ValueAddedWorkStatus,
    },
}

fn required_text(
    value: impl Into<String>,
    field: &'static str,
    maximum: usize,
) -> Result<String, ValueAddedWorkError> {
    let value = value.into();
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(ValueAddedWorkError::InvalidText { field });
    }
    if value.chars().count() > maximum {
        return Err(ValueAddedWorkError::TextTooLong { field, maximum });
    }
    Ok(value)
}

macro_rules! value_added_text {
    ($name:ident, $field:literal, $maximum:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ValueAddedWorkError> {
                required_text(value, $field, $maximum).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

value_added_text!(
    ValueAddedWorkNumber,
    "value-added work number",
    MAX_VALUE_ADDED_WORK_NUMBER_LENGTH
);
value_added_text!(
    ValueAddedWorkNote,
    "value-added work note",
    MAX_VALUE_ADDED_WORK_NOTE_LENGTH
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ValueAddedQuantity(i64);

impl ValueAddedQuantity {
    pub const fn new(value: i64) -> Result<Self, ValueAddedWorkError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ValueAddedWorkError::InvalidQuantity)
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ValueAddedRevision(i64);

impl ValueAddedRevision {
    pub const fn new(value: i64) -> Result<Self, ValueAddedWorkError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ValueAddedWorkError::InvalidRevision { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn next(self) -> Result<Self, ValueAddedWorkError> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(ValueAddedWorkError::RevisionExhausted),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueAddedWorkKind {
    Relabel,
    Refurbishment,
    Kit,
    Dekit,
    Assembly,
    ValueAddedService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueAddedInventoryStatus {
    Available,
    Hold,
    Damaged,
    Quarantine,
}

impl ValueAddedInventoryStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Hold => "hold",
            Self::Damaged => "damaged",
            Self::Quarantine => "quarantine",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "available" => Some(Self::Available),
            "hold" => Some(Self::Hold),
            "damaged" => Some(Self::Damaged),
            "quarantine" => Some(Self::Quarantine),
            _ => None,
        }
    }
}

impl ValueAddedWorkKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Relabel => "relabel",
            Self::Refurbishment => "refurbishment",
            Self::Kit => "kit",
            Self::Dekit => "dekit",
            Self::Assembly => "assembly",
            Self::ValueAddedService => "value_added_service",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "relabel" => Some(Self::Relabel),
            "refurbishment" => Some(Self::Refurbishment),
            "kit" => Some(Self::Kit),
            "dekit" => Some(Self::Dekit),
            "assembly" => Some(Self::Assembly),
            "value_added_service" => Some(Self::ValueAddedService),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueAddedWorkStatus {
    Draft,
    Released,
    Completed,
    Cancelled,
}

impl ValueAddedWorkStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Released => "released",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "released" => Some(Self::Released),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub const fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Draft, Self::Released)
                | (Self::Draft, Self::Cancelled)
                | (Self::Released, Self::Completed)
                | (Self::Released, Self::Cancelled)
        )
    }

    pub fn require_transition_to(self, target: Self) -> Result<(), ValueAddedWorkError> {
        if self.can_transition_to(target) {
            Ok(())
        } else {
            Err(ValueAddedWorkError::InvalidTransition {
                from: self,
                to: target,
            })
        }
    }
}

pub fn validate_value_added_shape(
    kind: ValueAddedWorkKind,
    input_balance_ids: &[i64],
    output_count: usize,
) -> Result<(), ValueAddedWorkError> {
    if input_balance_ids.is_empty() || output_count == 0 {
        return Err(ValueAddedWorkError::MissingLines);
    }
    if input_balance_ids.len() > MAX_VALUE_ADDED_WORK_LINES
        || output_count > MAX_VALUE_ADDED_WORK_LINES
    {
        return Err(ValueAddedWorkError::TooManyLines);
    }
    let mut sorted = input_balance_ids.to_vec();
    sorted.sort_unstable();
    if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ValueAddedWorkError::DuplicateInput);
    }
    let valid = match kind {
        ValueAddedWorkKind::Relabel | ValueAddedWorkKind::Refurbishment => {
            input_balance_ids.len() == 1 && output_count == 1
        }
        ValueAddedWorkKind::Kit | ValueAddedWorkKind::Assembly => {
            input_balance_ids.len() >= 2 && output_count == 1
        }
        ValueAddedWorkKind::Dekit => input_balance_ids.len() == 1 && output_count >= 2,
        ValueAddedWorkKind::ValueAddedService => true,
    };
    if valid {
        Ok(())
    } else {
        let requirement = match kind {
            ValueAddedWorkKind::Relabel | ValueAddedWorkKind::Refurbishment => {
                "exactly one input and one output"
            }
            ValueAddedWorkKind::Kit | ValueAddedWorkKind::Assembly => {
                "at least two inputs and exactly one output"
            }
            ValueAddedWorkKind::Dekit => "exactly one input and at least two outputs",
            ValueAddedWorkKind::ValueAddedService => unreachable!(),
        };
        Err(ValueAddedWorkError::InvalidShape { kind, requirement })
    }
}

pub fn validate_value_added_quantities(
    kind: ValueAddedWorkKind,
    input_total: i64,
    output_total: i64,
) -> Result<(), ValueAddedWorkError> {
    if input_total <= 0 || output_total <= 0 {
        return Err(ValueAddedWorkError::InvalidQuantity);
    }
    if matches!(
        kind,
        ValueAddedWorkKind::Relabel | ValueAddedWorkKind::Refurbishment
    ) && input_total != output_total
    {
        Err(ValueAddedWorkError::QuantityNotConserved)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_work_shapes_reject_generic_or_ambiguous_recipes() {
        assert!(validate_value_added_shape(ValueAddedWorkKind::Kit, &[1, 2], 1).is_ok());
        assert!(validate_value_added_shape(ValueAddedWorkKind::Dekit, &[1], 2).is_ok());
        assert!(validate_value_added_shape(ValueAddedWorkKind::Relabel, &[1, 2], 1).is_err());
        assert!(validate_value_added_shape(ValueAddedWorkKind::Assembly, &[1, 1], 1).is_err());
        assert!(validate_value_added_quantities(ValueAddedWorkKind::Relabel, 4, 3).is_err());
    }

    #[test]
    fn lifecycle_has_no_shortcut_to_completion() {
        assert!(ValueAddedWorkStatus::Draft
            .require_transition_to(ValueAddedWorkStatus::Released)
            .is_ok());
        assert!(ValueAddedWorkStatus::Draft
            .require_transition_to(ValueAddedWorkStatus::Completed)
            .is_err());
        assert!(ValueAddedWorkStatus::Completed
            .require_transition_to(ValueAddedWorkStatus::Cancelled)
            .is_err());
    }
}
