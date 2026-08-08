use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

use crate::{InventoryOwnerId, Timestamp};

pub const MAX_ORDER_KEY_LENGTH: usize = 200;
pub const MAX_ORDER_LINE_KEY_LENGTH: usize = 200;
pub const MAX_REQUESTED_UOM_LENGTH: usize = 32;
pub const MAX_DESTINATION_ADDRESS_LINE_LENGTH: usize = 200;
pub const MAX_DESTINATION_CITY_LENGTH: usize = 100;
pub const MAX_DESTINATION_REGION_LENGTH: usize = 100;
pub const MAX_DESTINATION_POSTAL_CODE_LENGTH: usize = 32;
pub const MAX_DESTINATION_COUNTRY_LENGTH: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderCreationField {
    OrderKey,
    LineKey,
    RequestedUom,
    DestinationLine1,
    DestinationLine2,
    DestinationCity,
    DestinationRegion,
    DestinationPostalCode,
    DestinationCountry,
}

impl fmt::Display for OrderCreationField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::OrderKey => "order key",
            Self::LineKey => "order line key",
            Self::RequestedUom => "requested UOM",
            Self::DestinationLine1 => "destination address line 1",
            Self::DestinationLine2 => "destination address line 2",
            Self::DestinationCity => "destination city",
            Self::DestinationRegion => "destination region",
            Self::DestinationPostalCode => "destination postal code",
            Self::DestinationCountry => "destination country",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OrderCreationError {
    #[error("{field} must be trimmed and nonblank")]
    InvalidText { field: OrderCreationField },
    #[error("{field} cannot exceed {maximum} characters")]
    TextTooLong {
        field: OrderCreationField,
        maximum: usize,
    },
    #[error("item ID must be a positive integer, got {value}")]
    InvalidItemId { value: i64 },
    #[error("order quantity must be a positive integer, got {value}")]
    InvalidQuantity { value: i64 },
    #[error("a fulfillment order requires at least one demand line")]
    MissingDemandLines,
    #[error("order line key must be unique within the order: {line_key}")]
    DuplicateLineKey { line_key: String },
}

fn validate_required_text(
    value: &str,
    field: OrderCreationField,
    maximum: usize,
) -> Result<(), OrderCreationError> {
    if value.is_empty() || value.trim() != value {
        return Err(OrderCreationError::InvalidText { field });
    }
    if value.chars().count() > maximum {
        return Err(OrderCreationError::TextTooLong { field, maximum });
    }
    Ok(())
}

fn validate_optional_text(
    value: Option<&str>,
    field: OrderCreationField,
    maximum: usize,
) -> Result<(), OrderCreationError> {
    if let Some(value) = value {
        validate_required_text(value, field, maximum)?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OrderKey(String);

impl OrderKey {
    pub fn new(value: impl Into<String>) -> Result<Self, OrderCreationError> {
        let value = value.into();
        validate_required_text(&value, OrderCreationField::OrderKey, MAX_ORDER_KEY_LENGTH)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OrderKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OrderLineKey(String);

impl OrderLineKey {
    pub fn new(value: impl Into<String>) -> Result<Self, OrderCreationError> {
        let value = value.into();
        validate_required_text(
            &value,
            OrderCreationField::LineKey,
            MAX_ORDER_LINE_KEY_LENGTH,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OrderLineKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestedUom(String);

impl RequestedUom {
    pub fn new(value: impl Into<String>) -> Result<Self, OrderCreationError> {
        let value = value.into();
        validate_required_text(
            &value,
            OrderCreationField::RequestedUom,
            MAX_REQUESTED_UOM_LENGTH,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RequestedUom {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderQuantity(i64);

impl OrderQuantity {
    pub const fn new(value: i64) -> Result<Self, OrderCreationError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(OrderCreationError::InvalidQuantity { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogItemId(i64);

impl CatalogItemId {
    pub const fn new(value: i64) -> Result<Self, OrderCreationError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(OrderCreationError::InvalidItemId { value })
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippingDestination {
    line1: String,
    line2: Option<String>,
    city: String,
    region: String,
    postal_code: String,
    country: String,
}

impl ShippingDestination {
    pub fn new(
        line1: impl Into<String>,
        line2: Option<String>,
        city: impl Into<String>,
        region: impl Into<String>,
        postal_code: impl Into<String>,
        country: impl Into<String>,
    ) -> Result<Self, OrderCreationError> {
        let destination = Self {
            line1: line1.into(),
            line2,
            city: city.into(),
            region: region.into(),
            postal_code: postal_code.into(),
            country: country.into(),
        };
        validate_required_text(
            &destination.line1,
            OrderCreationField::DestinationLine1,
            MAX_DESTINATION_ADDRESS_LINE_LENGTH,
        )?;
        validate_optional_text(
            destination.line2.as_deref(),
            OrderCreationField::DestinationLine2,
            MAX_DESTINATION_ADDRESS_LINE_LENGTH,
        )?;
        validate_required_text(
            &destination.city,
            OrderCreationField::DestinationCity,
            MAX_DESTINATION_CITY_LENGTH,
        )?;
        validate_required_text(
            &destination.region,
            OrderCreationField::DestinationRegion,
            MAX_DESTINATION_REGION_LENGTH,
        )?;
        validate_required_text(
            &destination.postal_code,
            OrderCreationField::DestinationPostalCode,
            MAX_DESTINATION_POSTAL_CODE_LENGTH,
        )?;
        validate_required_text(
            &destination.country,
            OrderCreationField::DestinationCountry,
            MAX_DESTINATION_COUNTRY_LENGTH,
        )?;
        Ok(destination)
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

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn postal_code(&self) -> &str {
        &self.postal_code
    }

    pub fn country(&self) -> &str {
        &self.country
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FulfillmentOrderDemandLine {
    line_key: OrderLineKey,
    item_id: CatalogItemId,
    quantity: OrderQuantity,
    requested_uom: RequestedUom,
}

impl FulfillmentOrderDemandLine {
    pub const fn new(
        line_key: OrderLineKey,
        item_id: CatalogItemId,
        quantity: OrderQuantity,
        requested_uom: RequestedUom,
    ) -> Self {
        Self {
            line_key,
            item_id,
            quantity,
            requested_uom,
        }
    }

    pub const fn line_key(&self) -> &OrderLineKey {
        &self.line_key
    }

    pub const fn item_id(&self) -> CatalogItemId {
        self.item_id
    }

    pub const fn quantity(&self) -> OrderQuantity {
        self.quantity
    }

    pub const fn requested_uom(&self) -> &RequestedUom {
        &self.requested_uom
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFulfillmentOrder {
    inventory_owner_id: InventoryOwnerId,
    order_key: OrderKey,
    rush: bool,
    ship_by: Option<Timestamp>,
    destination: ShippingDestination,
    demand_lines: Vec<FulfillmentOrderDemandLine>,
}

impl NewFulfillmentOrder {
    pub fn new(
        inventory_owner_id: InventoryOwnerId,
        order_key: OrderKey,
        rush: bool,
        ship_by: Option<Timestamp>,
        destination: ShippingDestination,
        demand_lines: Vec<FulfillmentOrderDemandLine>,
    ) -> Result<Self, OrderCreationError> {
        if demand_lines.is_empty() {
            return Err(OrderCreationError::MissingDemandLines);
        }

        let mut line_keys = HashSet::with_capacity(demand_lines.len());
        for line in &demand_lines {
            if !line_keys.insert(line.line_key().as_str()) {
                return Err(OrderCreationError::DuplicateLineKey {
                    line_key: line.line_key().as_str().to_owned(),
                });
            }
        }

        Ok(Self {
            inventory_owner_id,
            order_key,
            rush,
            ship_by,
            destination,
            demand_lines,
        })
    }

    pub const fn inventory_owner_id(&self) -> InventoryOwnerId {
        self.inventory_owner_id
    }

    pub const fn order_key(&self) -> &OrderKey {
        &self.order_key
    }

    pub const fn rush(&self) -> bool {
        self.rush
    }

    pub const fn ship_by(&self) -> Option<&Timestamp> {
        self.ship_by.as_ref()
    }

    pub const fn destination(&self) -> &ShippingDestination {
        &self.destination
    }

    pub fn demand_lines(&self) -> &[FulfillmentOrderDemandLine] {
        &self.demand_lines
    }

    pub const fn initial_status(&self) -> OrderStatus {
        OrderStatus::Open
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    AwaitingPacking,
    #[serde(rename = "awaiting shipment")]
    AwaitingShipment,
    Shipped,
    Cancelled,
    Held,
    Packing,
    Processing,
    #[default]
    Open,
    Void,
}

impl OrderStatus {
    pub const ALL: [Self; 9] = [
        Self::AwaitingPacking,
        Self::AwaitingShipment,
        Self::Shipped,
        Self::Cancelled,
        Self::Held,
        Self::Packing,
        Self::Processing,
        Self::Open,
        Self::Void,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingPacking => "awaiting packing",
            Self::AwaitingShipment => "awaiting shipment",
            Self::Shipped => "shipped",
            Self::Cancelled => "cancelled",
            Self::Held => "held",
            Self::Packing => "packing",
            Self::Processing => "processing",
            Self::Open => "open",
            Self::Void => "void",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "awaiting packing" => Some(Self::AwaitingPacking),
            "awaiting shipment" => Some(Self::AwaitingShipment),
            "shipped" => Some(Self::Shipped),
            "cancelled" => Some(Self::Cancelled),
            "held" => Some(Self::Held),
            "packing" => Some(Self::Packing),
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

    fn destination() -> ShippingDestination {
        ShippingDestination::new(
            "125 Shipping Lane",
            Some("Dock 4".into()),
            "Reno",
            "NV",
            "89502",
            "US",
        )
        .unwrap()
    }

    fn demand_line(line_key: &str, quantity: i64) -> FulfillmentOrderDemandLine {
        FulfillmentOrderDemandLine::new(
            OrderLineKey::new(line_key).unwrap(),
            CatalogItemId::new(41).unwrap(),
            OrderQuantity::new(quantity).unwrap(),
            RequestedUom::new("case").unwrap(),
        )
    }

    #[test]
    fn fulfillment_order_creation_builds_a_complete_open_aggregate() {
        let order = NewFulfillmentOrder::new(
            InventoryOwnerId::new(7).unwrap(),
            OrderKey::new("SO-1001").unwrap(),
            true,
            None,
            destination(),
            vec![demand_line("1", 12), demand_line("2", 3)],
        )
        .unwrap();

        assert_eq!(order.inventory_owner_id().get(), 7);
        assert_eq!(order.order_key().as_str(), "SO-1001");
        assert!(order.rush());
        assert_eq!(order.ship_by(), None);
        assert_eq!(order.initial_status(), OrderStatus::Open);
        assert_eq!(order.destination().city(), "Reno");
        assert_eq!(order.demand_lines().len(), 2);
        assert_eq!(order.demand_lines()[0].quantity().get(), 12);
    }

    #[test]
    fn fulfillment_order_creation_rejects_missing_and_duplicate_demand_lines() {
        let missing = NewFulfillmentOrder::new(
            InventoryOwnerId::new(7).unwrap(),
            OrderKey::new("SO-1001").unwrap(),
            false,
            None,
            destination(),
            Vec::new(),
        );
        assert_eq!(missing, Err(OrderCreationError::MissingDemandLines));

        let duplicate = NewFulfillmentOrder::new(
            InventoryOwnerId::new(7).unwrap(),
            OrderKey::new("SO-1001").unwrap(),
            false,
            None,
            destination(),
            vec![demand_line("LINE-1", 2), demand_line("LINE-1", 3)],
        );
        assert_eq!(
            duplicate,
            Err(OrderCreationError::DuplicateLineKey {
                line_key: "LINE-1".into()
            })
        );
    }

    #[test]
    fn order_entry_values_reject_invalid_text_and_quantities() {
        assert!(matches!(
            OrderKey::new(" SO-1001"),
            Err(OrderCreationError::InvalidText {
                field: OrderCreationField::OrderKey
            })
        ));
        assert!(matches!(
            OrderLineKey::new("x".repeat(MAX_ORDER_LINE_KEY_LENGTH + 1)),
            Err(OrderCreationError::TextTooLong {
                field: OrderCreationField::LineKey,
                maximum: MAX_ORDER_LINE_KEY_LENGTH
            })
        ));
        assert_eq!(
            RequestedUom::new(""),
            Err(OrderCreationError::InvalidText {
                field: OrderCreationField::RequestedUom
            })
        );
        assert_eq!(
            CatalogItemId::new(0),
            Err(OrderCreationError::InvalidItemId { value: 0 })
        );
        assert_eq!(
            OrderQuantity::new(-1),
            Err(OrderCreationError::InvalidQuantity { value: -1 })
        );
        assert!(matches!(
            ShippingDestination::new("", None, "Reno", "NV", "89502", "US"),
            Err(OrderCreationError::InvalidText {
                field: OrderCreationField::DestinationLine1
            })
        ));
        assert!(matches!(
            ShippingDestination::new(
                "125 Shipping Lane",
                None,
                "Reno",
                "NV",
                "9".repeat(MAX_DESTINATION_POSTAL_CODE_LENGTH + 1),
                "US"
            ),
            Err(OrderCreationError::TextTooLong {
                field: OrderCreationField::DestinationPostalCode,
                maximum: MAX_DESTINATION_POSTAL_CODE_LENGTH
            })
        ));
    }

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
