use serde::{Deserialize, Serialize};

use super::{BackorderPolicyResponse, ConfigurationScope, Revision};

pub const PRODUCT_DEFAULT_ALLOCATION_POLICY_HASH: &str =
    "6090a99a06ea2e049d7321d5cf2b8f462c6d6e6e2ca527ae87657a7a5fd9d156";

/// Allocation policy selected for an order planning run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderAllocationStrategy {
    Fifo,
    Fefo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationPolicySource {
    ProductDefault,
    Configuration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AllocationPolicyReference {
    pub source: AllocationPolicySource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration_revision: Option<Revision>,
    pub policy_hash: String,
}

impl AllocationPolicyReference {
    pub fn product_default() -> Self {
        Self {
            source: AllocationPolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            policy_hash: PRODUCT_DEFAULT_ALLOCATION_POLICY_HASH.to_owned(),
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let identity_is_valid = match self.source {
            AllocationPolicySource::ProductDefault => {
                self.configuration_id.is_none() && self.configuration_revision.is_none()
            }
            AllocationPolicySource::Configuration => {
                self.configuration_id.is_some_and(|id| id > 0)
                    && self.configuration_revision.is_some()
            }
        };
        if !identity_is_valid {
            return Err("allocation policy identity is inconsistent");
        }
        if self.policy_hash.len() != 64
            || !self
                .policy_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("allocation policy hash must be 64 lowercase hexadecimal characters");
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for AllocationPolicyReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawReference {
            source: AllocationPolicySource,
            #[serde(default)]
            configuration_id: Option<i64>,
            #[serde(default)]
            configuration_revision: Option<Revision>,
            policy_hash: String,
        }

        let raw = RawReference::deserialize(deserializer)?;
        let reference = Self {
            source: raw.source,
            configuration_id: raw.configuration_id,
            configuration_revision: raw.configuration_revision,
            policy_hash: raw.policy_hash,
        };
        reference.validate().map_err(serde::de::Error::custom)?;
        Ok(reference)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationPolicyResponse {
    pub source: AllocationPolicySource,
    pub configuration_id: Option<i64>,
    pub configuration_revision: Option<Revision>,
    pub configuration_scope: Option<ConfigurationScope>,
    pub strategy: OrderAllocationStrategy,
    pub allow_partial: bool,
    pub require_complete_line: bool,
    pub policy_hash: String,
}

impl AllocationPolicyResponse {
    pub fn reference(&self) -> AllocationPolicyReference {
        AllocationPolicyReference {
            source: self.source,
            configuration_id: self.configuration_id,
            configuration_revision: self.configuration_revision,
            policy_hash: self.policy_hash.clone(),
        }
    }
}

/// Parameters for an optimistic, replay-safe order allocation command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanOrderAllocationRequest {
    pub facility_id: i64,
    pub expected_revision: Revision,
    pub expected_policy: AllocationPolicyReference,
}

/// Facility-specific parameters for the allocation-readiness query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderAllocationReadinessRequest {
    pub facility_id: i64,
}

/// Cumulative outcome after evaluating all order demand lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderAllocationOutcome {
    FullyAllocated,
    PartiallyAllocated,
    NotAllocated,
}

/// Why a positive demand quantity remains unallocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderAllocationShortageReason {
    NoEligibleInventory,
    InsufficientEligibleInventory,
}

/// Concrete active stock assignment visible after allocation planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderAllocationDetailResponse {
    pub allocation_id: i64,
    pub reservation_id: i64,
    pub inventory_balance_id: i64,
    pub item_batch_id: i64,
    pub location_id: i64,
    pub location_name: Option<String>,
    pub location_barcode: Option<String>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    /// RFC 3339 timestamp, or `None` for stock without an expiration.
    pub expiration: Option<String>,
    pub quantity: i64,
}

/// Cumulative reservation and concrete allocation state for one demand line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderAllocationLineResponse {
    pub order_line_id: i64,
    pub line_key: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub uom: String,
    pub original_demand_quantity: i64,
    pub backordered_quantity: i64,
    pub demand_quantity: i64,
    pub reservation_id: Option<i64>,
    pub reserved_quantity: i64,
    pub allocated_quantity: i64,
    pub shortage_quantity: i64,
    pub shortage_reason: Option<OrderAllocationShortageReason>,
    pub allocations: Vec<OrderAllocationDetailResponse>,
}

/// Replay-stable result of one committed allocation planning run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanOrderAllocationResponse {
    pub allocation_run_id: i64,
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub policy: AllocationPolicyResponse,
    pub strategy: OrderAllocationStrategy,
    pub outcome: OrderAllocationOutcome,
    pub revision: Revision,
    pub newly_allocated_quantity: i64,
    pub original_demand_quantity: i64,
    pub backordered_quantity: i64,
    pub demand_quantity: i64,
    pub allocated_quantity: i64,
    pub shortage_quantity: i64,
    pub lines: Vec<OrderAllocationLineResponse>,
}

/// Operator readiness of another allocation run for the selected facility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderAllocationReadinessStatus {
    Ready,
    AlreadyFullyAllocated,
    Blocked,
}

/// Typed reason the selected order cannot currently be allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderAllocationReadinessBlocker {
    ActiveHold,
    CrossDockInProgress,
    OrderStatusNotAllocatable,
    FacilityNotEligible,
}

/// Facility available to the order owner and the actor's current site scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderAllocationFacilityResponse {
    pub facility_id: i64,
    pub facility_name: String,
}

/// Current order state used to render and gate the allocation workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderAllocationReadinessResponse {
    pub order_id: i64,
    pub inventory_owner_id: i64,
    pub order_key: String,
    pub facility_id: i64,
    pub eligible_facilities: Vec<OrderAllocationFacilityResponse>,
    pub backorder_policy: Option<BackorderPolicyResponse>,
    pub revision: Revision,
    pub status: OrderAllocationReadinessStatus,
    pub blocking_reasons: Vec<OrderAllocationReadinessBlocker>,
    pub policy: AllocationPolicyResponse,
    pub strategy: OrderAllocationStrategy,
    pub outcome: OrderAllocationOutcome,
    pub original_demand_quantity: i64,
    pub backordered_quantity: i64,
    pub demand_quantity: i64,
    pub reserved_quantity: i64,
    pub allocated_quantity: i64,
    pub shortage_quantity: i64,
    pub lines: Vec<OrderAllocationLineResponse>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn allocation_detail() -> OrderAllocationDetailResponse {
        OrderAllocationDetailResponse {
            allocation_id: 31,
            reservation_id: 22,
            inventory_balance_id: 42,
            item_batch_id: 52,
            location_id: 62,
            location_name: Some("Forward pick A-01".into()),
            location_barcode: Some("A-01".into()),
            license_plate_id: Some(72),
            license_plate_barcode: Some("LP-00072".into()),
            lot: Some("LOT-7".into()),
            serial: None,
            expiration: Some("2027-08-10T00:00:00+00:00".into()),
            quantity: 5,
        }
    }

    fn line() -> OrderAllocationLineResponse {
        OrderAllocationLineResponse {
            order_line_id: 12,
            line_key: "1".into(),
            item_id: 41,
            item_description: Some("Case-picked item".into()),
            uom: "case".into(),
            original_demand_quantity: 8,
            backordered_quantity: 0,
            demand_quantity: 8,
            reservation_id: Some(22),
            reserved_quantity: 8,
            allocated_quantity: 5,
            shortage_quantity: 3,
            shortage_reason: Some(OrderAllocationShortageReason::InsufficientEligibleInventory),
            allocations: vec![allocation_detail()],
        }
    }

    fn product_policy() -> AllocationPolicyResponse {
        AllocationPolicyResponse {
            source: AllocationPolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            configuration_scope: None,
            strategy: OrderAllocationStrategy::Fefo,
            allow_partial: true,
            require_complete_line: false,
            policy_hash: "a".repeat(64),
        }
    }

    #[test]
    fn allocation_command_is_strict_and_revision_validated() {
        let request = serde_json::from_value::<PlanOrderAllocationRequest>(json!({
            "facility_id": 8,
            "expected_revision": 3,
            "expected_policy": {
                "source": "product_default",
                "policy_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        }))
        .unwrap();
        assert_eq!(request.expected_revision.get(), 3);
        assert_eq!(
            request.expected_policy.source,
            AllocationPolicySource::ProductDefault
        );

        assert!(serde_json::from_value::<PlanOrderAllocationRequest>(json!({
            "facility_id": 8,
            "expected_revision": 0,
            "expected_policy": {
                "source": "product_default",
                "policy_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        }))
        .is_err());
        assert!(serde_json::from_value::<PlanOrderAllocationRequest>(json!({
            "facility_id": 8,
            "expected_revision": 3,
            "expected_policy": {
                "source": "product_default",
                "policy_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            },
            "inventory_owner_id": 9
        }))
        .is_err());
        assert!(serde_json::from_value::<PlanOrderAllocationRequest>(json!({
            "facility_id": 8,
            "expected_revision": 3,
            "strategy": "fifo"
        }))
        .is_err());
        assert!(serde_json::from_value::<PlanOrderAllocationRequest>(json!({
            "facility_id": 8,
            "expected_revision": 3,
            "expected_policy": {
                "source": "configuration",
                "policy_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        }))
        .is_err());
    }

    #[test]
    fn command_response_exposes_cumulative_traceable_allocation_state() {
        let response = PlanOrderAllocationResponse {
            allocation_run_id: 81,
            order_id: 7,
            inventory_owner_id: 9,
            facility_id: 8,
            policy: product_policy(),
            strategy: OrderAllocationStrategy::Fefo,
            outcome: OrderAllocationOutcome::PartiallyAllocated,
            revision: Revision::new(4).unwrap(),
            newly_allocated_quantity: 5,
            original_demand_quantity: 8,
            backordered_quantity: 0,
            demand_quantity: 8,
            allocated_quantity: 5,
            shortage_quantity: 3,
            lines: vec![line()],
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "allocation_run_id": 81,
                "order_id": 7,
                "inventory_owner_id": 9,
                "facility_id": 8,
                "policy": {
                    "source": "product_default",
                    "configuration_id": null,
                    "configuration_revision": null,
                    "configuration_scope": null,
                    "strategy": "fefo",
                    "allow_partial": true,
                    "require_complete_line": false,
                    "policy_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                },
                "strategy": "fefo",
                "outcome": "partially_allocated",
                "revision": 4,
                "newly_allocated_quantity": 5,
                "original_demand_quantity": 8,
                "backordered_quantity": 0,
                "demand_quantity": 8,
                "allocated_quantity": 5,
                "shortage_quantity": 3,
                "lines": [{
                    "order_line_id": 12,
                    "line_key": "1",
                    "item_id": 41,
                    "item_description": "Case-picked item",
                    "uom": "case",
                    "original_demand_quantity": 8,
                    "backordered_quantity": 0,
                    "demand_quantity": 8,
                    "reservation_id": 22,
                    "reserved_quantity": 8,
                    "allocated_quantity": 5,
                    "shortage_quantity": 3,
                    "shortage_reason": "insufficient_eligible_inventory",
                    "allocations": [{
                        "allocation_id": 31,
                        "reservation_id": 22,
                        "inventory_balance_id": 42,
                        "item_batch_id": 52,
                        "location_id": 62,
                        "location_name": "Forward pick A-01",
                        "location_barcode": "A-01",
                        "license_plate_id": 72,
                        "license_plate_barcode": "LP-00072",
                        "lot": "LOT-7",
                        "serial": null,
                        "expiration": "2027-08-10T00:00:00+00:00",
                        "quantity": 5
                    }]
                }]
            })
        );
    }

    #[test]
    fn readiness_returns_only_scoped_owner_facilities_and_exact_nested_state() {
        let response = OrderAllocationReadinessResponse {
            order_id: 7,
            inventory_owner_id: 9,
            order_key: "SO-1001".into(),
            facility_id: 8,
            eligible_facilities: vec![OrderAllocationFacilityResponse {
                facility_id: 8,
                facility_name: "Reno DC".into(),
            }],
            backorder_policy: None,
            revision: Revision::new(4).unwrap(),
            status: OrderAllocationReadinessStatus::Ready,
            blocking_reasons: Vec::new(),
            policy: product_policy(),
            strategy: OrderAllocationStrategy::Fefo,
            outcome: OrderAllocationOutcome::PartiallyAllocated,
            original_demand_quantity: 8,
            backordered_quantity: 0,
            demand_quantity: 8,
            reserved_quantity: 8,
            allocated_quantity: 5,
            shortage_quantity: 3,
            lines: vec![line()],
        };
        let value = serde_json::to_value(&response).unwrap();

        assert_eq!(value["eligible_facilities"][0]["facility_name"], "Reno DC");
        assert_eq!(value["lines"][0]["allocations"][0]["location_id"], 62);
        assert_eq!(
            serde_json::from_value::<OrderAllocationReadinessResponse>(value).unwrap(),
            response
        );

        assert!(
            serde_json::from_value::<OrderAllocationReadinessResponse>(json!({
                "order_id": 7,
                "inventory_owner_id": 9,
                "order_key": "SO-1001",
                "facility_id": 8,
                "eligible_facilities": [{
                    "facility_id": 8,
                    "facility_name": "Reno DC",
                    "tenant_id": 1
                }],
                "backorder_policy": null,
                "revision": 4,
                "status": "ready",
                "blocking_reasons": [],
                "policy": {
                    "source": "product_default",
                    "configuration_id": null,
                    "configuration_revision": null,
                    "configuration_scope": null,
                    "strategy": "fefo",
                    "allow_partial": true,
                    "require_complete_line": false,
                    "policy_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                },
                "strategy": "fefo",
                "outcome": "not_allocated",
                "original_demand_quantity": 8,
                "backordered_quantity": 0,
                "demand_quantity": 8,
                "reserved_quantity": 0,
                "allocated_quantity": 0,
                "shortage_quantity": 8,
                "lines": []
            }))
            .is_err()
        );
    }
}
