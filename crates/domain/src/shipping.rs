use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use crate::{CartonId, OrderStatus, PackSessionStatus};

pub const MAX_CARRIER_CODE_LENGTH: usize = 100;
pub const MAX_CARRIER_SERVICE_CODE_LENGTH: usize = 100;
pub const MAX_MANIFEST_REFERENCE_LENGTH: usize = 200;
pub const MAX_TRACKING_NUMBER_LENGTH: usize = 200;
pub const MAX_SHIPMENT_SCAN_VALUE_LENGTH: usize = 200;

/// Lifecycle of one full-order parcel shipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShipmentStatus {
    AwaitingManifest,
    Manifested,
    Departed,
}

impl ShipmentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingManifest => "awaiting manifest",
            Self::Manifested => "manifested",
            Self::Departed => "departed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "awaiting manifest" => Some(Self::AwaitingManifest),
            "manifested" => Some(Self::Manifested),
            "departed" => Some(Self::Departed),
            _ => None,
        }
    }
}

impl fmt::Display for ShipmentStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Positive revision used for optimistic shipment mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ShipmentRevision(i64);

impl ShipmentRevision {
    pub const fn new(value: i64) -> Result<Self, ShippingError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(ShippingError::InvalidRevision { value })
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

impl<'de> Deserialize<'de> for ShipmentRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShippingTextField {
    CarrierCode,
    CarrierServiceCode,
    ManifestReference,
    TrackingNumber,
    ShipmentScanValue,
}

impl fmt::Display for ShippingTextField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CarrierCode => "carrier code",
            Self::CarrierServiceCode => "carrier service code",
            Self::ManifestReference => "manifest reference",
            Self::TrackingNumber => "tracking number",
            Self::ShipmentScanValue => "shipment scan value",
        })
    }
}

fn validate_text(
    value: String,
    field: ShippingTextField,
    maximum: usize,
) -> Result<String, ShippingError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(ShippingError::InvalidText { field });
    }
    if value.chars().count() > maximum {
        return Err(ShippingError::TextTooLong { field, maximum });
    }
    Ok(value)
}

macro_rules! shipping_text_value {
    ($name:ident, $field:ident, $maximum:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ShippingError> {
                validate_text(value.into(), ShippingTextField::$field, $maximum).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = ShippingError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = ShippingError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(D::Error::custom)
            }
        }
    };
}

shipping_text_value!(CarrierCode, CarrierCode, MAX_CARRIER_CODE_LENGTH);
shipping_text_value!(
    CarrierServiceCode,
    CarrierServiceCode,
    MAX_CARRIER_SERVICE_CODE_LENGTH
);
shipping_text_value!(
    ManifestReference,
    ManifestReference,
    MAX_MANIFEST_REFERENCE_LENGTH
);
shipping_text_value!(TrackingNumber, TrackingNumber, MAX_TRACKING_NUMBER_LENGTH);
shipping_text_value!(
    ShipmentScanValue,
    ShipmentScanValue,
    MAX_SHIPMENT_SCAN_VALUE_LENGTH
);

/// Immutable identity needed to prove shipment carton set equality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShipmentCartonIdentity {
    carton_id: CartonId,
    carton_barcode: ShipmentScanValue,
}

impl ShipmentCartonIdentity {
    pub const fn new(carton_id: CartonId, carton_barcode: ShipmentScanValue) -> Self {
        Self {
            carton_id,
            carton_barcode,
        }
    }

    pub const fn carton_id(&self) -> CartonId {
        self.carton_id
    }

    pub const fn carton_barcode(&self) -> &ShipmentScanValue {
        &self.carton_barcode
    }
}

/// Carrier tracking identity assigned to exactly one shipment carton.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CartonTrackingAssignment {
    carton_id: CartonId,
    tracking_number: TrackingNumber,
}

impl CartonTrackingAssignment {
    pub const fn new(carton_id: CartonId, tracking_number: TrackingNumber) -> Self {
        Self {
            carton_id,
            tracking_number,
        }
    }

    pub const fn carton_id(&self) -> CartonId {
        self.carton_id
    }

    pub const fn tracking_number(&self) -> &TrackingNumber {
        &self.tracking_number
    }
}

/// Final aggregate transitions applied atomically when a shipment departs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipmentDepartureTransition {
    pub shipment_status: ShipmentStatus,
    pub order_status: OrderStatus,
}

/// Starts one shipment from the complete closed-carton set of a ready pack session.
pub fn create_shipment(
    order_status: OrderStatus,
    pack_session_status: PackSessionStatus,
    cartons: &[ShipmentCartonIdentity],
) -> Result<ShipmentStatus, ShippingError> {
    if !matches!(order_status, OrderStatus::AwaitingShipment) {
        return Err(ShippingError::OrderNotAwaitingShipment {
            status: order_status,
        });
    }
    if !matches!(pack_session_status, PackSessionStatus::ReadyToManifest) {
        return Err(ShippingError::PackSessionNotReady);
    }
    validate_carton_set(cartons)?;
    Ok(ShipmentStatus::AwaitingManifest)
}

/// Records one manual manifest only when every shipment carton has one tracking identity.
pub fn record_manual_manifest(
    status: ShipmentStatus,
    cartons: &[ShipmentCartonIdentity],
    assignments: &[CartonTrackingAssignment],
) -> Result<ShipmentStatus, ShippingError> {
    if !matches!(status, ShipmentStatus::AwaitingManifest) {
        return Err(ShippingError::ShipmentNotAwaitingManifest { status });
    }
    validate_carton_set(cartons)?;
    if assignments.len() != cartons.len() {
        return Err(ShippingError::TrackingAssignmentSetMismatch);
    }

    let carton_ids = cartons
        .iter()
        .map(ShipmentCartonIdentity::carton_id)
        .collect::<HashSet<_>>();
    let mut assigned_carton_ids = HashSet::with_capacity(assignments.len());
    let mut tracking_numbers = HashSet::with_capacity(assignments.len());
    for assignment in assignments {
        if !carton_ids.contains(&assignment.carton_id())
            || !assigned_carton_ids.insert(assignment.carton_id())
        {
            return Err(ShippingError::TrackingAssignmentSetMismatch);
        }
        if !tracking_numbers.insert(assignment.tracking_number().as_str()) {
            return Err(ShippingError::DuplicateTrackingNumber);
        }
    }

    if assigned_carton_ids != carton_ids {
        return Err(ShippingError::TrackingAssignmentSetMismatch);
    }
    Ok(ShipmentStatus::Manifested)
}

/// Confirms physical departure using a duplicate-free exact scan of the carton set.
pub fn confirm_shipment_departure(
    shipment_status: ShipmentStatus,
    order_status: OrderStatus,
    cartons: &[ShipmentCartonIdentity],
    scanned_carton_barcodes: &[ShipmentScanValue],
) -> Result<ShipmentDepartureTransition, ShippingError> {
    if !matches!(shipment_status, ShipmentStatus::Manifested) {
        return Err(ShippingError::ShipmentNotManifested {
            status: shipment_status,
        });
    }
    if !matches!(order_status, OrderStatus::AwaitingShipment) {
        return Err(ShippingError::OrderNotAwaitingShipment {
            status: order_status,
        });
    }
    validate_carton_set(cartons)?;
    if scanned_carton_barcodes.len() != cartons.len() {
        return Err(ShippingError::DepartureCartonSetMismatch);
    }

    let expected = cartons
        .iter()
        .map(|carton| carton.carton_barcode().as_str())
        .collect::<HashSet<_>>();
    let scanned = scanned_carton_barcodes
        .iter()
        .map(ShipmentScanValue::as_str)
        .collect::<HashSet<_>>();
    if scanned.len() != scanned_carton_barcodes.len() || scanned != expected {
        return Err(ShippingError::DepartureCartonSetMismatch);
    }

    Ok(ShipmentDepartureTransition {
        shipment_status: ShipmentStatus::Departed,
        order_status: OrderStatus::Shipped,
    })
}

fn validate_carton_set(cartons: &[ShipmentCartonIdentity]) -> Result<(), ShippingError> {
    if cartons.is_empty() {
        return Err(ShippingError::EmptyShipment);
    }
    let carton_ids = cartons
        .iter()
        .map(ShipmentCartonIdentity::carton_id)
        .collect::<HashSet<_>>();
    let carton_barcodes = cartons
        .iter()
        .map(|carton| carton.carton_barcode().as_str())
        .collect::<HashSet<_>>();
    if carton_ids.len() != cartons.len() || carton_barcodes.len() != cartons.len() {
        return Err(ShippingError::DuplicateShipmentCarton);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ShippingError {
    #[error("shipment revision must be a positive integer, got {value}")]
    InvalidRevision { value: i64 },
    #[error("{field} must be trimmed, nonblank, and free of control characters")]
    InvalidText { field: ShippingTextField },
    #[error("{field} cannot exceed {maximum} characters")]
    TextTooLong {
        field: ShippingTextField,
        maximum: usize,
    },
    #[error("only an awaiting-shipment order can create or depart a shipment, got {status}")]
    OrderNotAwaitingShipment { status: OrderStatus },
    #[error("packing session is not ready to manifest")]
    PackSessionNotReady,
    #[error("shipment must contain at least one carton")]
    EmptyShipment,
    #[error("shipment carton identities and barcodes must be unique")]
    DuplicateShipmentCarton,
    #[error("only an awaiting-manifest shipment can be manifested, got {status}")]
    ShipmentNotAwaitingManifest { status: ShipmentStatus },
    #[error("tracking assignments must identify every shipment carton exactly once")]
    TrackingAssignmentSetMismatch,
    #[error("tracking numbers must be unique within a shipment")]
    DuplicateTrackingNumber,
    #[error("only a manifested shipment can depart, got {status}")]
    ShipmentNotManifested { status: ShipmentStatus },
    #[error("departure scans must identify every shipment carton exactly once")]
    DepartureCartonSetMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carton(id: i64, barcode: &str) -> ShipmentCartonIdentity {
        ShipmentCartonIdentity::new(
            CartonId::new(id).unwrap(),
            ShipmentScanValue::new(barcode).unwrap(),
        )
    }

    fn assignment(id: i64, tracking: &str) -> CartonTrackingAssignment {
        CartonTrackingAssignment::new(
            CartonId::new(id).unwrap(),
            TrackingNumber::new(tracking).unwrap(),
        )
    }

    #[test]
    fn shipping_values_are_strict_and_revisioned() {
        assert_eq!(CarrierCode::new("UPS").unwrap().as_str(), "UPS");
        assert!(CarrierCode::new(" UPS").is_err());
        assert!(TrackingNumber::new("TRACK\n1").is_err());
        assert!(ManifestReference::new("x".repeat(MAX_MANIFEST_REFERENCE_LENGTH + 1)).is_err());
        let revision = ShipmentRevision::new(2).unwrap();
        assert_eq!(revision.checked_next().map(ShipmentRevision::get), Some(3));
        assert!(serde_json::from_str::<ShipmentRevision>("0").is_err());
    }

    #[test]
    fn shipment_starts_only_from_one_ready_nonempty_carton_set() {
        let cartons = [carton(1, "CARTON-1"), carton(2, "CARTON-2")];
        assert_eq!(
            create_shipment(
                OrderStatus::AwaitingShipment,
                PackSessionStatus::ReadyToManifest,
                &cartons
            ),
            Ok(ShipmentStatus::AwaitingManifest)
        );
        assert_eq!(
            create_shipment(
                OrderStatus::Packing,
                PackSessionStatus::ReadyToManifest,
                &cartons
            ),
            Err(ShippingError::OrderNotAwaitingShipment {
                status: OrderStatus::Packing
            })
        );
        assert_eq!(
            create_shipment(
                OrderStatus::AwaitingShipment,
                PackSessionStatus::Open,
                &cartons
            ),
            Err(ShippingError::PackSessionNotReady)
        );
        assert_eq!(
            create_shipment(
                OrderStatus::AwaitingShipment,
                PackSessionStatus::ReadyToManifest,
                &[]
            ),
            Err(ShippingError::EmptyShipment)
        );
    }

    #[test]
    fn manual_manifest_requires_exactly_one_unique_tracking_assignment_per_carton() {
        let cartons = [carton(1, "CARTON-1"), carton(2, "CARTON-2")];
        assert_eq!(
            record_manual_manifest(
                ShipmentStatus::AwaitingManifest,
                &cartons,
                &[assignment(2, "TRACK-2"), assignment(1, "TRACK-1")]
            ),
            Ok(ShipmentStatus::Manifested)
        );
        assert_eq!(
            record_manual_manifest(
                ShipmentStatus::AwaitingManifest,
                &cartons,
                &[assignment(1, "TRACK-1"), assignment(1, "TRACK-2")]
            ),
            Err(ShippingError::TrackingAssignmentSetMismatch)
        );
        assert_eq!(
            record_manual_manifest(
                ShipmentStatus::AwaitingManifest,
                &cartons,
                &[assignment(1, "TRACK-1"), assignment(2, "TRACK-1")]
            ),
            Err(ShippingError::DuplicateTrackingNumber)
        );
    }

    #[test]
    fn departure_requires_a_duplicate_free_exact_carton_scan_set() {
        let cartons = [carton(1, "CARTON-1"), carton(2, "CARTON-2")];
        let exact = [
            ShipmentScanValue::new("CARTON-2").unwrap(),
            ShipmentScanValue::new("CARTON-1").unwrap(),
        ];
        assert_eq!(
            confirm_shipment_departure(
                ShipmentStatus::Manifested,
                OrderStatus::AwaitingShipment,
                &cartons,
                &exact
            ),
            Ok(ShipmentDepartureTransition {
                shipment_status: ShipmentStatus::Departed,
                order_status: OrderStatus::Shipped,
            })
        );

        let duplicate = [
            ShipmentScanValue::new("CARTON-1").unwrap(),
            ShipmentScanValue::new("CARTON-1").unwrap(),
        ];
        assert_eq!(
            confirm_shipment_departure(
                ShipmentStatus::Manifested,
                OrderStatus::AwaitingShipment,
                &cartons,
                &duplicate
            ),
            Err(ShippingError::DepartureCartonSetMismatch)
        );
    }
}
