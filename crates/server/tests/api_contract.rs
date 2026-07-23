mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use tower::ServiceExt;
use wareboxes_core::dto::{ErrorCode, ErrorResponse};
use wareboxes_server::request_context::REQUEST_ID_HEADER;
use wareboxes_server::routes;
use wareboxes_server::state::AppState;

async fn error_body(response: axum::response::Response) -> ErrorResponse {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
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
