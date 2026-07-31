use serde::{Deserialize, Serialize};

/// Machine-readable error code returned by web operations endpoints.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    Forbidden,
    NotFound,
    ValidationFailed,
    Conflict,
    IdempotencyKeyReused,
    IdempotencyKeyRequired,
    InvalidRequest,
    MethodNotAllowed,
    PayloadTooLarge,
    UnsupportedMediaType,
    RateLimited,
    InternalError,
}

/// Field-level validation detail returned by web operations endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

/// Error response returned by web operations endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    pub code: ErrorCode,
    pub message: String,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<FieldError>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_have_stable_snake_case_names() {
        assert_eq!(
            serde_json::to_string(&ErrorCode::IdempotencyKeyReused).unwrap(),
            r#""idempotency_key_reused""#
        );
        assert_eq!(
            serde_json::from_str::<ErrorCode>(r#""validation_failed""#).unwrap(),
            ErrorCode::ValidationFailed
        );
    }

    #[test]
    fn error_response_omits_empty_details() {
        let response = ErrorResponse {
            code: ErrorCode::NotFound,
            message: "not found".into(),
            request_id: "request-42".into(),
            details: Vec::new(),
        };

        assert_eq!(
            serde_json::to_string(&response).unwrap(),
            r#"{"code":"not_found","message":"not found","request_id":"request-42"}"#
        );
    }

    #[test]
    fn error_response_round_trips_field_details() {
        let response = ErrorResponse {
            code: ErrorCode::ValidationFailed,
            message: "validation failed".into(),
            request_id: "request-43".into(),
            details: vec![FieldError {
                field: "quantity".into(),
                message: "must be positive".into(),
            }],
        };

        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(
            serde_json::from_str::<ErrorResponse>(&json).unwrap(),
            response
        );
    }

    #[test]
    fn error_response_rejects_unknown_fields() {
        assert!(serde_json::from_str::<ErrorResponse>(
            r#"{"code":"invalid_request","message":"invalid","request_id":"request-44","debug":"secret"}"#,
        )
        .is_err());
        assert!(serde_json::from_str::<ErrorResponse>(
            r#"{"code":"validation_failed","message":"invalid","request_id":"request-45","details":[{"field":"quantity","message":"invalid","path":"body"}]}"#,
        )
        .is_err());
    }
}
