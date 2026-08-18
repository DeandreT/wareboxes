use axum::http::{Method, StatusCode};
use sqlx::Row;
use tower::ServiceExt;
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{
    ChangeDataCellStatusRequest, CreateTenantRequest, DataCellEventPage, DataCellMode,
    DataCellPage, DataCellResponse, DataCellStatus, ReconfigureDataCellRequest,
    TenantLifecycleResponse,
};

use super::support::{grant_platform_administrator, request, response};
use crate::common::*;

#[tokio::test]
async fn registry_provisioning_capacity_audit_and_rls_are_enforced() {
    let fixture = Fixture::new().await;
    let platform_admin = fixture.user("cell-platform-admin@test.local").await;
    let home = tenant_for_user(&fixture.db, platform_admin.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    let tenant_admin = fixture.user("cell-tenant-admin@test.local").await;
    let ordinary = fixture.user("cell-ordinary@test.local").await;
    let ordinary_home = tenant_for_user(&fixture.db, ordinary.id).await;
    let token = auth::create_session(&fixture.db, platform_admin.id)
        .await
        .unwrap();
    let ordinary_token = auth::create_session(&fixture.db, ordinary.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let forbidden = app
        .clone()
        .oneshot(request(
            &ordinary_token,
            ordinary_home,
            Method::GET,
            "/api/v1/platform/data-cells",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let invalid_dedicated = app
        .clone()
        .oneshot(request(
            &token,
            home,
            Method::POST,
            "/api/v1/platform/data-cells",
            Some("reject-invalid-dedicated-capacity"),
            &wareboxes_api_contract::v1::RegisterDataCellRequest {
                key: "invalid-dedicated-capacity".into(),
                name: "Invalid dedicated capacity".into(),
                region: "us-west-2".into(),
                residency: "US".into(),
                mode: DataCellMode::Dedicated,
                max_tenants: 2,
            },
        ))
        .await
        .unwrap();
    assert_eq!(invalid_dedicated.status(), StatusCode::BAD_REQUEST);

    let initial: DataCellPage = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::GET,
                "/api/v1/platform/data-cells?limit=20",
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert!(initial.items.iter().any(|cell| {
        cell.key == "local-default"
            && cell.status == DataCellStatus::Active
            && cell.residency == "GLOBAL"
    }));

    let registration = wareboxes_api_contract::v1::RegisterDataCellRequest {
        key: "us-west-primary".into(),
        name: "US West primary".into(),
        region: "us-west-2".into(),
        residency: "US".into(),
        mode: DataCellMode::Shared,
        max_tenants: 2,
    };
    let registered: DataCellResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                "/api/v1/platform/data-cells",
                Some("register-us-west-primary"),
                &registration,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(registered.status, DataCellStatus::Provisioning);
    assert_eq!(registered.revision.get(), 1);

    let replay: DataCellResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                "/api/v1/platform/data-cells",
                Some("register-us-west-primary"),
                &registration,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, registered);
    let mut changed = registration.clone();
    changed.name = "Conflicting cell".into();
    let conflict = app
        .clone()
        .oneshot(request(
            &token,
            home,
            Method::POST,
            "/api/v1/platform/data-cells",
            Some("register-us-west-primary"),
            &changed,
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let active: DataCellResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/data-cells/{}/status-changes",
                    registered.data_cell_id
                ),
                Some("activate-us-west-primary"),
                &ChangeDataCellStatusRequest {
                    expected_revision: registered.revision,
                    status: DataCellStatus::Active,
                    reason: "regional readiness checks passed".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(active.revision.get(), 2);

    let configured: DataCellResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/data-cells/{}/reconfigurations",
                    active.data_cell_id
                ),
                Some("expand-us-west-primary"),
                &ReconfigureDataCellRequest {
                    expected_revision: active.revision,
                    name: "US West primary production".into(),
                    max_tenants: 3,
                    reason: "measured capacity envelope increased".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(configured.revision.get(), 3);
    assert_eq!(configured.max_tenants, 3);

    let created: TenantLifecycleResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                "/api/v1/platform/tenants",
                Some("create-us-resident-tenant"),
                &CreateTenantRequest {
                    slug: "us-resident-tenant".into(),
                    name: "US resident tenant".into(),
                    administrator_email: tenant_admin.email.clone(),
                    data_cell_id: configured.data_cell_id,
                    residency_requirement: "US".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(created.data_cell_id, configured.data_cell_id);
    assert_eq!(created.data_cell_region, "us-west-2");
    assert_eq!(created.data_cell_residency, "US");
    assert_eq!(created.residency_requirement, "US");
    assert_eq!(created.placement_revision.get(), 1);

    let events: DataCellEventPage = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::GET,
                &format!(
                    "/api/v1/platform/data-cells/{}/events?limit=20",
                    configured.data_cell_id
                ),
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(events.items.len(), 3);
    assert_eq!(events.items[0].action, "reconfigured");
    assert_eq!(events.items[1].action, "activated");
    assert_eq!(events.items[2].action, "registered");

    let draining: DataCellResponse = response(
        app.clone()
            .oneshot(request(
                &token,
                home,
                Method::POST,
                &format!(
                    "/api/v1/platform/data-cells/{}/status-changes",
                    configured.data_cell_id
                ),
                Some("drain-us-west-primary"),
                &ChangeDataCellStatusRequest {
                    expected_revision: configured.revision,
                    status: DataCellStatus::Draining,
                    reason: "prepare the cell for a controlled tenant move".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let blocked = app
        .clone()
        .oneshot(request(
            &token,
            home,
            Method::POST,
            "/api/v1/platform/tenants",
            Some("blocked-draining-placement"),
            &CreateTenantRequest {
                slug: "blocked-draining-placement".into(),
                name: "Blocked draining placement".into(),
                administrator_email: ordinary.email.clone(),
                data_cell_id: draining.data_cell_id,
                residency_requirement: "US".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::CONFLICT);
    let retire = app
        .clone()
        .oneshot(request(
            &token,
            home,
            Method::POST,
            &format!(
                "/api/v1/platform/data-cells/{}/status-changes",
                draining.data_cell_id
            ),
            Some("reject-retire-with-placement"),
            &ChangeDataCellStatusRequest {
                expected_revision: draining.revision,
                status: DataCellStatus::Retired,
                reason: "must be empty before retirement".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(retire.status(), StatusCode::CONFLICT);

    let invisible = sqlx::query(
        r#"SELECT
          (SELECT COUNT(*) FROM data_cells) cells,
          (SELECT COUNT(*) FROM data_cell_events) cell_events,
          (SELECT COUNT(*) FROM tenant_cell_placements) placements,
          (SELECT COUNT(*) FROM tenant_cell_placement_events) placement_events"#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(invisible.get::<i64, _>("cells"), 0);
    assert_eq!(invisible.get::<i64, _>("cell_events"), 0);
    assert_eq!(invisible.get::<i64, _>("placements"), 0);
    assert_eq!(invisible.get::<i64, _>("placement_events"), 0);
    let direct_insert = sqlx::query(
        r#"INSERT INTO data_cells
        (cell_key,name,region,residency_code,mode,status,revision,max_tenants,
         created_at,created_by_user_id)
        VALUES('rls-bypass','RLS bypass','local','GLOBAL','shared','active',1,1,
               CURRENT_TIMESTAMP,$1)"#,
    )
    .bind(ordinary.id)
    .execute(&fixture.db)
    .await;
    assert!(direct_insert.is_err());
    let admin_db = admin_db_for(&fixture.db).await;
    let row = sqlx::query(
        r#"SELECT
          (SELECT COUNT(*) FROM tenant_cell_placements WHERE tenant_id=$1) placements,
          (SELECT COUNT(*) FROM tenant_cell_placement_events WHERE tenant_id=$1) placement_events,
          (SELECT COUNT(*) FROM outbox_events WHERE tenant_id=$2
             AND aggregate_type='data_cell') cell_outbox"#,
    )
    .bind(created.tenant_id)
    .bind(home.get())
    .fetch_one(&admin_db)
    .await
    .unwrap();
    assert_eq!(row.get::<i64, _>("placements"), 1);
    assert_eq!(row.get::<i64, _>("placement_events"), 1);
    assert_eq!(row.get::<i64, _>("cell_outbox"), 4);
    admin_db.close().await;
}
