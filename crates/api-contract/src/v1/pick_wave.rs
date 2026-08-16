use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use super::{ConfigurationScope, CursorPage, OpaqueCursor, PageLimit, Revision};

pub const MAX_PICK_WAVE_NAME_LENGTH: usize = 100;
pub const MAX_PICK_WAVE_CANCELLATION_NOTE_LENGTH: usize = 500;
pub const PRODUCT_DEFAULT_WAVE_POLICY_HASH: &str =
    "03e485c29e6c4e032786157f4f1e216bd741a35ef6f6c3895b35e9c579f443b9";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WavePolicySource {
    ProductDefault,
    Configuration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WavePolicyExpectation {
    pub source: WavePolicySource,
    pub configuration_id: Option<i64>,
    pub configuration_revision: Option<i64>,
    pub policy_hash: String,
}

impl WavePolicyExpectation {
    pub fn product_default() -> Self {
        Self {
            source: WavePolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            policy_hash: PRODUCT_DEFAULT_WAVE_POLICY_HASH.to_owned(),
        }
    }

    fn validate(&self) -> Result<(), &'static str> {
        let identity_is_valid = match self.source {
            WavePolicySource::ProductDefault => {
                self.configuration_id.is_none() && self.configuration_revision.is_none()
            }
            WavePolicySource::Configuration => {
                self.configuration_id.is_some_and(|id| id > 0)
                    && self
                        .configuration_revision
                        .is_some_and(|revision| revision > 0)
            }
        };
        if !identity_is_valid {
            return Err("wave policy identity is invalid");
        }
        if self.policy_hash.len() != 64
            || !self
                .policy_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("wave policy hash must be lowercase SHA-256 hex");
        }
        if self.source == WavePolicySource::ProductDefault
            && self.policy_hash != PRODUCT_DEFAULT_WAVE_POLICY_HASH
        {
            return Err("wave product-default policy hash is invalid");
        }
        Ok(())
    }
}

impl Default for WavePolicyExpectation {
    fn default() -> Self {
        Self::product_default()
    }
}

impl<'de> Deserialize<'de> for WavePolicyExpectation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            source: WavePolicySource,
            configuration_id: Option<i64>,
            configuration_revision: Option<i64>,
            policy_hash: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        let value = Self {
            source: raw.source,
            configuration_id: raw.configuration_id,
            configuration_revision: raw.configuration_revision,
            policy_hash: raw.policy_hash,
        };
        value.validate().map_err(D::Error::custom)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WavePolicyResponse {
    pub source: WavePolicySource,
    pub configuration_id: Option<i64>,
    pub configuration_revision: Option<i64>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub max_orders: u32,
    pub require_complete_allocation: bool,
    pub policy_hash: String,
}

impl WavePolicyResponse {
    pub fn expectation(&self) -> WavePolicyExpectation {
        WavePolicyExpectation {
            source: self.source,
            configuration_id: self.configuration_id,
            configuration_revision: self.configuration_revision,
            policy_hash: self.policy_hash.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickWaveStatus {
    Planned,
    Released,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickWaveCancellationReason {
    OperationalChange,
    CapacityConstraint,
    OrderChange,
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickWaveSort {
    Name,
    Status,
    Orders,
    Tasks,
    Units,
    #[default]
    PlannedAt,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickWaveSortDirection {
    Asc,
    #[default]
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanPickWaveOrderRequest {
    pub order_id: i64,
    pub expected_revision: Revision,
    pub sequence: u32,
    #[serde(default)]
    pub expected_policy: WavePolicyExpectation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvePickWavePolicyOrderRequest {
    pub order_id: i64,
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvePickWavePoliciesRequest {
    pub facility_id: i64,
    pub orders: Vec<ResolvePickWavePolicyOrderRequest>,
}

impl<'de> Deserialize<'de> for ResolvePickWavePoliciesRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            facility_id: i64,
            orders: Vec<ResolvePickWavePolicyOrderRequest>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.facility_id <= 0
            || raw.orders.is_empty()
            || raw.orders.iter().any(|order| order.order_id <= 0)
        {
            return Err(D::Error::custom("wave policy resolution scope is invalid"));
        }
        Ok(Self {
            facility_id: raw.facility_id,
            orders: raw.orders,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickWavePolicyResolutionResponse {
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub policy: WavePolicyResponse,
}

pub type PickWavePolicyResolutionsResponse = Vec<PickWavePolicyResolutionResponse>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanPickWaveRequest {
    pub facility_id: i64,
    pub destination_location_id: i64,
    pub name: String,
    pub orders: Vec<PlanPickWaveOrderRequest>,
}

impl<'de> Deserialize<'de> for PlanPickWaveRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            facility_id: i64,
            destination_location_id: i64,
            name: String,
            orders: Vec<PlanPickWaveOrderRequest>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.facility_id <= 0 || raw.destination_location_id <= 0 {
            return Err(serde::de::Error::custom(
                "facility and destination IDs must be positive",
            ));
        }
        if raw.name.is_empty()
            || raw.name.trim() != raw.name
            || raw.name.chars().count() > MAX_PICK_WAVE_NAME_LENGTH
            || raw.name.chars().any(char::is_control)
        {
            return Err(serde::de::Error::custom("pick wave name is invalid"));
        }
        if raw.orders.is_empty()
            || raw
                .orders
                .iter()
                .any(|order| order.order_id <= 0 || order.sequence == 0)
        {
            return Err(serde::de::Error::custom("pick wave orders are invalid"));
        }
        Ok(Self {
            facility_id: raw.facility_id,
            destination_location_id: raw.destination_location_id,
            name: raw.name,
            orders: raw.orders,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePickWaveRequest {
    pub expected_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelPickWaveRequest {
    pub expected_revision: Revision,
    pub reason: PickWaveCancellationReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl<'de> Deserialize<'de> for CancelPickWaveRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            expected_revision: Revision,
            reason: PickWaveCancellationReason,
            #[serde(default)]
            note: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.note.as_ref().is_some_and(|note| {
            note.is_empty()
                || note.trim() != note
                || note.chars().count() > MAX_PICK_WAVE_CANCELLATION_NOTE_LENGTH
                || note.chars().any(char::is_control)
        }) || (raw.reason == PickWaveCancellationReason::Other && raw.note.is_none())
        {
            return Err(serde::de::Error::custom(
                "pick wave cancellation note is invalid",
            ));
        }
        Ok(Self {
            expected_revision: raw.expected_revision,
            reason: raw.reason,
            note: raw.note,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickWaveOrderResponse {
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub order_key: String,
    pub sequence: u32,
    pub expected_revision: Revision,
    pub resulting_revision: Option<Revision>,
    pub release_id: Option<i64>,
    pub status: String,
    pub allocation_count: i64,
    pub pick_task_count: i64,
    pub released_quantity: i64,
    pub wave_policy: WavePolicyResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickWaveResponse {
    pub wave_id: i64,
    pub facility_id: i64,
    pub facility_name: String,
    pub destination_location_id: i64,
    pub destination_location_name: String,
    pub name: String,
    pub status: PickWaveStatus,
    pub revision: Revision,
    pub order_count: i64,
    pub allocation_count: i64,
    pub pick_task_count: i64,
    pub released_quantity: i64,
    pub orders: Vec<PickWaveOrderResponse>,
    pub planned_by: i64,
    pub planned_at: String,
    pub released_by: Option<i64>,
    pub released_at: Option<String>,
    pub cancelled_by: Option<i64>,
    pub cancelled_at: Option<String>,
    pub cancellation_reason: Option<PickWaveCancellationReason>,
    pub cancellation_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickWavePageRequest {
    #[serde(default)]
    pub facility_id: Option<i64>,
    #[serde(default)]
    pub status: Option<PickWaveStatus>,
    #[serde(default)]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub sort: PickWaveSort,
    #[serde(default)]
    pub direction: PickWaveSortDirection,
    #[serde(default)]
    pub limit: PageLimit,
}

pub type PickWavePage = CursorPage<PickWaveResponse>;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn plan_is_strict_and_contains_only_server_checkable_preconditions() {
        let request = serde_json::from_value::<PlanPickWaveRequest>(json!({
            "facility_id": 2,
            "destination_location_id": 3,
            "name": "AM parcel",
            "orders": [{"order_id": 4, "expected_revision": 2, "sequence": 1}]
        }))
        .unwrap();
        assert_eq!(request.orders.len(), 1);
        assert_eq!(
            request.orders[0].expected_policy,
            WavePolicyExpectation::product_default()
        );
        assert!(serde_json::from_value::<PlanPickWaveRequest>(json!({
            "facility_id": 2,
            "destination_location_id": 3,
            "name": "AM parcel",
            "orders": [{"order_id": 4, "expected_revision": 2, "sequence": 1}],
            "tenant_id": 9
        }))
        .is_err());
    }

    #[test]
    fn wave_policy_expectations_are_exact_and_fail_closed() {
        let configured = serde_json::from_value::<WavePolicyExpectation>(json!({
            "source": "configuration",
            "configuration_id": 8,
            "configuration_revision": 4,
            "policy_hash": "a".repeat(64)
        }))
        .unwrap();
        assert_eq!(configured.configuration_id, Some(8));
        assert!(serde_json::from_value::<WavePolicyExpectation>(json!({
            "source": "configuration",
            "configuration_revision": 4,
            "policy_hash": "a".repeat(64)
        }))
        .is_err());
        assert!(serde_json::from_value::<WavePolicyExpectation>(json!({
            "source": "product_default",
            "configuration_id": 8,
            "policy_hash": PRODUCT_DEFAULT_WAVE_POLICY_HASH
        }))
        .is_err());
        assert!(serde_json::from_value::<WavePolicyExpectation>(json!({
            "source": "product_default",
            "policy_hash": "ABC"
        }))
        .is_err());
    }

    #[test]
    fn policy_resolution_requires_positive_scope_and_members() {
        assert!(
            serde_json::from_value::<ResolvePickWavePoliciesRequest>(json!({
                "facility_id": 2,
                "orders": [{"order_id": 4, "expected_revision": 2}]
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<ResolvePickWavePoliciesRequest>(json!({
                "facility_id": 2,
                "orders": []
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ResolvePickWavePoliciesRequest>(json!({
                "facility_id": 0,
                "orders": [{"order_id": 4, "expected_revision": 2}]
            }))
            .is_err()
        );
    }

    #[test]
    fn other_cancellation_requires_a_note() {
        assert!(serde_json::from_value::<CancelPickWaveRequest>(json!({
            "expected_revision": 1,
            "reason": "other"
        }))
        .is_err());
    }
}
