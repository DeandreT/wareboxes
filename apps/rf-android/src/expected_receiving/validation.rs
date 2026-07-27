use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ReceivingValidationError {
    #[error("identifier must be positive")]
    InvalidPositiveIdentifier,
    #[error("quantity must be positive")]
    InvalidPositiveQuantity,
    #[error("quantity cannot be negative")]
    InvalidNonNegativeQuantity,
    #[error("quantity exceeds the supported range")]
    QuantityOverflow,
    #[error("barcode must be trimmed, nonempty, and within its size limit")]
    InvalidBarcode,
    #[error("load barcode does not use the supported execution-code alphabet")]
    InvalidLoadBarcode,
    #[error("stock dimension must be trimmed, nonempty, and within its size limit")]
    InvalidStockDimension,
    #[error("expiration must be a valid RFC 3339 timestamp")]
    InvalidExpiration,
    #[error("exception note must be trimmed, nonempty, and within its size limit")]
    InvalidExceptionNote,
    #[error("expected receipt line requires an item barcode")]
    MissingItemBarcode,
    #[error("expected receipt line contains duplicate item barcodes")]
    DuplicateItemBarcode,
    #[error("expected receipt quantities do not reconcile")]
    InvalidLineQuantities,
    #[error("expected receiving session requires an open line")]
    MissingOpenLines,
    #[error("expected receiving session contains a closed line")]
    ClosedLineInSession,
    #[error("expected receiving session contains duplicate load lines")]
    DuplicateLoadLine,
    #[error("confirmation recovery snapshot requires an open selected line")]
    InvalidRecoveryLine,
    #[error("confirmation intent is inconsistent with its recovery snapshot")]
    InvalidConfirmationIntent,
}
