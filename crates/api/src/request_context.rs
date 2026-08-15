use axum::body::Body;
use axum::extract::FromRequestParts;
use axum::extract::Request;
use axum::http::request::Parts;
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use rand::distributions::Alphanumeric;
use rand::Rng;
use wareboxes_api_contract::v1::{ErrorReason as V1ErrorReason, ErrorResponse as V1ErrorResponse};
use wareboxes_api_contract::web::{ErrorCode as WebErrorCode, ErrorResponse as WebErrorResponse};
use wareboxes_application::idempotency::IdempotencyKey as ApplicationIdempotencyKey;

use crate::error::AppError;

pub const REQUEST_ID_HEADER: &str = "x-request-id";
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

#[derive(Debug, Clone)]
pub struct IdempotencyKey(ApplicationIdempotencyKey);

impl IdempotencyKey {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<S> FromRequestParts<S> for IdempotencyKey
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let value = parts
            .headers
            .get(IDEMPOTENCY_KEY_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .ok_or_else(AppError::idempotency_key_required)?;
        Ok(Self(ApplicationIdempotencyKey::new(value)?))
    }
}

tokio::task_local! {
    static REQUEST_ID: String;
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

pub fn new_request_id() -> String {
    let suffix: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(24)
        .map(char::from)
        .collect();
    format!("req_{suffix}")
}

pub fn current_request_id() -> Option<String> {
    REQUEST_ID.try_with(Clone::clone).ok()
}

pub fn current_request_id_or_new() -> String {
    current_request_id().unwrap_or_else(new_request_id)
}

fn framework_error(status: StatusCode) -> (WebErrorCode, &'static str) {
    match status {
        StatusCode::UNAUTHORIZED => (WebErrorCode::Unauthorized, "unauthorized"),
        StatusCode::FORBIDDEN => (WebErrorCode::Forbidden, "forbidden"),
        StatusCode::NOT_FOUND => (WebErrorCode::NotFound, "not found"),
        StatusCode::METHOD_NOT_ALLOWED => (WebErrorCode::MethodNotAllowed, "method not allowed"),
        StatusCode::CONFLICT => (WebErrorCode::Conflict, "conflict"),
        StatusCode::PAYLOAD_TOO_LARGE => (WebErrorCode::PayloadTooLarge, "payload too large"),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => {
            (WebErrorCode::UnsupportedMediaType, "unsupported media type")
        }
        StatusCode::UNPROCESSABLE_ENTITY => (WebErrorCode::ValidationFailed, "validation failed"),
        StatusCode::TOO_MANY_REQUESTS => (WebErrorCode::RateLimited, "rate limit exceeded"),
        status if status.is_server_error() => (WebErrorCode::InternalError, "internal error"),
        _ => (WebErrorCode::InvalidRequest, "invalid request"),
    }
}

fn v1_framework_error(status: StatusCode) -> (V1ErrorReason, &'static str) {
    match status {
        StatusCode::UNAUTHORIZED => (V1ErrorReason::Unauthorized, "unauthorized"),
        StatusCode::FORBIDDEN => (V1ErrorReason::Forbidden, "forbidden"),
        StatusCode::NOT_FOUND => (V1ErrorReason::NotFound, "not found"),
        StatusCode::METHOD_NOT_ALLOWED => (V1ErrorReason::MethodNotAllowed, "method not allowed"),
        StatusCode::CONFLICT => (V1ErrorReason::Conflict, "conflict"),
        StatusCode::PAYLOAD_TOO_LARGE => (V1ErrorReason::PayloadTooLarge, "payload too large"),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => (
            V1ErrorReason::UnsupportedMediaType,
            "unsupported media type",
        ),
        StatusCode::UNPROCESSABLE_ENTITY => (V1ErrorReason::ValidationFailed, "validation failed"),
        StatusCode::TOO_MANY_REQUESTS => (V1ErrorReason::RateLimited, "rate limit exceeded"),
        status if status.is_server_error() => (V1ErrorReason::InternalError, "internal error"),
        _ => (V1ErrorReason::InvalidRequest, "invalid request"),
    }
}

fn ensure_error_contract(response: &mut Response, request_id: &str, is_v1: bool) {
    if !response.status().is_client_error() && !response.status().is_server_error() {
        return;
    }
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if is_json {
        return;
    }

    let body = if is_v1 {
        let (reason, message) = v1_framework_error(response.status());
        serde_json::to_vec(&V1ErrorResponse::new(reason, message, request_id))
    } else {
        let (code, message) = framework_error(response.status());
        serde_json::to_vec(&WebErrorResponse {
            code,
            message: message.into(),
            request_id: request_id.into(),
            details: Vec::new(),
        })
    };
    if let Ok(body) = body {
        *response.body_mut() = Body::from(body);
        response.headers_mut().remove(header::CONTENT_LENGTH);
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
}

fn is_v1_path(path: &str) -> bool {
    path == wareboxes_api_contract::v1::API_PREFIX
        || path
            .strip_prefix(wareboxes_api_contract::v1::API_PREFIX)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub async fn assign_request_id(mut request: Request, next: Next) -> Response {
    let is_v1 = is_v1_path(request.uri().path());
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| valid_request_id(value))
        .map(str::to_owned)
        .unwrap_or_else(new_request_id);
    let header_name = HeaderName::from_static(REQUEST_ID_HEADER);
    if let Ok(header_value) = HeaderValue::from_str(&request_id) {
        request
            .headers_mut()
            .insert(header_name.clone(), header_value);
    }

    REQUEST_ID
        .scope(request_id.clone(), async move {
            let mut response = next.run(request).await;
            ensure_error_contract(&mut response, &request_id, is_v1);
            if let Ok(header_value) = HeaderValue::from_str(&request_id) {
                response.headers_mut().insert(header_name, header_value);
            }
            response
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::{is_v1_path, valid_request_id};

    #[test]
    fn request_ids_allow_log_safe_correlation_characters() {
        assert!(valid_request_id("client-42.trace_1"));
        assert!(!valid_request_id(""));
        assert!(!valid_request_id("contains spaces"));
        assert!(!valid_request_id(&"a".repeat(129)));
    }

    #[test]
    fn versioned_contract_detection_requires_a_path_boundary() {
        assert!(is_v1_path("/api/v1"));
        assert!(is_v1_path("/api/v1/orders"));
        assert!(!is_v1_path("/api/v10/orders"));
        assert!(!is_v1_path("/api/orders"));
    }
}
