use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::{CursorPage, OpaqueCursor, PageLimit, Revision};

const MAX_CODE_LENGTH: usize = 32;
const MAX_NAME_LENGTH: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageZonePurpose {
    Receiving,
    Reserve,
    Pick,
    Staging,
    Packing,
    Shipping,
    Quarantine,
    Damage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageZoneStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureStorageZoneRequest {
    pub facility_id: i64,
    pub code: String,
    pub name: String,
    pub purpose: StorageZonePurpose,
    pub travel_sequence: u32,
    pub location_ids: Vec<i64>,
    pub expected_revision: Option<Revision>,
}

impl<'de> Deserialize<'de> for ConfigureStorageZoneRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            facility_id: i64,
            code: String,
            name: String,
            purpose: StorageZonePurpose,
            travel_sequence: u32,
            location_ids: Vec<i64>,
            #[serde(default)]
            expected_revision: Option<Revision>,
        }

        let mut raw = Raw::deserialize(deserializer)?;
        if raw.facility_id <= 0 {
            return Err(D::Error::custom("facility_id must be positive"));
        }
        validate_text(&raw.code, MAX_CODE_LENGTH, "code")?;
        if !raw
            .code
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        {
            return Err(D::Error::custom(
                "code must contain only letters, numbers, '-' or '_'",
            ));
        }
        validate_text(&raw.name, MAX_NAME_LENGTH, "name")?;
        if raw.location_ids.iter().any(|location_id| *location_id <= 0) {
            return Err(D::Error::custom("location_ids must be positive"));
        }
        raw.location_ids.sort_unstable();
        raw.location_ids.dedup();
        if raw.location_ids.is_empty() {
            return Err(D::Error::custom("location_ids must not be empty"));
        }
        Ok(Self {
            facility_id: raw.facility_id,
            code: raw.code,
            name: raw.name,
            purpose: raw.purpose,
            travel_sequence: raw.travel_sequence,
            location_ids: raw.location_ids,
            expected_revision: raw.expected_revision,
        })
    }
}

fn validate_text<E>(value: &str, max: usize, field: &str) -> Result<(), E>
where
    E: serde::de::Error,
{
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > max
        || value.chars().any(char::is_control)
    {
        return Err(E::custom(format!(
            "{field} must be trimmed, nonempty, and at most {max} characters"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetireStorageZoneRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageZoneLocationResponse {
    pub location_id: i64,
    pub barcode: String,
    pub name: Option<String>,
    pub location_type: String,
    pub pickable: bool,
    pub receivable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageZoneResponse {
    pub storage_zone_id: i64,
    pub facility_id: i64,
    pub facility_name: String,
    pub code: String,
    pub name: String,
    pub purpose: StorageZonePurpose,
    pub travel_sequence: u32,
    pub status: StorageZoneStatus,
    pub revision: Revision,
    pub locations: Vec<StorageZoneLocationResponse>,
    pub configured_by: i64,
    pub configured_at: String,
    pub retired_by: Option<i64>,
    pub retired_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct StorageZonePageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facility_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<StorageZonePurpose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<StorageZoneStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type StorageZonePage = CursorPage<StorageZoneResponse>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_is_strict_and_canonicalizes_location_ids() {
        let request: ConfigureStorageZoneRequest = serde_json::from_value(serde_json::json!({
            "facility_id": 3,
            "code": "PICK-A",
            "name": "Fast pick",
            "purpose": "pick",
            "travel_sequence": 10,
            "location_ids": [8, 7, 8]
        }))
        .unwrap();
        assert_eq!(request.location_ids, vec![7, 8]);
        assert!(
            serde_json::from_value::<ConfigureStorageZoneRequest>(serde_json::json!({
                "facility_id": 3,
                "code": "PICK-A",
                "name": "Fast pick",
                "purpose": "pick",
                "travel_sequence": 10,
                "location_ids": [7],
                "capacity": 100
            }))
            .is_err()
        );
    }
}
