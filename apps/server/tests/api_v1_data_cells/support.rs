#![allow(dead_code)]

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api_contract::v1::{
    ChangeDataCellStatusRequest, DataCellMode, DataCellResponse, DataCellStatus,
    RegisterDataCellRequest, Revision,
};

use crate::common::{admin_db_for, db, TenantId};

pub fn request<T: Serialize>(
    token: &str,
    context_tenant_id: TenantId,
    method: Method,
    uri: &str,
    key: Option<&str>,
    body: &T,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, context_tenant_id.to_string());
    if let Some(key) = key {
        builder = builder
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .header(REQUEST_ID_HEADER, key)
            .header(header::CONTENT_TYPE, "application/json");
    }
    builder
        .body(if key.is_some() {
            Body::from(serde_json::to_vec(body).unwrap())
        } else {
            Body::empty()
        })
        .unwrap()
}

pub async fn response<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
    status: StatusCode,
) -> T {
    let actual = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    assert_eq!(
        actual,
        status,
        "unexpected response: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

pub async fn grant_platform_administrator(db: &db::Db, user_id: i64) {
    let admin_db = admin_db_for(db).await;
    sqlx::query(
        r#"INSERT INTO platform_administrators
        (user_id,revision,granted_at,granted_by_user_id)
        VALUES($1,1,CURRENT_TIMESTAMP,$1)"#,
    )
    .bind(user_id)
    .execute(&admin_db)
    .await
    .unwrap();
    admin_db.close().await;
}

pub struct ActiveDataCell<'a> {
    pub key: &'a str,
    pub region: &'a str,
    pub residency: &'a str,
    pub mode: DataCellMode,
    pub capacity: u32,
}

pub async fn register_and_activate(
    app: &axum::Router,
    token: &str,
    home: TenantId,
    cell: ActiveDataCell<'_>,
) -> DataCellResponse {
    let ActiveDataCell {
        key,
        region,
        residency,
        mode,
        capacity,
    } = cell;
    let registered: DataCellResponse = response(
        app.clone()
            .oneshot(request(
                token,
                home,
                Method::POST,
                "/api/v1/platform/data-cells",
                Some(&format!("register-{key}")),
                &RegisterDataCellRequest {
                    key: key.into(),
                    name: format!("Cell {key}"),
                    region: region.into(),
                    residency: residency.into(),
                    mode,
                    max_tenants: capacity,
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    response(
        app.clone()
            .oneshot(request(
                token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/data-cells/{}/status-changes",
                    registered.data_cell_id
                ),
                Some(&format!("activate-{key}")),
                &ChangeDataCellStatusRequest {
                    expected_revision: Revision::new(1).unwrap(),
                    status: DataCellStatus::Active,
                    reason: "production readiness checks passed".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await
}
