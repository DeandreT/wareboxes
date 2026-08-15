mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{
    ChangeTenantStatusRequest, CreateServiceAccountRequest, CreateTenantRequest,
    IssueServiceAccountCredentialRequest, IssuedServiceAccountCredentialResponse, Revision,
    ServiceAccountAccessRequest, ServiceAccountResponse, TenantLifecycleEventPage,
    TenantLifecyclePage, TenantLifecycleResponse, TenantStatus,
};

fn request<T: Serialize>(
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

async fn response<T: serde::de::DeserializeOwned>(
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

async fn grant_platform_administrator(db: &db::Db, user_id: i64) {
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

async fn grant_permission(fixture: &Fixture, tenant_id: TenantId, user_id: i64, name: &str) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        name,
        Some(name),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("tenant-lifecycle-{name}-{user_id}"),
        Some("Tenant lifecycle acceptance role"),
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(&fixture.db, tenant_id, user_id, role)
        .await
        .unwrap();
}

#[tokio::test]
async fn platform_tenant_lifecycle_is_atomic_scoped_and_revokes_access() {
    let fixture = Fixture::new().await;
    let platform_admin = fixture.user("platform-admin@test.local").await;
    let platform_home = tenant_for_user(&fixture.db, platform_admin.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    assert!(
        wareboxes_api::repo::tenant_lifecycle::is_platform_administrator(
            &fixture.db,
            platform_admin.id
        )
        .await
        .unwrap()
    );
    let tenant_admin = fixture.user("new-tenant-admin@test.local").await;
    let ordinary_user = fixture.user("ordinary-user@test.local").await;
    let ordinary_home = tenant_for_user(&fixture.db, ordinary_user.id).await;
    let platform_token = auth::create_session(&fixture.db, platform_admin.id)
        .await
        .unwrap();
    let ordinary_token = auth::create_session(&fixture.db, ordinary_user.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let forbidden = app
        .clone()
        .oneshot(request(
            &ordinary_token,
            ordinary_home,
            Method::GET,
            "/api/v1/platform/tenants",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    let _: TenantLifecyclePage = response(
        app.clone()
            .oneshot(request(
                &platform_token,
                platform_home,
                Method::GET,
                "/api/v1/platform/tenants?limit=20",
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let self_suspend = app
        .clone()
        .oneshot(request(
            &platform_token,
            platform_home,
            Method::POST,
            &format!(
                "/api/v1/platform/tenants/{}/status-changes",
                platform_home.get()
            ),
            Some("reject-current-tenant-suspension"),
            &ChangeTenantStatusRequest {
                expected_revision: Revision::new(1).unwrap(),
                status: TenantStatus::Suspended,
                reason: "must not lock out the platform operator".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(self_suspend.status(), StatusCode::BAD_REQUEST);

    let create = CreateTenantRequest {
        slug: "northwest-3pl".into(),
        name: "Northwest 3PL".into(),
        administrator_email: tenant_admin.email.clone(),
    };
    let created: TenantLifecycleResponse = response(
        app.clone()
            .oneshot(request(
                &platform_token,
                platform_home,
                Method::POST,
                "/api/v1/platform/tenants",
                Some("create-northwest"),
                &create,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(created.status, TenantStatus::Active);
    assert_eq!(created.revision.get(), 1);
    assert_eq!(created.initial_admin_user_id, Some(tenant_admin.id));
    assert_eq!(created.active_member_count, 1);
    let target_tenant = TenantId::new(created.tenant_id).unwrap();

    let replay: TenantLifecycleResponse = response(
        app.clone()
            .oneshot(request(
                &platform_token,
                platform_home,
                Method::POST,
                "/api/v1/platform/tenants",
                Some("create-northwest"),
                &create,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, created);
    let mut changed_create = create.clone();
    changed_create.name = "Different tenant".into();
    let conflict = app
        .clone()
        .oneshot(request(
            &platform_token,
            platform_home,
            Method::POST,
            "/api/v1/platform/tenants",
            Some("create-northwest"),
            &changed_create,
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let tenant_admin_access = tenant_accesses_for_user(&fixture.db, tenant_admin.id).await;
    assert!(tenant_admin_access.iter().any(|access| {
        access.tenant_id == target_tenant
            && access.site_scope.all_facilities
            && access.owner_scope.all_inventory_owners
    }));
    assert!(wareboxes_api::permissions::user_has_permission(
        &fixture.db,
        target_tenant,
        tenant_admin.id,
        "admin"
    )
    .await
    .unwrap());

    grant_permission(&fixture, target_tenant, tenant_admin.id, "orders").await;
    let facility_id = fixture.facility(target_tenant, "Northwest DC").await;
    let owner_id = fixture
        .inventory_owner(target_tenant, "Northwest client")
        .await;
    fixture
        .assign_owner_to_facility(target_tenant, owner_id, facility_id)
        .await;
    let tenant_admin_token = auth::create_session(&fixture.db, tenant_admin.id)
        .await
        .unwrap();
    let service_account: ServiceAccountResponse = response(
        app.clone()
            .oneshot(request(
                &tenant_admin_token,
                target_tenant,
                Method::POST,
                "/api/v1/service-accounts",
                Some("create-target-service-account"),
                &CreateServiceAccountRequest {
                    name: "Target ERP".into(),
                    description: None,
                    access: ServiceAccountAccessRequest {
                        all_facilities: false,
                        facility_ids: vec![facility_id],
                        all_inventory_owners: false,
                        inventory_owner_ids: vec![owner_id],
                        permission_names: vec!["orders".into()],
                    },
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let service_token = format!("wbs_sa_{}", "A".repeat(48));
    let _: IssuedServiceAccountCredentialResponse = response(
        app.clone()
            .oneshot(request(
                &tenant_admin_token,
                target_tenant,
                Method::POST,
                &format!(
                    "/api/v1/service-accounts/{}/credentials",
                    service_account.service_account_id
                ),
                Some("issue-target-credential"),
                &IssueServiceAccountCredentialRequest {
                    expected_revision: service_account.revision,
                    label: "primary".into(),
                    expires_at: None,
                    bearer_token: service_token.clone(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let service_context = app
        .clone()
        .oneshot(request(
            &service_token,
            target_tenant,
            Method::GET,
            "/api/auth/context",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(service_context.status(), StatusCode::OK);

    let suspend = ChangeTenantStatusRequest {
        expected_revision: Revision::new(1).unwrap(),
        status: TenantStatus::Suspended,
        reason: "customer contract paused".into(),
    };
    let suspended: TenantLifecycleResponse = response(
        app.clone()
            .oneshot(request(
                &platform_token,
                platform_home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenants/{}/status-changes",
                    target_tenant.get()
                ),
                Some("suspend-northwest"),
                &suspend,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(suspended.status, TenantStatus::Suspended);
    assert_eq!(suspended.revision.get(), 2);
    let target_session = app
        .clone()
        .oneshot(request(
            &tenant_admin_token,
            target_tenant,
            Method::GET,
            "/api/auth/context",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(target_session.status(), StatusCode::UNAUTHORIZED);
    let revoked_service = app
        .clone()
        .oneshot(request(
            &service_token,
            target_tenant,
            Method::GET,
            "/api/auth/context",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(revoked_service.status(), StatusCode::UNAUTHORIZED);

    let reactivate = ChangeTenantStatusRequest {
        expected_revision: Revision::new(2).unwrap(),
        status: TenantStatus::Active,
        reason: "customer contract restored".into(),
    };
    let active: TenantLifecycleResponse = response(
        app.clone()
            .oneshot(request(
                &platform_token,
                platform_home,
                Method::POST,
                &format!(
                    "/api/v1/platform/tenants/{}/status-changes",
                    target_tenant.get()
                ),
                Some("reactivate-northwest"),
                &reactivate,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(active.status, TenantStatus::Active);
    assert_eq!(active.revision.get(), 3);
    assert!(auth::create_session(&fixture.db, tenant_admin.id)
        .await
        .is_ok());
    let still_revoked = app
        .clone()
        .oneshot(request(
            &service_token,
            target_tenant,
            Method::GET,
            "/api/auth/context",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(still_revoked.status(), StatusCode::UNAUTHORIZED);

    let page: TenantLifecyclePage = response(
        app.clone()
            .oneshot(request(
                &platform_token,
                platform_home,
                Method::GET,
                "/api/v1/platform/tenants?status=active&search=northwest&limit=20",
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].tenant_id, target_tenant.get());
    let events: TenantLifecycleEventPage = response(
        app.clone()
            .oneshot(request(
                &platform_token,
                platform_home,
                Method::GET,
                &format!(
                    "/api/v1/platform/tenants/{}/events?limit=20",
                    target_tenant.get()
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
    let suspension = events
        .items
        .iter()
        .find(|event| event.action == "suspended")
        .unwrap();
    assert!(suspension.revoked_session_count >= 1);
    assert_eq!(suspension.revoked_credential_count, 1);

    let unbound_lifecycle_events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tenant_lifecycle_events")
            .fetch_one(&fixture.db)
            .await
            .unwrap();
    assert_eq!(unbound_lifecycle_events, 0);
    let outbox_count: i64 = {
        let mut tx = tenant_tx(&fixture.db, target_tenant).await;
        let count = sqlx::query_scalar(
            "SELECT COUNT(*) FROM outbox_events WHERE tenant_id=$1 AND aggregate_type='tenant'",
        )
        .bind(target_tenant.get())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        count
    };
    assert_eq!(outbox_count, 3);

    let raw_mutation = sqlx::query("UPDATE tenants SET status='suspended' WHERE id=$1")
        .bind(target_tenant.get())
        .execute(&fixture.db)
        .await
        .unwrap_err();
    assert_eq!(
        raw_mutation.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );

    let concurrent_suspend = ChangeTenantStatusRequest {
        expected_revision: Revision::new(3).unwrap(),
        status: TenantStatus::Suspended,
        reason: "concurrent platform suspension".into(),
    };
    let first = app.clone().oneshot(request(
        &platform_token,
        platform_home,
        Method::POST,
        &format!(
            "/api/v1/platform/tenants/{}/status-changes",
            target_tenant.get()
        ),
        Some("concurrent-suspend-a"),
        &concurrent_suspend,
    ));
    let second = app.clone().oneshot(request(
        &platform_token,
        platform_home,
        Method::POST,
        &format!(
            "/api/v1/platform/tenants/{}/status-changes",
            target_tenant.get()
        ),
        Some("concurrent-suspend-b"),
        &concurrent_suspend,
    ));
    let (first, second) = tokio::join!(first, second);
    let mut statuses = vec![first.unwrap().status(), second.unwrap().status()];
    statuses.sort_by_key(|status| status.as_u16());
    assert_eq!(statuses, vec![StatusCode::OK, StatusCode::CONFLICT]);

    let lifecycle_event_count: i64 = {
        let admin_db = admin_db_for(&fixture.db).await;
        let count =
            sqlx::query_scalar("SELECT COUNT(*) FROM tenant_lifecycle_events WHERE tenant_id=$1")
                .bind(target_tenant.get())
                .fetch_one(&admin_db)
                .await
                .unwrap();
        admin_db.close().await;
        count
    };
    assert_eq!(lifecycle_event_count, 4);
    let final_outbox_count: i64 = {
        let mut tx = tenant_tx(&fixture.db, target_tenant).await;
        let count = sqlx::query_scalar(
            "SELECT COUNT(*) FROM outbox_events WHERE tenant_id=$1 AND aggregate_type='tenant'",
        )
        .bind(target_tenant.get())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        count
    };
    assert_eq!(final_outbox_count, 4);
}
