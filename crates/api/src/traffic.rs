use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;

use crate::config::SecurityConfig;

pub struct TrafficGate {
    in_flight: Arc<Semaphore>,
    request_rate: FixedWindow,
    login_rate: FixedWindow,
    timeout: Duration,
}

struct FixedWindow {
    limit: usize,
    duration: Duration,
    state: Mutex<WindowState>,
}

struct WindowState {
    started_at: Instant,
    accepted: usize,
}

impl FixedWindow {
    fn new(limit: usize, duration: Duration) -> Self {
        Self {
            limit,
            duration,
            state: Mutex::new(WindowState {
                started_at: Instant::now(),
                accepted: 0,
            }),
        }
    }

    fn allow(&self, now: Instant) -> bool {
        let mut state = recover_lock(&self.state);
        if now.saturating_duration_since(state.started_at) >= self.duration {
            state.started_at = now;
            state.accepted = 0;
        }
        if state.accepted >= self.limit {
            return false;
        }
        state.accepted += 1;
        true
    }
}

fn recover_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl TrafficGate {
    pub fn new(config: &SecurityConfig) -> Self {
        Self {
            in_flight: Arc::new(Semaphore::new(config.max_in_flight_requests)),
            request_rate: FixedWindow::new(
                config.request_rate_limit_per_second,
                Duration::from_secs(1),
            ),
            login_rate: FixedWindow::new(
                config.login_rate_limit_per_minute,
                Duration::from_secs(60),
            ),
            timeout: config.request_timeout,
        }
    }
}

fn is_service_endpoint(path: &str) -> bool {
    matches!(
        path,
        "/health" | "/health/live" | "/health/ready" | "/metrics"
    )
}

fn is_login_endpoint(path: &str) -> bool {
    matches!(path, "/api/auth/login" | "/api/web/auth/login")
}

fn rejection(status: StatusCode) -> Response {
    let mut response = status.into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

pub async fn enforce(
    State(gate): State<Arc<TrafficGate>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if is_service_endpoint(path) {
        return next.run(request).await;
    }

    let now = Instant::now();
    if !gate.request_rate.allow(now) || (is_login_endpoint(path) && !gate.login_rate.allow(now)) {
        return rejection(StatusCode::TOO_MANY_REQUESTS);
    }
    let Ok(permit) = gate.in_flight.clone().try_acquire_owned() else {
        return rejection(StatusCode::SERVICE_UNAVAILABLE);
    };
    let response = tokio::time::timeout(gate.timeout, next.run(request)).await;
    drop(permit);
    match response {
        Ok(response) => response,
        Err(_) => rejection(StatusCode::GATEWAY_TIMEOUT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    #[test]
    fn fixed_window_rejects_excess_and_resets_after_its_duration() {
        let start = Instant::now();
        let rate = FixedWindow::new(2, Duration::from_secs(1));
        assert!(rate.allow(start));
        assert!(rate.allow(start));
        assert!(!rate.allow(start));
        assert!(rate.allow(start + Duration::from_secs(2)));
    }

    #[test]
    fn service_endpoints_are_not_suppressed_by_traffic_load() {
        for path in ["/health", "/health/live", "/health/ready", "/metrics"] {
            assert!(is_service_endpoint(path));
        }
        assert!(!is_service_endpoint("/api/orders"));
        assert!(is_login_endpoint("/api/auth/login"));
        assert!(is_login_endpoint("/api/web/auth/login"));
        assert!(!is_login_endpoint("/api/auth/me"));
    }

    #[tokio::test]
    async fn slow_application_requests_are_cancelled_at_the_deadline() {
        let config = SecurityConfig {
            request_timeout: Duration::from_millis(1),
            ..SecurityConfig::default()
        };
        let app = Router::new()
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    "late"
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                Arc::new(TrafficGate::new(&config)),
                enforce,
            ));

        let response = app
            .oneshot(Request::get("/slow").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    }
}
