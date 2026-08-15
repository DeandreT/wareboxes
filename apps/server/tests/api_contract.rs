mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use tower::ServiceExt;
use wareboxes_api::request_context::REQUEST_ID_HEADER;
use wareboxes_api::routes;
use wareboxes_api::state::AppState;
use wareboxes_api_contract::v1::{ErrorReason as V1ErrorReason, ErrorResponse as V1ErrorResponse};
use wareboxes_api_contract::web::{ErrorCode, ErrorResponse};

async fn error_body(response: axum::response::Response) -> ErrorResponse {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn response_text(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn v1_error_body(response: axum::response::Response) -> V1ErrorResponse {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn health_endpoints_distinguish_liveness_from_database_readiness() {
    let db = common::setup().await;
    let app = routes::app(AppState::new(db.clone()));

    let live = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(live.status(), StatusCode::OK);
    assert!(response_text(live).await.contains("\"status\":\"ok\""));

    let ready = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    assert!(response_text(ready).await.contains("\"status\":\"ready\""));

    let metrics = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metrics.status(), StatusCode::OK);
    assert!(metrics
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/plain"));
    let metrics = response_text(metrics).await;
    assert!(metrics.contains("wareboxes_http_requests_total{status_class=\"2xx\"} 2"));
    assert!(metrics.contains("wareboxes_readiness_checks_total{result=\"ready\"} 1"));
    assert!(metrics.contains("wareboxes_database_pool_connections{state=\"open\"}"));
}

#[tokio::test]
async fn readiness_fails_closed_when_the_database_is_unavailable() {
    let db = common::setup().await;
    let app = routes::app(AppState::new(db.clone()));
    db.close().await;

    let live = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(live.status(), StatusCode::OK);

    for path in ["/health", "/health/ready"] {
        let ready = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response_text(ready)
            .await
            .contains("\"status\":\"unready\""));
    }
}

#[tokio::test]
async fn traffic_limits_fail_with_the_versioned_error_contract_and_spare_health() {
    let db = common::setup().await;
    let security = wareboxes_api::config::SecurityConfig {
        request_rate_limit_per_second: 2,
        ..wareboxes_api::config::SecurityConfig::default()
    };
    let app = routes::app(AppState::with_security(db, security));

    for request_number in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/not-a-route")
                    .header(REQUEST_ID_HEADER, format!("rate-{request_number}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            v1_error_body(response).await.reason,
            V1ErrorReason::NotFound
        );
    }

    let limited = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/not-a-route")
                .header(REQUEST_ID_HEADER, "rate-limited")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers().get(header::RETRY_AFTER).unwrap(), "1");
    let limited = v1_error_body(limited).await;
    assert_eq!(limited.reason, V1ErrorReason::RateLimited);
    assert_eq!(limited.request_id, "rate-limited");

    let health = app
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
}

#[tokio::test]
async fn responses_expose_correlated_request_ids_and_stable_errors() {
    let db = common::setup().await;
    let app = routes::app(AppState::new(db));

    let success = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/health")
                .header(REQUEST_ID_HEADER, "client-42.trace_1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(success.status(), StatusCode::OK);
    assert_eq!(
        success.headers().get(REQUEST_ID_HEADER).unwrap(),
        "client-42.trace_1"
    );

    let validation = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .header(REQUEST_ID_HEADER, "validation-1")
                .body(Body::from(r#"{"email":"bad","password":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(validation.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        validation.headers().get(REQUEST_ID_HEADER).unwrap(),
        "validation-1"
    );
    let validation_body = error_body(validation).await;
    assert_eq!(validation_body.code, ErrorCode::ValidationFailed);
    assert_eq!(validation_body.message, "validation failed");
    assert_eq!(validation_body.request_id, "validation-1");
    assert!(validation_body
        .details
        .iter()
        .any(|detail| detail.field == "email"));
    assert!(validation_body
        .details
        .iter()
        .any(|detail| detail.field == "password"));

    let malformed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .header(REQUEST_ID_HEADER, "malformed-1")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(malformed.status().is_client_error());
    assert_eq!(
        malformed.headers().get(REQUEST_ID_HEADER).unwrap(),
        "malformed-1"
    );
    let malformed_body = error_body(malformed).await;
    assert_eq!(malformed_body.code, ErrorCode::InvalidRequest);
    assert_eq!(malformed_body.request_id, "malformed-1");

    let missing = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/not-a-route")
                .header(REQUEST_ID_HEADER, "not valid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let generated = missing
        .headers()
        .get(REQUEST_ID_HEADER)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(generated.starts_with("req_"));
    let missing_body = error_body(missing).await;
    assert_eq!(missing_body.code, ErrorCode::NotFound);
    assert_eq!(missing_body.request_id, generated);
}
