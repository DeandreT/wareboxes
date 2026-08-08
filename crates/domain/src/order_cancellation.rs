use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

/// Maximum operator note length, measured in Unicode scalar values.
pub const MAX_CANCELLATION_NOTE_LENGTH: usize = 1_000;

/// Durable business reason for cancelling a fulfillment order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderCancellationReason {
    ClientRequest,
    DuplicateOrder,
    DataCorrection,
    InventoryUnavailable,
    FulfillmentException,
    Other,
}

impl OrderCancellationReason {
    pub const ALL: [Self; 6] = [
        Self::ClientRequest,
        Self::DuplicateOrder,
        Self::DataCorrection,
        Self::InventoryUnavailable,
        Self::FulfillmentException,
        Self::Other,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClientRequest => "client_request",
            Self::DuplicateOrder => "duplicate_order",
            Self::DataCorrection => "data_correction",
            Self::InventoryUnavailable => "inventory_unavailable",
            Self::FulfillmentException => "fulfillment_exception",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "client_request" => Some(Self::ClientRequest),
            "duplicate_order" => Some(Self::DuplicateOrder),
            "data_correction" => Some(Self::DataCorrection),
            "inventory_unavailable" => Some(Self::InventoryUnavailable),
            "fulfillment_exception" => Some(Self::FulfillmentException),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    pub const fn requires_note(self) -> bool {
        matches!(self, Self::Other)
    }
}

impl fmt::Display for OrderCancellationReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Trimmed, nonblank operator context for an order cancellation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CancellationNote(String);

impl CancellationNote {
    pub fn new(value: impl Into<String>) -> Result<Self, OrderCancellationError> {
        let value = value.into();
        if value.is_empty() || value.trim() != value {
            return Err(OrderCancellationError::InvalidNote);
        }
        if value.chars().count() > MAX_CANCELLATION_NOTE_LENGTH {
            return Err(OrderCancellationError::NoteTooLong {
                maximum: MAX_CANCELLATION_NOTE_LENGTH,
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CancellationNote {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CancellationNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// A validated cancellation reason and its optional operator context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrderCancellationDetails {
    reason: OrderCancellationReason,
    note: Option<CancellationNote>,
}

impl OrderCancellationDetails {
    pub fn new(
        reason: OrderCancellationReason,
        note: Option<CancellationNote>,
    ) -> Result<Self, OrderCancellationError> {
        if reason.requires_note() && note.is_none() {
            return Err(OrderCancellationError::NoteRequired);
        }
        Ok(Self { reason, note })
    }

    pub const fn reason(&self) -> OrderCancellationReason {
        self.reason
    }

    pub fn note(&self) -> Option<&CancellationNote> {
        self.note.as_ref()
    }

    pub fn into_note(self) -> Option<CancellationNote> {
        self.note
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OrderCancellationError {
    #[error("cancellation note must be trimmed and nonblank")]
    InvalidNote,
    #[error("cancellation note cannot exceed {maximum} characters")]
    NoteTooLong { maximum: usize },
    #[error("cancellation reason other requires a note")]
    NoteRequired,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_reasons_round_trip_through_persistence_values() {
        for reason in OrderCancellationReason::ALL {
            assert_eq!(
                OrderCancellationReason::parse(reason.as_str()),
                Some(reason)
            );
        }
        assert_eq!(OrderCancellationReason::parse("customer_request"), None);
    }

    #[test]
    fn notes_are_trimmed_nonblank_and_character_bounded() {
        assert_eq!(
            CancellationNote::new("Client cancelled at 14:05").map(|note| note.as_str().to_owned()),
            Ok("Client cancelled at 14:05".into())
        );
        assert_eq!(
            CancellationNote::new(" cancellation"),
            Err(OrderCancellationError::InvalidNote)
        );
        assert_eq!(
            CancellationNote::new(" "),
            Err(OrderCancellationError::InvalidNote)
        );
        assert_eq!(
            CancellationNote::new("x".repeat(MAX_CANCELLATION_NOTE_LENGTH + 1)),
            Err(OrderCancellationError::NoteTooLong {
                maximum: MAX_CANCELLATION_NOTE_LENGTH
            })
        );

        let multibyte = "é".repeat(MAX_CANCELLATION_NOTE_LENGTH);
        assert!(CancellationNote::new(multibyte).is_ok());
    }

    #[test]
    fn other_requires_operator_context() {
        assert_eq!(
            OrderCancellationDetails::new(OrderCancellationReason::Other, None),
            Err(OrderCancellationError::NoteRequired)
        );
        assert!(OrderCancellationDetails::new(
            OrderCancellationReason::Other,
            Some(CancellationNote::new("Carrier rejected the load").unwrap())
        )
        .is_ok());
        assert!(
            OrderCancellationDetails::new(OrderCancellationReason::ClientRequest, None).is_ok()
        );
    }
}
