mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    EmployeeIdentityChangeKind, EmployeeIdentityChangeResponse, ErrorReason, ErrorResponse,
    LinkEmployeeIdentityRequest, UnlinkEmployeeIdentityRequest,
};
use wareboxes_application::workforce_identity::{
    LINK_EMPLOYEE_IDENTITY_OPERATION, UNLINK_EMPLOYEE_IDENTITY_OPERATION,
};

fn command_request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    employee_id: i64,
    action: &str,
    key: Option<&str>,
    body: &T,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/api/v1/workforce/employees/{employee_id}/identity-{action}"
        ))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = key {
        request = request
            .header(IDEMPOTENCY_KEY_HEADER, key)
            .header(REQUEST_ID_HEADER, format!("request-{key}"));
    }
    request
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn grant_admin(db: &db::Db, tenant_id: TenantId, user_id: i64, role_name: &str) {
    let permission =
        match wareboxes_persistence_postgres::permissions::find_by_name(db, tenant_id, "admin")
            .await
            .unwrap()
        {
            Some(permission) => permission.id,
            None => wareboxes_persistence_postgres::permissions::add_permission(
                db,
                tenant_id,
                "admin",
                Some("Tenant administrator"),
            )
            .await
            .unwrap(),
        };
    let role = wareboxes_persistence_postgres::roles::add_role(
        db,
        tenant_id,
        role_name,
        Some("Workforce identity administrator"),
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(db, tenant_id, role, permission)
        .await
        .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(db, tenant_id, user_id, role)
        .await
        .unwrap();
}

async fn add_membership(db: &db::Db, tenant_id: TenantId, user_id: i64) {
    let mut tx = tenant_tx(db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id,user_id) VALUES ($1,$2)")
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn employee(
    fixture: &Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    facility_id: i64,
    key: &str,
) -> i64 {
    let access = repo::tenants::access_for_user(&fixture.db, actor_id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    repo::employees::add_employee(
        &fixture.db,
        tenant_id,
        &access.site_scope,
        &repo::employees::NewEmployee {
            first_name: key,
            last_name: "Operator",
            title: "Warehouse operator",
            employee_type: "employee",
            email: None,
            phone: None,
            hired: db::now_iso(),
            facility_ids: &[facility_id],
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn identity_lifecycle_is_replay_safe_audited_and_blocked_during_open_attendance() {
    let fixture = Fixture::new().await;
    let administrator = fixture.user("identity-admin@test.local").await;
    let target = fixture.user("identity-target@test.local").await;
    let replacement = fixture.user("identity-replacement@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, administrator.id).await;
    grant_admin(&fixture.db, tenant_id, administrator.id, "identity-admin").await;
    add_membership(&fixture.db, tenant_id, target.id).await;
    add_membership(&fixture.db, tenant_id, replacement.id).await;
    let facility_id = fixture.facility(tenant_id, "Identity DC").await;
    let employee_id = employee(
        &fixture,
        tenant_id,
        administrator.id,
        facility_id,
        "Identity",
    )
    .await;
    let token = auth::create_session(&fixture.db, administrator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let link = LinkEmployeeIdentityRequest {
        user_id: target.id,
        expected_user_id: None,
        reason: "enable interactive workforce access".into(),
    };

    let missing_key = app
        .clone()
        .oneshot(command_request(
            &token,
            tenant_id,
            employee_id,
            "links",
            None,
            &link,
        ))
        .await
        .unwrap();
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(missing_key).await.reason,
        ErrorReason::IdempotencyKeyRequired
    );

    let first = app
        .clone()
        .oneshot(command_request(
            &token,
            tenant_id,
            employee_id,
            "links",
            Some("identity-link"),
            &link,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: EmployeeIdentityChangeResponse = response_json(first).await;
    assert_eq!(first.kind, EmployeeIdentityChangeKind::Linked);
    assert_eq!(first.previous_user_id, None);
    assert_eq!(first.user_id, Some(target.id));
    assert_eq!(first.resulting_revision, 1);

    let replay = app
        .clone()
        .oneshot(command_request(
            &token,
            tenant_id,
            employee_id,
            "links",
            Some("identity-link"),
            &link,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<EmployeeIdentityChangeResponse>(replay).await,
        first
    );

    let reused = app
        .clone()
        .oneshot(command_request(
            &token,
            tenant_id,
            employee_id,
            "links",
            Some("identity-link"),
            &LinkEmployeeIdentityRequest {
                reason: "different request".into(),
                ..link.clone()
            },
        ))
        .await
        .unwrap();
    assert_eq!(reused.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(reused).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let relink = app
        .clone()
        .oneshot(command_request(
            &token,
            tenant_id,
            employee_id,
            "links",
            Some("identity-relink"),
            &LinkEmployeeIdentityRequest {
                user_id: replacement.id,
                expected_user_id: Some(target.id),
                reason: "replace interactive account".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(relink.status(), StatusCode::OK);
    let relink: EmployeeIdentityChangeResponse = response_json(relink).await;
    assert_eq!(relink.kind, EmployeeIdentityChangeKind::Relinked);
    assert_eq!(relink.previous_user_id, Some(target.id));
    assert_eq!(relink.user_id, Some(replacement.id));

    let unlink = app
        .clone()
        .oneshot(command_request(
            &token,
            tenant_id,
            employee_id,
            "unlinks",
            Some("identity-unlink"),
            &UnlinkEmployeeIdentityRequest {
                expected_user_id: replacement.id,
                reason: "interactive access ended".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(unlink.status(), StatusCode::OK);
    let unlink: EmployeeIdentityChangeResponse = response_json(unlink).await;
    assert_eq!(unlink.kind, EmployeeIdentityChangeKind::Unlinked);
    assert_eq!(unlink.user_id, None);

    let linked_again = app
        .clone()
        .oneshot(command_request(
            &token,
            tenant_id,
            employee_id,
            "links",
            Some("identity-link-open"),
            &LinkEmployeeIdentityRequest {
                user_id: target.id,
                expected_user_id: None,
                reason: "restore interactive access".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(linked_again.status(), StatusCode::OK);

    let mut attendance_tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO attendance_intervals
          (tenant_id,employee_id,facility_id,status,revision,clocked_in_at,clocked_in_by_user_id)
        VALUES ($1,$2,$3,'open',1,$4,$5)
        "#,
    )
    .bind(tenant_id.get())
    .bind(employee_id)
    .bind(facility_id)
    .bind(db::now_iso())
    .bind(administrator.id)
    .execute(&mut *attendance_tx)
    .await
    .unwrap();
    attendance_tx.commit().await.unwrap();

    for request in [
        command_request(
            &token,
            tenant_id,
            employee_id,
            "unlinks",
            Some("identity-open-unlink"),
            &UnlinkEmployeeIdentityRequest {
                expected_user_id: target.id,
                reason: "blocked while clocked in".into(),
            },
        ),
        command_request(
            &token,
            tenant_id,
            employee_id,
            "links",
            Some("identity-open-relink"),
            &LinkEmployeeIdentityRequest {
                user_id: replacement.id,
                expected_user_id: Some(target.id),
                reason: "blocked while clocked in".into(),
            },
        ),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let evidence: (Option<i64>, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT employee.user_id,employee.identity_revision,
          (SELECT COUNT(*) FROM employee_identity_changes change
            WHERE change.tenant_id=$1 AND change.employee_id=$2),
          (SELECT COUNT(*) FROM outbox_events event
            WHERE event.tenant_id=$1 AND event.aggregate_type='employee'
              AND event.aggregate_id=$2::text
              AND event.event_type LIKE 'workforce.employee_identity.%'),
          (SELECT COUNT(*) FROM command_idempotency_records record
            WHERE record.tenant_id=$1 AND record.operation IN($3,$4))
        FROM employees employee WHERE employee.tenant_id=$1 AND employee.id=$2
        "#,
    )
    .bind(tenant_id.get())
    .bind(employee_id)
    .bind(LINK_EMPLOYEE_IDENTITY_OPERATION)
    .bind(UNLINK_EMPLOYEE_IDENTITY_OPERATION)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(evidence, (Some(target.id), 4, 4, 4, 4));
}

#[tokio::test]
async fn employee_offboarding_requires_identity_unlink_before_termination() {
    let fixture = Fixture::new().await;
    let administrator = fixture.user("offboarding-admin@test.local").await;
    let target = fixture.user("offboarding-target@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, administrator.id).await;
    grant_admin(
        &fixture.db,
        tenant_id,
        administrator.id,
        "offboarding-admin",
    )
    .await;
    add_membership(&fixture.db, tenant_id, target.id).await;
    let facility_id = fixture.facility(tenant_id, "Offboarding DC").await;
    let employee_id = employee(
        &fixture,
        tenant_id,
        administrator.id,
        facility_id,
        "Offboarding",
    )
    .await;
    let token = auth::create_session(&fixture.db, administrator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let linked = app
        .clone()
        .oneshot(command_request(
            &token,
            tenant_id,
            employee_id,
            "links",
            Some("offboarding-link"),
            &LinkEmployeeIdentityRequest {
                user_id: target.id,
                expected_user_id: None,
                reason: "enable workforce access".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(linked.status(), StatusCode::OK);

    let access = repo::tenants::access_for_user(&fixture.db, administrator.id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    let terminated_at = db::now_iso();
    let blocked = repo::employees::update_employee(
        &fixture.db,
        tenant_id,
        &access.site_scope,
        employee_id,
        &repo::employees::EmployeeChanges {
            first_name: None,
            last_name: None,
            title: None,
            employee_type: None,
            email: None,
            phone: None,
            terminated: Some(terminated_at),
            facility_ids: None,
        },
    )
    .await;
    assert!(blocked.is_err(), "linked employees cannot be terminated");

    let unlinked = app
        .clone()
        .oneshot(command_request(
            &token,
            tenant_id,
            employee_id,
            "unlinks",
            Some("offboarding-unlink"),
            &UnlinkEmployeeIdentityRequest {
                expected_user_id: target.id,
                reason: "revoke workforce access before termination".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(unlinked.status(), StatusCode::OK);
    assert!(repo::employees::update_employee(
        &fixture.db,
        tenant_id,
        &access.site_scope,
        employee_id,
        &repo::employees::EmployeeChanges {
            first_name: None,
            last_name: None,
            title: None,
            employee_type: None,
            email: None,
            phone: None,
            terminated: Some(terminated_at),
            facility_ids: None,
        },
    )
    .await
    .unwrap());

    let relink = app
        .oneshot(command_request(
            &token,
            tenant_id,
            employee_id,
            "links",
            Some("offboarding-relink"),
            &LinkEmployeeIdentityRequest {
                user_id: target.id,
                expected_user_id: None,
                reason: "attempt to restore a terminated identity".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(relink.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn identity_routes_are_permission_and_tenant_scope_concealed() {
    let fixture = Fixture::new().await;
    let administrator = fixture.user("identity-scope-admin@test.local").await;
    let operator = fixture.user("identity-scope-operator@test.local").await;
    let target = fixture.user("identity-scope-target@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, administrator.id).await;
    grant_admin(
        &fixture.db,
        tenant_id,
        administrator.id,
        "identity-scope-admin",
    )
    .await;
    add_membership(&fixture.db, tenant_id, operator.id).await;
    add_membership(&fixture.db, tenant_id, target.id).await;
    let facility_id = fixture.facility(tenant_id, "Scoped Identity DC").await;
    let employee_id = employee(&fixture, tenant_id, administrator.id, facility_id, "Scoped").await;
    let admin_token = auth::create_session(&fixture.db, administrator.id)
        .await
        .unwrap();
    let operator_token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let body = LinkEmployeeIdentityRequest {
        user_id: target.id,
        expected_user_id: None,
        reason: "scope test".into(),
    };

    let forbidden = app
        .clone()
        .oneshot(command_request(
            &operator_token,
            tenant_id,
            employee_id,
            "links",
            Some("identity-forbidden"),
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let other = fixture.user("identity-other-tenant@test.local").await;
    let other_tenant = tenant_for_user(&fixture.db, other.id).await;
    let other_facility = fixture.facility(other_tenant, "Other Identity DC").await;
    let other_employee = employee(&fixture, other_tenant, other.id, other_facility, "Other").await;
    for employee_id in [other_employee, i64::MAX] {
        let concealed = app
            .clone()
            .oneshot(command_request(
                &admin_token,
                tenant_id,
                employee_id,
                "links",
                Some(&format!("identity-concealed-{employee_id}")),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
    }

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effect_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM employee_identity_changes WHERE tenant_id=$1")
            .bind(tenant_id.get())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effect_count, 0);
}
