use axum::http::HeaderValue;

#[derive(Debug, Clone)]
pub struct SecurityConfig {
    pub allow_public_registration: bool,
    pub cors_allowed_origins: Vec<HeaderValue>,
    pub max_request_body_bytes: usize,
    pub web_session_absolute_ttl_seconds: i32,
    pub web_session_idle_ttl_seconds: i32,
    pub secure_web_session_cookie: bool,
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
        }
    }
}
