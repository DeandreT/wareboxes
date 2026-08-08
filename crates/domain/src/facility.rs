use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const MAX_FACILITY_ORIGIN_NAME_LENGTH: usize = 200;
pub const MAX_FACILITY_ORIGIN_COMPANY_LENGTH: usize = 200;
pub const MAX_FACILITY_ORIGIN_ADDRESS_LINE_LENGTH: usize = 200;
pub const MAX_FACILITY_ORIGIN_CITY_LENGTH: usize = 100;
pub const MAX_FACILITY_ORIGIN_STATE_LENGTH: usize = 100;
pub const MAX_FACILITY_ORIGIN_POSTAL_CODE_LENGTH: usize = 32;
pub const MAX_FACILITY_ORIGIN_COUNTRY_LENGTH: usize = 100;
pub const MAX_FACILITY_ORIGIN_PHONE_LENGTH: usize = 64;
pub const MAX_FACILITY_ORIGIN_EMAIL_LENGTH: usize = 254;

/// Positive revision used for optimistic facility configuration writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct FacilityRevision(i64);

impl FacilityRevision {
    pub const fn new(value: i64) -> Result<Self, FacilityShippingOriginError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(FacilityShippingOriginError::InvalidRevision { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl<'de> Deserialize<'de> for FacilityRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FacilityShippingOriginField {
    Name,
    Company,
    Line1,
    Line2,
    City,
    State,
    PostalCode,
    Country,
    Phone,
    Email,
}

impl fmt::Display for FacilityShippingOriginField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Name => "facility shipping origin name",
            Self::Company => "facility shipping origin company",
            Self::Line1 => "facility shipping origin address line 1",
            Self::Line2 => "facility shipping origin address line 2",
            Self::City => "facility shipping origin city",
            Self::State => "facility shipping origin state",
            Self::PostalCode => "facility shipping origin postal code",
            Self::Country => "facility shipping origin country",
            Self::Phone => "facility shipping origin phone",
            Self::Email => "facility shipping origin email",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FacilityShippingOriginError {
    #[error("facility shipping origin requires a name or company")]
    MissingName,
    #[error("{field} must be trimmed and nonblank")]
    InvalidText { field: FacilityShippingOriginField },
    #[error("{field} cannot exceed {maximum} characters")]
    TextTooLong {
        field: FacilityShippingOriginField,
        maximum: usize,
    },
    #[error("facility revision must be a positive integer, got {value}")]
    InvalidRevision { value: i64 },
}

/// Complete carrier-facing origin copied when a shipment is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacilityShippingOrigin {
    name: Option<String>,
    company: Option<String>,
    line1: String,
    line2: Option<String>,
    city: String,
    state: Option<String>,
    postal_code: String,
    country: String,
    phone: Option<String>,
    email: Option<String>,
}

impl FacilityShippingOrigin {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: Option<String>,
        company: Option<String>,
        line1: String,
        line2: Option<String>,
        city: String,
        state: Option<String>,
        postal_code: String,
        country: String,
        phone: Option<String>,
        email: Option<String>,
    ) -> Result<Self, FacilityShippingOriginError> {
        validate_optional(
            name.as_deref(),
            FacilityShippingOriginField::Name,
            MAX_FACILITY_ORIGIN_NAME_LENGTH,
        )?;
        validate_optional(
            company.as_deref(),
            FacilityShippingOriginField::Company,
            MAX_FACILITY_ORIGIN_COMPANY_LENGTH,
        )?;
        if name.is_none() && company.is_none() {
            return Err(FacilityShippingOriginError::MissingName);
        }
        validate_required(
            &line1,
            FacilityShippingOriginField::Line1,
            MAX_FACILITY_ORIGIN_ADDRESS_LINE_LENGTH,
        )?;
        validate_optional(
            line2.as_deref(),
            FacilityShippingOriginField::Line2,
            MAX_FACILITY_ORIGIN_ADDRESS_LINE_LENGTH,
        )?;
        validate_required(
            &city,
            FacilityShippingOriginField::City,
            MAX_FACILITY_ORIGIN_CITY_LENGTH,
        )?;
        validate_optional(
            state.as_deref(),
            FacilityShippingOriginField::State,
            MAX_FACILITY_ORIGIN_STATE_LENGTH,
        )?;
        validate_required(
            &postal_code,
            FacilityShippingOriginField::PostalCode,
            MAX_FACILITY_ORIGIN_POSTAL_CODE_LENGTH,
        )?;
        validate_required(
            &country,
            FacilityShippingOriginField::Country,
            MAX_FACILITY_ORIGIN_COUNTRY_LENGTH,
        )?;
        validate_optional(
            phone.as_deref(),
            FacilityShippingOriginField::Phone,
            MAX_FACILITY_ORIGIN_PHONE_LENGTH,
        )?;
        validate_optional(
            email.as_deref(),
            FacilityShippingOriginField::Email,
            MAX_FACILITY_ORIGIN_EMAIL_LENGTH,
        )?;
        Ok(Self {
            name,
            company,
            line1,
            line2,
            city,
            state,
            postal_code,
            country,
            phone,
            email,
        })
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn company(&self) -> Option<&str> {
        self.company.as_deref()
    }

    pub fn line1(&self) -> &str {
        &self.line1
    }

    pub fn line2(&self) -> Option<&str> {
        self.line2.as_deref()
    }

    pub fn city(&self) -> &str {
        &self.city
    }

    pub fn state(&self) -> Option<&str> {
        self.state.as_deref()
    }

    pub fn postal_code(&self) -> &str {
        &self.postal_code
    }

    pub fn country(&self) -> &str {
        &self.country
    }

    pub fn phone(&self) -> Option<&str> {
        self.phone.as_deref()
    }

    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }
}

fn validate_required(
    value: &str,
    field: FacilityShippingOriginField,
    maximum: usize,
) -> Result<(), FacilityShippingOriginError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(FacilityShippingOriginError::InvalidText { field });
    }
    if value.chars().count() > maximum {
        return Err(FacilityShippingOriginError::TextTooLong { field, maximum });
    }
    Ok(())
}

fn validate_optional(
    value: Option<&str>,
    field: FacilityShippingOriginField,
    maximum: usize,
) -> Result<(), FacilityShippingOriginError> {
    if let Some(value) = value {
        validate_required(value, field, maximum)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_origin_accepts_name_or_company_and_optional_region() {
        let origin = FacilityShippingOrigin::new(
            None,
            Some("Wareboxes Fulfillment".into()),
            "100 Distribution Way".into(),
            None,
            "Reno".into(),
            None,
            "89502".into(),
            "US".into(),
            None,
            None,
        )
        .unwrap();
        assert_eq!(origin.name(), None);
        assert_eq!(origin.company(), Some("Wareboxes Fulfillment"));
        assert_eq!(origin.state(), None);
    }

    #[test]
    fn incomplete_or_untrimmed_origins_are_rejected() {
        let missing_name = FacilityShippingOrigin::new(
            None,
            None,
            "100 Distribution Way".into(),
            None,
            "Reno".into(),
            Some("NV".into()),
            "89502".into(),
            "US".into(),
            None,
            None,
        );
        assert_eq!(missing_name, Err(FacilityShippingOriginError::MissingName));

        let untrimmed = FacilityShippingOrigin::new(
            Some("Shipping".into()),
            None,
            " 100 Distribution Way".into(),
            None,
            "Reno".into(),
            None,
            "89502".into(),
            "US".into(),
            None,
            None,
        );
        assert!(matches!(
            untrimmed,
            Err(FacilityShippingOriginError::InvalidText {
                field: FacilityShippingOriginField::Line1
            })
        ));
    }

    #[test]
    fn facility_revisions_are_positive_and_checked() {
        assert_eq!(
            FacilityRevision::new(4)
                .unwrap()
                .checked_next()
                .unwrap()
                .get(),
            5
        );
        assert!(FacilityRevision::new(0).is_err());
        assert_eq!(
            FacilityRevision::new(i64::MAX).unwrap().checked_next(),
            None
        );
    }
}
