mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use chrono::{Duration, Utc};
use common::*;
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{auth, permissions, repo, routes, state::AppState};
use wareboxes_api_contract::v1::{
    ApproveSupportAccessRequest, RequestSupportAccessRequest, Revision, RevokeSupportAccessRequest,
    SupportAccessEventPage, SupportAccessPage, SupportAccessPolicyRequest, SupportAccessResponse,
    SupportAccessStatus,
};
use wareboxes_core::dto::{SelectTenantRequest, WebSessionContext};

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

async fn decode<T: serde::de::DeserializeOwned>(
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

async fn grant_platform_administrator(db: &wareboxes_api::db::Db, user_id: i64) {
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

#[tokio::test]
async fn support_access_requires_two_people_and_fails_closed_after_revocation() {
    let fixture = Fixture::new().await;
    let requester = fixture.user("support-requester@test.local").await;
    let approver = fixture.user("support-approver@test.local").await;
    let competing_approver = fixture.user("support-competing-approver@test.local").await;
    let ordinary = fixture.user("support-ordinary@test.local").await;
    let target_admin = fixture.user("support-target-admin@test.local").await;
    let requester_home = tenant_for_user(&fixture.db, requester.id).await;
    let approver_home = tenant_for_user(&fixture.db, approver.id).await;
    let competing_home = tenant_for_user(&fixture.db, competing_approver.id).await;
    let ordinary_home = tenant_for_user(&fixture.db, ordinary.id).await;
    let target_tenant = tenant_for_user(&fixture.db, target_admin.id).await;
    grant_platform_administrator(&fixture.db, requester.id).await;
    grant_platform_administrator(&fixture.db, approver.id).await;
    grant_platform_administrator(&fixture.db, competing_approver.id).await;
    let requester_token = auth::create_session(&fixture.db, requester.id)
        .await
        .unwrap();
    let approver_token = auth::create_session(&fixture.db, approver.id)
        .await
        .unwrap();
    let competing_token = auth::create_session(&fixture.db, competing_approver.id)
        .await
        .unwrap();
    let ordinary_token = auth::create_session(&fixture.db, ordinary.id)
        .await
        .unwrap();
    let allowed_facility = fixture.facility(target_tenant, "Support allowed DC").await;
    let denied_facility = fixture.facility(target_tenant, "Support denied DC").await;
    let allowed_owner = fixture
        .inventory_owner(target_tenant, "Support allowed owner")
        .await;
    let denied_owner = fixture
        .inventory_owner(target_tenant, "Support denied owner")
        .await;
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        target_tenant,
        "wms",
        Some("WMS read and execute"),
    )
    .await
    .unwrap();
    assert!(permission > 0);
    let state = AppState::new(fixture.db.clone());
    let web_session_cookie_name = auth::web_session_cookie_name(&state.security);
    let security = state.security.clone();
    let app = routes::app(state);
    let ordinary_list = app
        .clone()
        .oneshot(request(
            &ordinary_token,
            ordinary_home,
            Method::GET,
            "/api/v1/platform/support-access",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(ordinary_list.status(), StatusCode::FORBIDDEN);

    let request_body = RequestSupportAccessRequest {
        tenant_id: target_tenant.get(),
        reason: "Investigate reconciliation incident INC-4242".into(),
        expires_at: (Utc::now() + Duration::hours(1)).to_rfc3339(),
        access: SupportAccessPolicyRequest {
            all_facilities: false,
            facility_ids: vec![allowed_facility],
            all_inventory_owners: false,
            inventory_owner_ids: vec![allowed_owner],
            permission_names: vec!["wms".into()],
        },
    };
    let requested: SupportAccessResponse = decode(
        app.clone()
            .oneshot(request(
                &requester_token,
                requester_home,
                Method::POST,
                "/api/v1/platform/support-access",
                Some("request-support-inc-4242"),
                &request_body,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(requested.status, SupportAccessStatus::Pending);
    assert_eq!(requested.revision.get(), 1);
    assert_eq!(requested.requested_by, requester.id);
    assert_eq!(requested.access.facility_ids, vec![allowed_facility]);
    assert_eq!(requested.access.inventory_owner_ids, vec![allowed_owner]);

    let replay: SupportAccessResponse = decode(
        app.clone()
            .oneshot(request(
                &requester_token,
                requester_home,
                Method::POST,
                "/api/v1/platform/support-access",
                Some("request-support-inc-4242"),
                &request_body,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, requested);
    assert!(
        repo::tenants::access_for_user(&fixture.db, requester.id, target_tenant)
            .await
            .unwrap()
            .is_none()
    );

    let self_approval = app
        .clone()
        .oneshot(request(
            &requester_token,
            requester_home,
            Method::POST,
            &format!(
                "/api/v1/platform/support-access/{}/approvals",
                requested.support_access_grant_id
            ),
            Some("self-approve-support-inc-4242"),
            &ApproveSupportAccessRequest {
                expected_revision: Revision::new(1).unwrap(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(self_approval.status(), StatusCode::FORBIDDEN);

    let approval_path = format!(
        "/api/v1/platform/support-access/{}/approvals",
        requested.support_access_grant_id
    );
    let approval_body = ApproveSupportAccessRequest {
        expected_revision: Revision::new(1).unwrap(),
    };
    let (first, second) = tokio::join!(
        app.clone().oneshot(request(
            &approver_token,
            approver_home,
            Method::POST,
            &approval_path,
            Some("approve-support-inc-4242-a"),
            &approval_body,
        )),
        app.clone().oneshot(request(
            &competing_token,
            competing_home,
            Method::POST,
            &approval_path,
            Some("approve-support-inc-4242-b"),
            &approval_body,
        )),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    let statuses = [first.status(), second.status()];
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
    let approved_response = if first.status() == StatusCode::OK {
        first
    } else {
        second
    };
    let approved: SupportAccessResponse = decode(approved_response, StatusCode::OK).await;
    assert_eq!(approved.status, SupportAccessStatus::Active);
    assert_eq!(approved.revision.get(), 2);
    assert_ne!(approved.approved_by, Some(requester.id));

    let support_access = repo::tenants::access_for_user(&fixture.db, requester.id, target_tenant)
        .await
        .unwrap()
        .expect("approved access is available");
    assert!(!support_access.site_scope.all_facilities);
    assert!(support_access
        .site_scope
        .includes(wareboxes_domain::FacilityId::new(allowed_facility).unwrap()));
    assert!(!support_access
        .site_scope
        .includes(wareboxes_domain::FacilityId::new(denied_facility).unwrap()));
    assert!(support_access
        .owner_scope
        .includes(wareboxes_domain::InventoryOwnerId::new(allowed_owner).unwrap()));
    assert!(!support_access
        .owner_scope
        .includes(wareboxes_domain::InventoryOwnerId::new(denied_owner).unwrap()));
    assert!(
        permissions::user_has_permission(&fixture.db, target_tenant, requester.id, "wms")
            .await
            .unwrap()
    );
    assert!(
        !permissions::user_has_permission(&fixture.db, target_tenant, requester.id, "admin")
            .await
            .unwrap()
    );

    let requester_web_token = auth::create_web_session(&fixture.db, requester.id, 3_600)
        .await
        .unwrap();
    let browser_context: WebSessionContext = decode(
        app.clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/web/auth/tenant")
                    .header(header::HOST, "support.test")
                    .header(header::ORIGIN, "http://support.test")
                    .header(
                        header::COOKIE,
                        format!("{web_session_cookie_name}={requester_web_token}"),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&SelectTenantRequest {
                            tenant_id: target_tenant.get(),
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        browser_context.active_support_access_id,
        Some(requested.support_access_grant_id)
    );

    let balance_page = app
        .clone()
        .oneshot(request(
            &requester_token,
            target_tenant,
            Method::GET,
            "/api/v1/inventory/balances?limit=20",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(balance_page.status(), StatusCode::OK);

    let support_write = app
        .clone()
        .oneshot(request(
            &requester_token,
            target_tenant,
            Method::POST,
            &format!(
                "/api/v1/platform/support-access/{}/revocations",
                requested.support_access_grant_id
            ),
            Some("support-session-cannot-write"),
            &RevokeSupportAccessRequest {
                expected_revision: Revision::new(2).unwrap(),
                reason: "This must be denied at the support transport boundary".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(support_write.status(), StatusCode::FORBIDDEN);

    let mut unprivileged_tx = tenant_tx(&fixture.db, target_tenant).await;
    let hidden_grants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM support_access_grants")
        .fetch_one(&mut *unprivileged_tx)
        .await
        .unwrap();
    let forged = sqlx::query(
        "UPDATE support_access_grants SET expires_at=expires_at+interval '1 hour' WHERE id=$1",
    )
    .bind(requested.support_access_grant_id)
    .execute(&mut *unprivileged_tx)
    .await
    .unwrap();
    unprivileged_tx.commit().await.unwrap();
    assert_eq!(hidden_grants, 0);
    assert_eq!(forged.rows_affected(), 0);

    let list: SupportAccessPage = decode(
        app.clone()
            .oneshot(request(
                &approver_token,
                approver_home,
                Method::GET,
                &format!(
                    "/api/v1/platform/support-access?tenant_id={}&status=active&limit=20",
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
    assert_eq!(list.items.len(), 1);

    let revoker_token = if approved.approved_by == Some(approver.id) {
        &approver_token
    } else {
        &competing_token
    };
    let revoker_home = if approved.approved_by == Some(approver.id) {
        approver_home
    } else {
        competing_home
    };
    let revoked: SupportAccessResponse = decode(
        app.clone()
            .oneshot(request(
                revoker_token,
                revoker_home,
                Method::POST,
                &format!(
                    "/api/v1/platform/support-access/{}/revocations",
                    requested.support_access_grant_id
                ),
                Some("revoke-support-inc-4242"),
                &RevokeSupportAccessRequest {
                    expected_revision: Revision::new(2).unwrap(),
                    reason: "Incident investigation complete".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(revoked.status, SupportAccessStatus::Revoked);
    assert_eq!(revoked.revision.get(), 3);

    let denied_after_revoke = app
        .clone()
        .oneshot(request(
            &requester_token,
            target_tenant,
            Method::GET,
            "/api/v1/inventory/balances?limit=20",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(denied_after_revoke.status(), StatusCode::FORBIDDEN);
    assert!(
        repo::tenants::access_for_user(&fixture.db, requester.id, target_tenant)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !permissions::user_has_permission(&fixture.db, target_tenant, requester.id, "wms")
            .await
            .unwrap()
    );
    assert!(
        auth::web_session_context_for_token(&fixture.db, &security, &requester_web_token)
            .await
            .unwrap()
            .is_none()
    );

    let events: SupportAccessEventPage = decode(
        app.clone()
            .oneshot(request(
                revoker_token,
                revoker_home,
                Method::GET,
                &format!(
                    "/api/v1/platform/support-access/{}/events?limit=20",
                    requested.support_access_grant_id
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
    assert_eq!(events.items[0].action, "revoked");
    assert_eq!(events.items[1].action, "approved");
    assert_eq!(events.items[2].action, "requested");

    let mut tx = tenant_tx(&fixture.db, target_tenant).await;
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM outbox_events WHERE aggregate_type='support_access' AND aggregate_id=$1",
    )
    .bind(requested.support_access_grant_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(outbox_count, 3);
}

#[tokio::test]
async fn support_access_expiration_is_enforced_without_a_cleanup_job() {
    let fixture = Fixture::new().await;
    let requester = fixture.user("support-expiry-requester@test.local").await;
    let approver = fixture.user("support-expiry-approver@test.local").await;
    let target_admin = fixture.user("support-expiry-target@test.local").await;
    let requester_home = tenant_for_user(&fixture.db, requester.id).await;
    let approver_home = tenant_for_user(&fixture.db, approver.id).await;
    let target_tenant = tenant_for_user(&fixture.db, target_admin.id).await;
    grant_platform_administrator(&fixture.db, requester.id).await;
    grant_platform_administrator(&fixture.db, approver.id).await;
    let requester_token = auth::create_session(&fixture.db, requester.id)
        .await
        .unwrap();
    let approver_token = auth::create_session(&fixture.db, approver.id)
        .await
        .unwrap();
    let facility = fixture.facility(target_tenant, "Expiry DC").await;
    let owner = fixture.inventory_owner(target_tenant, "Expiry owner").await;
    wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        target_tenant,
        "wms",
        Some("WMS"),
    )
    .await
    .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let requested: SupportAccessResponse = decode(
        app.clone()
            .oneshot(request(
                &requester_token,
                requester_home,
                Method::POST,
                "/api/v1/platform/support-access",
                Some("request-expiring-support"),
                &RequestSupportAccessRequest {
                    tenant_id: target_tenant.get(),
                    reason: "Short diagnostic window".into(),
                    expires_at: (Utc::now() + Duration::seconds(3)).to_rfc3339(),
                    access: SupportAccessPolicyRequest {
                        all_facilities: false,
                        facility_ids: vec![facility],
                        all_inventory_owners: false,
                        inventory_owner_ids: vec![owner],
                        permission_names: vec!["wms".into()],
                    },
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    let approved: SupportAccessResponse = decode(
        app.clone()
            .oneshot(request(
                &approver_token,
                approver_home,
                Method::POST,
                &format!(
                    "/api/v1/platform/support-access/{}/approvals",
                    requested.support_access_grant_id
                ),
                Some("approve-expiring-support"),
                &ApproveSupportAccessRequest {
                    expected_revision: Revision::new(1).unwrap(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(approved.status, SupportAccessStatus::Active);
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    assert!(
        repo::tenants::access_for_user(&fixture.db, requester.id, target_tenant)
            .await
            .unwrap()
            .is_none()
    );
    let detail: SupportAccessResponse = decode(
        app.clone()
            .oneshot(request(
                &approver_token,
                approver_home,
                Method::GET,
                &format!(
                    "/api/v1/platform/support-access/{}",
                    requested.support_access_grant_id
                ),
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(detail.status, SupportAccessStatus::Expired);
}
