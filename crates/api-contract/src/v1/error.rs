use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Stable machine-readable reason for an API failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ErrorReason {
    Unauthorized,
    Forbidden,
    NotFound,
    ValidationFailed,
    InvalidRequest,
    Conflict,
    InvalidStateTransition,
    InsufficientInventory,
    IdempotencyKeyRequired,
    IdempotencyKeyReused,
    PreconditionRequired,
    RevisionConflict,
    InvalidCursor,
    MethodNotAllowed,
    PayloadTooLarge,
    UnsupportedMediaType,
    RateLimited,
    InternalError,
}

/// Machine-addressable validation failure for one request field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldViolation {
    /// Request field or logical input that failed validation.
    #[schema(example = "lines[0].quantity")]
    pub field: String,
    /// Stable validation explanation intended for an integration operator.
    #[schema(example = "must be a positive integer")]
    pub reason: String,
}

/// Error response returned by every version 1 endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    pub reason: ErrorReason,
    #[schema(example = "idempotency key was already used with a different request")]
    pub message: String,
    #[schema(
        min_length = 1,
        max_length = 128,
        pattern = "^[A-Za-z0-9._:-]{1,128}$",
        example = "partner-order-1001-attempt-1"
    )]
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub violations: Vec<FieldViolation>,
}

impl ErrorResponse {
    /// Creates an error response without field violations.
    pub fn new(
        reason: ErrorReason,
        message: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            reason,
            message: message.into(),
            request_id: request_id.into(),
            violations: Vec::new(),
        }
    }

    /// Adds field-level violations.
    pub fn with_violations(mut self, violations: Vec<FieldViolation>) -> Self {
        self.violations = violations;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_reasons_have_stable_snake_case_names() {
        assert_eq!(
            serde_json::to_string(&ErrorReason::RevisionConflict).unwrap(),
            r#""revision_conflict""#
        );
        assert_eq!(
            serde_json::from_str::<ErrorReason>(r#""idempotency_key_reused""#).unwrap(),
            ErrorReason::IdempotencyKeyReused
        );
    }

    #[test]
    fn error_response_omits_empty_violations() {
        let response =
            ErrorResponse::new(ErrorReason::NotFound, "resource not found", "request-42");

        assert_eq!(
            serde_json::to_string(&response).unwrap(),
            r#"{"reason":"not_found","message":"resource not found","request_id":"request-42"}"#
        );
    }

    #[test]
    fn error_response_round_trips_field_violations() {
        let response = ErrorResponse::new(
            ErrorReason::ValidationFailed,
            "validation failed",
            "request-43",
        )
        .with_violations(vec![FieldViolation {
            field: "quantity".into(),
            reason: "must_be_positive".into(),
        }]);

        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(
            serde_json::from_str::<ErrorResponse>(&json).unwrap(),
            response
        );
    }
}
