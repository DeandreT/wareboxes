use serde::{Deserialize, Serialize};

pub const MAX_EMPLOYEE_IDENTITY_REASON_LENGTH: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkforceError {
    #[error("employee identity change reason must not be blank")]
    BlankReason,
    #[error("employee identity change reason must not contain control characters")]
    ControlCharacter,
    #[error(
        "employee identity change reason exceeds {MAX_EMPLOYEE_IDENTITY_REASON_LENGTH} characters"
    )]
    ReasonTooLong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EmployeeIdentityReason(String);

impl EmployeeIdentityReason {
    pub fn new(value: impl Into<String>) -> Result<Self, WorkforceError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(WorkforceError::BlankReason);
        }
        if value.chars().count() > MAX_EMPLOYEE_IDENTITY_REASON_LENGTH {
            return Err(WorkforceError::ReasonTooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(WorkforceError::ControlCharacter);
        }
        if value.trim() != value {
            return Err(WorkforceError::BlankReason);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmployeeIdentityChangeKind {
    Linked,
    Relinked,
    Unlinked,
}

impl EmployeeIdentityChangeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linked => "linked",
            Self::Relinked => "relinked",
            Self::Unlinked => "unlinked",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_reason_is_trimmed_control_free_and_bounded() {
        assert_eq!(
            EmployeeIdentityReason::new("approved workforce identity")
                .unwrap()
                .as_str(),
            "approved workforce identity"
        );
        assert!(EmployeeIdentityReason::new(" ").is_err());
        assert!(EmployeeIdentityReason::new(" padded ").is_err());
        assert!(EmployeeIdentityReason::new("line\nbreak").is_err());
        assert!(EmployeeIdentityReason::new("x".repeat(501)).is_err());
    }
}
