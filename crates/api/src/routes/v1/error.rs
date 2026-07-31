use axum::body::to_bytes;
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use wareboxes_api_contract::v1::{ErrorReason, ErrorResponse, FieldViolation};
use wareboxes_api_contract::web::{ErrorCode as WebErrorCode, ErrorResponse as WebErrorResponse};
use wareboxes_core::CoreError;

use crate::error::AppError;
use crate::request_context::{current_request_id_or_new, REQUEST_ID_HEADER};

pub type V1Result<T> = Result<T, V1Error>;

#[derive(Debug)]
pub struct V1Error {
    status: StatusCode,
    reason: ErrorReason,
    message: String,
    violations: Vec<FieldViolation>,
}

impl V1Error {
    pub fn invalid_cursor() -> Self {
        Self::invalid_cursor_for("inventory balance")
    }

    pub fn invalid_cursor_for(resource: &str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            reason: ErrorReason::InvalidCursor,
            message: format!("invalid {resource} cursor"),
            violations: Vec::new(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            reason: ErrorReason::InternalError,
            message: message.into(),
            violations: Vec::new(),
        }
    }
}

impl From<AppError> for V1Error {
    fn from(error: AppError) -> Self {
        match error {
            AppError::Core(core) => Self::from(core),
            AppError::Db(error) => Self::internal(format!("database error: {error}")),
            AppError::Other(error) => Self::internal(error.to_string()),
        }
    }
}

impl From<CoreError> for V1Error {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::Unauthorized => Self::simple(
                StatusCode::UNAUTHORIZED,
                ErrorReason::Unauthorized,
                "unauthorized",
            ),
            CoreError::Forbidden => {
                Self::simple(StatusCode::FORBIDDEN, ErrorReason::Forbidden, "forbidden")
            }
            CoreError::NotFound(resource) => Self::simple(
                StatusCode::NOT_FOUND,
                ErrorReason::NotFound,
                format!("not found: {resource}"),
            ),
            CoreError::Validation(details) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                reason: ErrorReason::ValidationFailed,
                message: "validation failed".into(),
                violations: details
                    .into_iter()
                    .map(|detail| FieldViolation {
                        field: detail.field,
                        reason: detail.message,
                    })
                    .collect(),
            },
            CoreError::Conflict(message) => {
                Self::simple(StatusCode::CONFLICT, ErrorReason::Conflict, message)
            }
            CoreError::IdempotencyKeyReused => Self::simple(
                StatusCode::CONFLICT,
                ErrorReason::IdempotencyKeyReused,
                "idempotency key was already used with a different request",
            ),
            CoreError::IdempotencyKeyRequired => Self::simple(
                StatusCode::BAD_REQUEST,
                ErrorReason::IdempotencyKeyRequired,
                "idempotency key is required",
            ),
            CoreError::BadRequest(message) => Self::simple(
                StatusCode::BAD_REQUEST,
                ErrorReason::InvalidRequest,
                message,
            ),
            CoreError::Internal(message) => Self::internal(message),
        }
    }
}

impl V1Error {
    fn simple(status: StatusCode, reason: ErrorReason, message: impl Into<String>) -> Self {
        Self {
            status,
            reason,
            message: message.into(),
            violations: Vec::new(),
        }
    }

    fn contract(&self, request_id: String) -> ErrorResponse {
        ErrorResponse::new(self.reason, public_message(self), request_id)
            .with_violations(self.violations.clone())
    }
}

impl IntoResponse for V1Error {
    fn into_response(self) -> Response {
        let request_id = current_request_id_or_new();
        if self.status.is_server_error() {
            tracing::error!(%request_id, error = %self.message, "v1 request failed");
        }
        let status = self.status;
        let mut response = (status, Json(self.contract(request_id.clone()))).into_response();
        if let Ok(value) = HeaderValue::from_str(&request_id) {
            response
                .headers_mut()
                .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
        }
        response
    }
}

fn public_message(error: &V1Error) -> String {
    if error.status.is_server_error() {
        "internal error".into()
    } else {
        error.message.clone()
    }
}

pub async fn normalize_error_response(response: Response) -> Response {
    if !response.status().is_client_error() && !response.status().is_server_error() {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let body = match to_bytes(body, 64 * 1024).await {
        Ok(body) => body,
        Err(error) => {
            tracing::error!(%error, "failed to read v1 error response");
            Default::default()
        }
    };
    let fallback_request_id = parts
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(current_request_id_or_new);
    let contract = if let Ok(contract) = serde_json::from_slice::<ErrorResponse>(&body) {
        contract
    } else if let Ok(web) = serde_json::from_slice::<WebErrorResponse>(&body) {
        web_contract(web)
    } else {
        let (reason, message) = reason_for_status(parts.status);
        ErrorResponse::new(reason, message, fallback_request_id)
    };

    let mut normalized = (parts.status, Json(contract)).into_response();
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    *normalized.headers_mut() = parts.headers;
    *normalized.extensions_mut() = parts.extensions;
    normalized
}

fn web_contract(web: WebErrorResponse) -> ErrorResponse {
    ErrorResponse::new(
        match web.code {
            WebErrorCode::Unauthorized => ErrorReason::Unauthorized,
            WebErrorCode::Forbidden => ErrorReason::Forbidden,
            WebErrorCode::NotFound => ErrorReason::NotFound,
            WebErrorCode::ValidationFailed => ErrorReason::ValidationFailed,
            WebErrorCode::Conflict => ErrorReason::Conflict,
            WebErrorCode::IdempotencyKeyReused => ErrorReason::IdempotencyKeyReused,
            WebErrorCode::IdempotencyKeyRequired => ErrorReason::IdempotencyKeyRequired,
            WebErrorCode::InvalidRequest => ErrorReason::InvalidRequest,
            WebErrorCode::MethodNotAllowed => ErrorReason::MethodNotAllowed,
            WebErrorCode::PayloadTooLarge => ErrorReason::PayloadTooLarge,
            WebErrorCode::UnsupportedMediaType => ErrorReason::UnsupportedMediaType,
            WebErrorCode::RateLimited => ErrorReason::RateLimited,
            WebErrorCode::InternalError => ErrorReason::InternalError,
        },
        web.message,
        web.request_id,
    )
    .with_violations(
        web.details
            .into_iter()
            .map(|detail| FieldViolation {
                field: detail.field,
                reason: detail.message,
            })
            .collect(),
    )
}

fn reason_for_status(status: StatusCode) -> (ErrorReason, &'static str) {
    match status {
        StatusCode::UNAUTHORIZED => (ErrorReason::Unauthorized, "unauthorized"),
        StatusCode::FORBIDDEN => (ErrorReason::Forbidden, "forbidden"),
        StatusCode::NOT_FOUND => (ErrorReason::NotFound, "not found"),
        StatusCode::METHOD_NOT_ALLOWED => (ErrorReason::MethodNotAllowed, "method not allowed"),
        StatusCode::CONFLICT => (ErrorReason::Conflict, "conflict"),
        StatusCode::PAYLOAD_TOO_LARGE => (ErrorReason::PayloadTooLarge, "payload too large"),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => {
            (ErrorReason::UnsupportedMediaType, "unsupported media type")
        }
        StatusCode::UNPROCESSABLE_ENTITY => (ErrorReason::ValidationFailed, "validation failed"),
        StatusCode::TOO_MANY_REQUESTS => (ErrorReason::RateLimited, "rate limit exceeded"),
        status if status.is_server_error() => (ErrorReason::InternalError, "internal error"),
        _ => (ErrorReason::InvalidRequest, "invalid request"),
    }
}
