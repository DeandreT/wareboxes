use serde::{Deserialize, Serialize};

use super::{
    DockBarcode, ExceptionNote, Expiration, ItemBarcode, LicensePlateBarcode, PositiveQuantity,
    StockDimension,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationMode {
    Received,
    Quarantined,
    Unexpected,
    Rejected,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerCapture {
    Loose,
    LicensePlate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptExceptionReason {
    Damaged,
    QualityRejected,
    ShortShipment,
    CountDiscrepancy,
    WrongItem,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptQuarantineReason {
    Damaged,
    QualityInspection,
    CountDiscrepancy,
    WrongItem,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnexpectedReceiptReason {
    Excess,
    UnexpectedItem,
    BlindReceipt,
    MisShipped,
    Other,
}

impl ReceiptQuarantineReason {
    pub(super) fn from_exception(reason: ReceiptExceptionReason) -> Option<Self> {
        match reason {
            ReceiptExceptionReason::Damaged => Some(Self::Damaged),
            ReceiptExceptionReason::QualityRejected => Some(Self::QualityInspection),
            ReceiptExceptionReason::CountDiscrepancy => Some(Self::CountDiscrepancy),
            ReceiptExceptionReason::WrongItem => Some(Self::WrongItem),
            ReceiptExceptionReason::Other => Some(Self::Other),
            ReceiptExceptionReason::ShortShipment => None,
        }
    }

    pub(super) fn as_exception(self) -> ReceiptExceptionReason {
        match self {
            Self::Damaged => ReceiptExceptionReason::Damaged,
            Self::QualityInspection => ReceiptExceptionReason::QualityRejected,
            Self::CountDiscrepancy => ReceiptExceptionReason::CountDiscrepancy,
            Self::WrongItem => ReceiptExceptionReason::WrongItem,
            Self::Other => ReceiptExceptionReason::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpectedReceiptCommand {
    Received {
        item_barcode: ItemBarcode,
        receiving_location_barcode: DockBarcode,
        quantity: PositiveQuantity,
        license_plate_barcode: Option<LicensePlateBarcode>,
        lot: Option<StockDimension>,
        serial: Option<StockDimension>,
        expiration: Option<Expiration>,
    },
    Quarantined {
        item_barcode: ItemBarcode,
        receiving_location_barcode: DockBarcode,
        quantity: PositiveQuantity,
        license_plate_barcode: Option<LicensePlateBarcode>,
        lot: Option<StockDimension>,
        serial: Option<StockDimension>,
        expiration: Option<Expiration>,
        reason: ReceiptQuarantineReason,
        note: Option<ExceptionNote>,
    },
    Rejected {
        item_barcode: ItemBarcode,
        quantity: PositiveQuantity,
        reason: ReceiptExceptionReason,
        note: Option<ExceptionNote>,
    },
    Missing {
        quantity: PositiveQuantity,
        reason: ReceiptExceptionReason,
        note: Option<ExceptionNote>,
    },
}

impl ExpectedReceiptCommand {
    #[must_use]
    pub const fn disposition(&self) -> ConfirmationMode {
        match self {
            Self::Received { .. } => ConfirmationMode::Received,
            Self::Quarantined { .. } => ConfirmationMode::Quarantined,
            Self::Rejected { .. } => ConfirmationMode::Rejected,
            Self::Missing { .. } => ConfirmationMode::Missing,
        }
    }

    #[must_use]
    pub const fn quantity(&self) -> PositiveQuantity {
        match self {
            Self::Received { quantity, .. }
            | Self::Quarantined { quantity, .. }
            | Self::Rejected { quantity, .. }
            | Self::Missing { quantity, .. } => *quantity,
        }
    }
}
