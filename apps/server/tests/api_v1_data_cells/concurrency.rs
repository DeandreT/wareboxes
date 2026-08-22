use axum::http::{Method, StatusCode};
use tower::ServiceExt;
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{CreateTenantRequest, DataCellMode};

use super::support::{
    grant_platform_administrator, register_and_activate, request, ActiveDataCell,
};
use crate::common::*;

#[tokio::test]
async fn dedicated_capacity_and_residency_are_serialized() {
    let fixture = Fixture::new().await;
    let platform_admin = fixture.user("dedicated-platform-admin@test.local").await;
    let home = tenant_for_user(&fixture.db, platform_admin.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    let first_admin = fixture.user("dedicated-first-admin@test.local").await;
    let second_admin = fixture.user("dedicated-second-admin@test.local").await;
    let token = auth::create_session(&fixture.db, platform_admin.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let cell = register_and_activate(
        &app,
        &token,
        home,
        ActiveDataCell {
            key: "eu-dedicated-a",
            region: "eu-central-1",
            residency: "EU",
            mode: DataCellMode::Dedicated,
            capacity: 1,
        },
    )
    .await;

    let mismatch = app
        .clone()
        .oneshot(request(
            &token,
            home,
            Method::POST,
            "/api/v1/platform/tenants",
            Some("reject-us-in-eu-cell"),
            &CreateTenantRequest {
                slug: "reject-us-in-eu-cell".into(),
                name: "Reject US in EU cell".into(),
                administrator_email: first_admin.email.clone(),
                data_cell_id: cell.data_cell_id,
                residency_requirement: "US".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(mismatch.status(), StatusCode::BAD_REQUEST);

    let first = app.clone().oneshot(request(
        &token,
        home,
        Method::POST,
        "/api/v1/platform/tenants",
        Some("dedicated-first-tenant"),
        &CreateTenantRequest {
            slug: "dedicated-first-tenant".into(),
            name: "Dedicated first tenant".into(),
            administrator_email: first_admin.email.clone(),
            data_cell_id: cell.data_cell_id,
            residency_requirement: "EU".into(),
        },
    ));
    let second = app.clone().oneshot(request(
        &token,
        home,
        Method::POST,
        "/api/v1/platform/tenants",
        Some("dedicated-second-tenant"),
        &CreateTenantRequest {
            slug: "dedicated-second-tenant".into(),
            name: "Dedicated second tenant".into(),
            administrator_email: second_admin.email.clone(),
            data_cell_id: cell.data_cell_id,
            residency_requirement: "EU".into(),
        },
    ));
    let (first, second) = tokio::join!(first, second);
    let statuses = [first.unwrap().status(), second.unwrap().status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );

    let admin_db = admin_db_for(&fixture.db).await;
    let placements: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tenant_cell_placements WHERE data_cell_id=$1")
            .bind(cell.data_cell_id)
            .fetch_one(&admin_db)
            .await
            .unwrap();
    assert_eq!(placements, 1);
    admin_db.close().await;
}
