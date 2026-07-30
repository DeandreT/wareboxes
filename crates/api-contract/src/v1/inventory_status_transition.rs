use serde::{Deserialize, Serialize};

use super::InventoryBalanceStatus;

/// Auditable business reason for changing the disposition of inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryStatusTransitionReason {
    QualityInspection,
    DamageSuspected,
    DamageConfirmed,
    InspectionPassed,
    InventoryDiscrepancy,
    DiscrepancyResolved,
    RegulatoryRestriction,
    RegulatoryRelease,
    CustomerRequest,
    CustomerRelease,
    Other,
}

/// Moves uncommitted inventory from its current status to another disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInventoryStatusTransitionRequest {
    pub quantity: i64,
    pub to_status: InventoryBalanceStatus,
    pub reason: InventoryStatusTransitionReason,
    pub note: Option<String>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
}

/// Committed journal result for an inventory disposition change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryStatusTransitionResponse {
    pub inventory_transaction_id: i64,
    pub source_inventory_balance_id: i64,
    pub target_inventory_balance_id: i64,
    pub quantity: i64,
    pub from_status: InventoryBalanceStatus,
    pub to_status: InventoryBalanceStatus,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn status_transition_contract_is_exact_and_header_idempotent() {
        let request = CreateInventoryStatusTransitionRequest {
            quantity: 5,
            to_status: InventoryBalanceStatus::Quarantine,
            reason: InventoryStatusTransitionReason::QualityInspection,
            note: Some("Awaiting inspection".into()),
            reference_type: Some("receipt".into()),
            reference_id: Some(81),
        };

        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "quantity": 5,
                "to_status": "quarantine",
                "reason": "quality_inspection",
                "note": "Awaiting inspection",
                "reference_type": "receipt",
                "reference_id": 81
            })
        );
        assert!(
            serde_json::from_value::<CreateInventoryStatusTransitionRequest>(json!({
                "quantity": 5,
                "to_status": "quarantine",
                "reason": "quality_inspection",
                "note": null,
                "reference_type": null,
                "reference_id": null,
                "inventory_balance_id": 42
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<CreateInventoryStatusTransitionRequest>(json!({
                "quantity": 5,
                "to_status": "quarantine",
                "reason": "quality_inspection",
                "note": null,
                "reference_type": null,
                "reference_id": null,
                "idempotency_key": "must-be-a-header"
            }))
            .is_err()
        );
    }

    #[test]
    fn status_transition_response_uses_public_inventory_terms() {
        let response = InventoryStatusTransitionResponse {
            inventory_transaction_id: 91,
            source_inventory_balance_id: 42,
            target_inventory_balance_id: 43,
            quantity: 5,
            from_status: InventoryBalanceStatus::Available,
            to_status: InventoryBalanceStatus::Quarantine,
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "inventory_transaction_id": 91,
                "source_inventory_balance_id": 42,
                "target_inventory_balance_id": 43,
                "quantity": 5,
                "from_status": "available",
                "to_status": "quarantine"
            })
        );
    }
}
