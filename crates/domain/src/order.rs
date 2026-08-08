use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    #[serde(rename = "awaiting shipment")]
    AwaitingShipment,
    Shipped,
    Cancelled,
    Held,
    Processing,
    #[default]
    Open,
    Void,
}

impl OrderStatus {
    pub const ALL: [Self; 7] = [
        Self::AwaitingShipment,
        Self::Shipped,
        Self::Cancelled,
        Self::Held,
        Self::Processing,
        Self::Open,
        Self::Void,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingShipment => "awaiting shipment",
            Self::Shipped => "shipped",
            Self::Cancelled => "cancelled",
            Self::Held => "held",
            Self::Processing => "processing",
            Self::Open => "open",
            Self::Void => "void",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "awaiting shipment" => Some(Self::AwaitingShipment),
            "shipped" => Some(Self::Shipped),
            "cancelled" => Some(Self::Cancelled),
            "held" => Some(Self::Held),
            "processing" => Some(Self::Processing),
            "open" => Some(Self::Open),
            "void" => Some(Self::Void),
            _ => None,
        }
    }

    pub const fn is_mutable(self) -> bool {
        matches!(self, Self::Cancelled | Self::Held | Self::Open | Self::Void)
    }

    pub const fn place_hold(self) -> Result<Self, OrderHoldTransitionError> {
        match self {
            Self::Open | Self::Held => Ok(Self::Held),
            _ => Err(OrderHoldTransitionError::OrderNotHoldable),
        }
    }

    pub const fn release_hold(
        self,
        active_holds_remaining: bool,
    ) -> Result<Self, OrderHoldTransitionError> {
        if !matches!(self, Self::Held) {
            return Err(OrderHoldTransitionError::OrderNotHeld);
        }
        if active_holds_remaining {
            Ok(Self::Held)
        } else {
            Ok(Self::Open)
        }
    }
}

impl fmt::Display for OrderStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrderHoldReason {
    AddressReview,
    ComplianceReview,
    CustomerRequest,
    InventoryShortage,
    PaymentReview,
    Other,
}

impl OrderHoldReason {
    pub const ALL: [Self; 6] = [
        Self::AddressReview,
        Self::ComplianceReview,
        Self::CustomerRequest,
        Self::InventoryShortage,
        Self::PaymentReview,
        Self::Other,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AddressReview => "address_review",
            Self::ComplianceReview => "compliance_review",
            Self::CustomerRequest => "customer_request",
            Self::InventoryShortage => "inventory_shortage",
            Self::PaymentReview => "payment_review",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "address_review" => Some(Self::AddressReview),
            "compliance_review" => Some(Self::ComplianceReview),
            "customer_request" => Some(Self::CustomerRequest),
            "inventory_shortage" => Some(Self::InventoryShortage),
            "payment_review" => Some(Self::PaymentReview),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

impl fmt::Display for OrderHoldReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OrderHoldTransitionError {
    #[error("order cannot be held in its current state")]
    OrderNotHoldable,
    #[error("order is not held")]
    OrderNotHeld,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_reasons_round_trip_through_persistence_values() {
        for reason in OrderHoldReason::ALL {
            assert_eq!(OrderHoldReason::parse(reason.as_str()), Some(reason));
        }
    }

    #[test]
    fn order_holds_only_transition_safe_workflow_states() {
        assert_eq!(OrderStatus::Open.place_hold(), Ok(OrderStatus::Held));
        assert_eq!(OrderStatus::Held.place_hold(), Ok(OrderStatus::Held));
        assert_eq!(
            OrderStatus::Processing.place_hold(),
            Err(OrderHoldTransitionError::OrderNotHoldable)
        );
        assert_eq!(OrderStatus::Held.release_hold(true), Ok(OrderStatus::Held));
        assert_eq!(OrderStatus::Held.release_hold(false), Ok(OrderStatus::Open));
        assert_eq!(
            OrderStatus::Open.release_hold(false),
            Err(OrderHoldTransitionError::OrderNotHeld)
        );
    }
}
