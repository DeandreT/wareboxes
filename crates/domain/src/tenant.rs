use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    #[default]
    Active,
    Suspended,
}

impl TenantStatus {
    pub const ALL: [Self; 2] = [Self::Active, Self::Suspended];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            _ => None,
        }
    }
}

impl fmt::Display for TenantStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::TenantStatus;

    #[test]
    fn every_status_round_trips_through_its_persisted_value() {
        let cases = [
            (TenantStatus::Active, "active"),
            (TenantStatus::Suspended, "suspended"),
        ];

        assert_eq!(
            cases.map(|(status, _)| status),
            TenantStatus::ALL,
            "the test cases must cover every tenant status"
        );

        for (status, persisted_value) in cases {
            assert_eq!(status.as_str(), persisted_value);
            assert_eq!(status.to_string(), persisted_value);
            assert_eq!(TenantStatus::parse(persisted_value), Some(status));
        }
    }

    #[test]
    fn parsing_rejects_values_outside_the_domain_vocabulary() {
        for value in ["", "ACTIVE", "deleted", " active", "active "] {
            assert_eq!(TenantStatus::parse(value), None);
        }
    }

    #[test]
    fn active_is_the_default_status() {
        assert_eq!(TenantStatus::default(), TenantStatus::Active);
    }
}
