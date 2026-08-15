use std::time::Duration;

use axum::http::HeaderValue;

#[derive(Debug, Clone)]
pub struct SecurityConfig {
    pub allow_public_registration: bool,
    pub cors_allowed_origins: Vec<HeaderValue>,
    pub max_request_body_bytes: usize,
    pub web_session_absolute_ttl_seconds: i32,
    pub web_session_idle_ttl_seconds: i32,
    pub secure_web_session_cookie: bool,
    pub max_in_flight_requests: usize,
    pub request_rate_limit_per_second: usize,
    pub login_rate_limit_per_minute: usize,
    pub request_timeout: Duration,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            allow_public_registration: false,
            cors_allowed_origins: Vec::new(),
            max_request_body_bytes: 1024 * 1024,
            web_session_absolute_ttl_seconds: 12 * 60 * 60,
            web_session_idle_ttl_seconds: 30 * 60,
            secure_web_session_cookie: false,
            max_in_flight_requests: 256,
            request_rate_limit_per_second: 1_000,
            login_rate_limit_per_minute: 60,
            request_timeout: Duration::from_secs(30),
        }
    }
}
