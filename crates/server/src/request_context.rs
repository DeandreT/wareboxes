use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use rand::distributions::Alphanumeric;
use rand::Rng;
use wareboxes_core::dto::{ErrorCode, ErrorResponse};

pub const REQUEST_ID_HEADER: &str = "x-request-id";

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

fn framework_error(status: StatusCode) -> (ErrorCode, &'static str) {
    match status {
        StatusCode::UNAUTHORIZED => (ErrorCode::Unauthorized, "unauthorized"),
        StatusCode::FORBIDDEN => (ErrorCode::Forbidden, "forbidden"),
        StatusCode::NOT_FOUND => (ErrorCode::NotFound, "not found"),
        StatusCode::METHOD_NOT_ALLOWED => (ErrorCode::MethodNotAllowed, "method not allowed"),
        StatusCode::CONFLICT => (ErrorCode::Conflict, "conflict"),
        StatusCode::PAYLOAD_TOO_LARGE => (ErrorCode::PayloadTooLarge, "payload too large"),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => {
            (ErrorCode::UnsupportedMediaType, "unsupported media type")
        }
        StatusCode::UNPROCESSABLE_ENTITY => (ErrorCode::ValidationFailed, "validation failed"),
        StatusCode::TOO_MANY_REQUESTS => (ErrorCode::RateLimited, "rate limit exceeded"),
        status if status.is_server_error() => (ErrorCode::InternalError, "internal error"),
        _ => (ErrorCode::InvalidRequest, "invalid request"),
    }
}

fn ensure_error_contract(response: &mut Response, request_id: &str) {
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

    let (code, message) = framework_error(response.status());
    let contract = ErrorResponse {
        code,
        message: message.into(),
        request_id: request_id.into(),
        details: Vec::new(),
    };
    if let Ok(body) = serde_json::to_vec(&contract) {
        *response.body_mut() = Body::from(body);
        response.headers_mut().remove(header::CONTENT_LENGTH);
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
}

pub async fn assign_request_id(mut request: Request, next: Next) -> Response {
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
            ensure_error_contract(&mut response, &request_id);
            if let Ok(header_value) = HeaderValue::from_str(&request_id) {
                response.headers_mut().insert(header_name, header_value);
            }
            response
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::valid_request_id;

    #[test]
    fn request_ids_allow_log_safe_correlation_characters() {
        assert!(valid_request_id("client-42.trace_1"));
        assert!(!valid_request_id(""));
        assert!(!valid_request_id("contains spaces"));
        assert!(!valid_request_id(&"a".repeat(129)));
    }
}
