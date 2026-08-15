mod common;

use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ClockInRequest, LaborReferenceCandidatePageResponse, LaborReferenceType,
    LaborRosterPageResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    supervisor_id: i64,
    supervisor_token: String,
    operator_id: i64,
    operator_token: String,
    viewer_token: String,
    facility_id: i64,
    hidden_facility_id: i64,
    owner_id: i64,
    operator_employee_id: i64,
    foreign_facility_id: i64,
    foreign_employee_id: i64,
    app: axum::Router,
}

impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let supervisor = fixture.user("labor-candidate-supervisor@test.local").await;
        let tenant_id = tenant_for_user(&fixture.db, supervisor.id).await;
        grant_permissions(
            &fixture,
            tenant_id,
            supervisor.id,
            "labor-candidate-supervisor",
            &[
                "admin",
                "labor_view",
                "labor_supervise",
                "labor_certify",
                "wms",
            ],
        )
        .await;

        let operator = fixture.user("labor-candidate-operator@test.local").await;
        add_membership(&fixture, tenant_id, operator.id).await;
        grant_permissions(
            &fixture,
            tenant_id,
            operator.id,
            "labor-candidate-operator",
            &["labor_view", "labor_execute", "wms"],
        )
        .await;
        let viewer = fixture.user("labor-candidate-viewer@test.local").await;
        add_membership(&fixture, tenant_id, viewer.id).await;
        grant_permissions(
            &fixture,
            tenant_id,
            viewer.id,
            "labor-candidate-viewer",
            &["labor_view"],
        )
        .await;

        let facility_id = fixture
            .facility(tenant_id, "Labor Candidate Facility")
            .await;
        let hidden_facility_id = fixture.facility(tenant_id, "Hidden Labor Facility").await;
        let owner_id = fixture
            .inventory_owner(tenant_id, "Labor Candidate Owner")
            .await;
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
        let supervisor_access =
            repo::tenants::access_for_user(&fixture.db, supervisor.id, tenant_id)
                .await
                .unwrap()
                .unwrap();
        let operator_employee_id = add_linked_employee(
            &fixture,
            &supervisor_access,
            supervisor.id,
            operator.id,
            &operator.email,
            "Alex",
            "Operator",
            facility_id,
            "operator",
        )
        .await;
        add_linked_employee(
            &fixture,
            &supervisor_access,
            supervisor.id,
            supervisor.id,
            &supervisor.email,
            "Sam",
            "Supervisor",
            facility_id,
            "supervisor",
        )
        .await;
        assert!(repo::tenants::update_user_access_scope(
            &fixture.db,
            tenant_id,
            &UpdateUserAccessScope {
                user_id: operator.id,
                all_facilities: false,
                facility_ids: vec![facility_id],
                all_inventory_owners: false,
                inventory_owner_ids: vec![owner_id],
            },
        )
        .await
        .unwrap());

        let foreign_user = fixture.user("labor-candidate-foreign@test.local").await;
        let foreign_tenant_id = tenant_for_user(&fixture.db, foreign_user.id).await;
        let foreign_facility_id = fixture
            .facility(foreign_tenant_id, "Foreign Labor Facility")
            .await;
        let foreign_access =
            repo::tenants::access_for_user(&fixture.db, foreign_user.id, foreign_tenant_id)
                .await
                .unwrap()
                .unwrap();
        let foreign_employee_id = repo::employees::add_employee(
            &fixture.db,
            foreign_tenant_id,
            &foreign_access.site_scope,
            &repo::employees::NewEmployee {
                first_name: "Foreign",
                last_name: "Employee",
                title: "Associate",
                employee_type: "hourly",
                email: Some(&foreign_user.email),
                phone: None,
                hired: wareboxes_api::db::now_iso() - Duration::from_secs(86_400),
                facility_ids: &[foreign_facility_id],
            },
        )
        .await
        .unwrap();

        let supervisor_token = wareboxes_api::auth::create_session(&fixture.db, supervisor.id)
            .await
            .unwrap();
        let operator_token = wareboxes_api::auth::create_session(&fixture.db, operator.id)
            .await
            .unwrap();
        let viewer_token = wareboxes_api::auth::create_session(&fixture.db, viewer.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            tenant_id,
            supervisor_id: supervisor.id,
            supervisor_token,
            operator_id: operator.id,
            operator_token,
            viewer_token,
            facility_id,
            hidden_facility_id,
            owner_id,
            operator_employee_id,
            foreign_facility_id,
            foreign_employee_id,
            app,
        }
    }

    async fn send<T: Serialize>(
        &self,
        token: &str,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<&T>,
    ) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(request(token, self.tenant_id, method, path, key, body))
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn roster_is_permission_gated_cursor_bounded_and_scope_concealed() {
    let rig = Rig::new().await;
    let forbidden = rig
        .send::<serde_json::Value>(
            &rig.viewer_token,
            Method::GET,
            &format!("/api/v1/labor/roster?facility_id={}", rig.facility_id),
            None,
            None,
        )
        .await;
    assert_status(forbidden, StatusCode::FORBIDDEN).await;

    let first: LaborRosterPageResponse = json(
        rig.send::<serde_json::Value>(
            &rig.supervisor_token,
            Method::GET,
            &format!(
                "/api/v1/labor/roster?facility_id={}&inventory_owner_id={}&limit=1",
                rig.facility_id, rig.owner_id
            ),
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(first.items.len(), 1);
    let cursor = first
        .next_cursor
        .expect("two scoped employees yield a next page");
    let second: LaborRosterPageResponse = json(
        rig.send::<serde_json::Value>(
            &rig.supervisor_token,
            Method::GET,
            &format!(
                "/api/v1/labor/roster?facility_id={}&inventory_owner_id={}&limit=1&cursor={}",
                rig.facility_id, rig.owner_id, cursor
            ),
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(second.items.len(), 1);
    assert_ne!(first.items[0].employee_id, second.items[0].employee_id);
    assert!(first.items[0]
        .eligibility_evidence
        .iter()
        .any(|line| line.contains("facility assignment")));

    let changed_filter = rig
        .send::<serde_json::Value>(
            &rig.supervisor_token,
            Method::GET,
            &format!(
                "/api/v1/labor/roster?facility_id={}&limit=1&cursor={}",
                rig.facility_id, cursor
            ),
            None,
            None,
        )
        .await;
    assert_status(changed_filter, StatusCode::BAD_REQUEST).await;
    let oversized = rig
        .send::<serde_json::Value>(
            &rig.supervisor_token,
            Method::GET,
            &format!(
                "/api/v1/labor/roster?facility_id={}&limit=101",
                rig.facility_id
            ),
            None,
            None,
        )
        .await;
    assert_status(oversized, StatusCode::BAD_REQUEST).await;
    for guessed_facility in [rig.hidden_facility_id, rig.foreign_facility_id] {
        let concealed = rig
            .send::<serde_json::Value>(
                &rig.operator_token,
                Method::GET,
                &format!("/api/v1/labor/roster?facility_id={guessed_facility}"),
                None,
                None,
            )
            .await;
        assert_status(concealed, StatusCode::NOT_FOUND).await;
    }
}

#[tokio::test]
async fn reference_candidates_return_only_executable_typed_work_and_hide_guesses() {
    let rig = Rig::new().await;
    let _: wareboxes_api_contract::v1::AttendanceIntervalResponse = json(
        rig.send(
            &rig.operator_token,
            Method::POST,
            "/api/v1/labor/attendance",
            Some("candidate-clock-in"),
            Some(&ClockInRequest {
                employee_id: rig.operator_employee_id,
                facility_id: rig.facility_id,
                note: Some("Candidate acceptance shift".into()),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let location_id = rig
        .fixture
        .location(rig.tenant_id, rig.facility_id, "CANDIDATE-COUNT")
        .await;
    let task_id = repo::tasks::create_location_cycle_count_task(
        &rig.fixture.db,
        rig.tenant_id,
        rig.supervisor_id,
        location_id,
        Some(20),
        Some(rig.operator_id),
        None,
        None,
        Some("Count candidate location".into()),
    )
    .await
    .unwrap();
    let access = repo::tenants::access_for_user(&rig.fixture.db, rig.operator_id, rig.tenant_id)
        .await
        .unwrap()
        .unwrap();
    assert!(repo::tasks::start_task_in_scope(
        &rig.fixture.db,
        &access,
        &wareboxes_application::CommandContext {
            tenant_id: rig.tenant_id,
            actor_id: wareboxes_domain::UserId::new(rig.operator_id).unwrap(),
            request_id: "candidate-task-claim".into(),
            idempotency_key: Some("candidate-task-claim".into()),
        },
        task_id,
    )
    .await
    .unwrap());

    let page: LaborReferenceCandidatePageResponse = json(
        rig.send::<serde_json::Value>(
            &rig.operator_token,
            Method::GET,
            &format!(
                "/api/v1/labor/reference-candidates?facility_id={}&employee_id={}&activity_kind=cycle_count&quantity_basis=task&limit=10",
                rig.facility_id, rig.operator_employee_id
            ),
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(page.employee_id, rig.operator_employee_id);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].reference_type, LaborReferenceType::WorkTask);
    assert_eq!(page.items[0].reference_id, task_id);
    assert_eq!(page.items[0].canonical_quantity, 1);
    assert!(page.items[0]
        .eligibility_evidence
        .iter()
        .any(|line| line.contains("Canonical task quantity: 1")));

    let invalid_pair = rig
        .send::<serde_json::Value>(
            &rig.operator_token,
            Method::GET,
            &format!(
                "/api/v1/labor/reference-candidates?facility_id={}&employee_id={}&activity_kind=yard&quantity_basis=unit",
                rig.facility_id, rig.operator_employee_id
            ),
            None,
            None,
        )
        .await;
    assert_status(invalid_pair, StatusCode::BAD_REQUEST).await;
    let guessed_employee = rig
        .send::<serde_json::Value>(
            &rig.operator_token,
            Method::GET,
            &format!(
                "/api/v1/labor/reference-candidates?facility_id={}&employee_id={}&activity_kind=cycle_count&quantity_basis=task",
                rig.facility_id, rig.foreign_employee_id
            ),
            None,
            None,
        )
        .await;
    assert_status(guessed_employee, StatusCode::NOT_FOUND).await;
}

async fn add_membership(fixture: &Fixture, tenant_id: TenantId, user_id: i64) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id,user_id) VALUES($1,$2)")
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn add_linked_employee(
    fixture: &Fixture,
    access: &wareboxes_core::models::TenantAccess,
    actor_id: i64,
    user_id: i64,
    email: &str,
    first_name: &str,
    last_name: &str,
    facility_id: i64,
    key: &str,
) -> i64 {
    let employee_id = repo::employees::add_employee(
        &fixture.db,
        access.tenant_id,
        &access.site_scope,
        &repo::employees::NewEmployee {
            first_name,
            last_name,
            title: "Warehouse Associate",
            employee_type: "hourly",
            email: Some(email),
            phone: None,
            hired: wareboxes_api::db::now_iso() - Duration::from_secs(86_400),
            facility_ids: &[facility_id],
        },
    )
    .await
    .unwrap();
    repo::employees::link_employee_identity(
        &fixture.db,
        access,
        &wareboxes_application::CommandContext {
            tenant_id: access.tenant_id,
            actor_id: wareboxes_domain::UserId::new(actor_id).unwrap(),
            request_id: format!("candidate-link-{key}"),
            idempotency_key: Some(format!("candidate-link-{key}")),
        },
        &wareboxes_application::workforce_identity::LinkEmployeeIdentityCommand {
            employee_id: wareboxes_domain::EmployeeId::new(employee_id).unwrap(),
            user_id: wareboxes_domain::UserId::new(user_id).unwrap(),
            expected_user_id: None,
            reason: wareboxes_domain::EmployeeIdentityReason::new(
                "enable scoped labor candidate acceptance",
            )
            .unwrap(),
        },
    )
    .await
    .unwrap();
    employee_id
}

async fn grant_permissions(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    role_name: &str,
    permission_names: &[&str],
) {
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        role_name,
        Some("Labor candidate acceptance role"),
    )
    .await
    .unwrap();
    for name in permission_names {
        let permission = wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            name,
            Some("Labor candidate acceptance permission"),
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
    }
    wareboxes_persistence_postgres::roles::add_role_to_user(&fixture.db, tenant_id, user_id, role)
        .await
        .unwrap();
}

fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    key: Option<&str>,
    body: Option<&T>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = key {
        builder = builder.header(IDEMPOTENCY_KEY_HEADER, key);
    }
    let body = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(body).unwrap())
    } else {
        Body::empty()
    };
    builder.body(body).unwrap()
}

async fn response(value: axum::response::Response) -> (StatusCode, axum::body::Bytes) {
    let status = value.status();
    let body = to_bytes(value.into_body(), 2 * 1024 * 1024).await.unwrap();
    (status, body)
}

async fn assert_status(value: axum::response::Response, expected: StatusCode) {
    let (status, body) = response(value).await;
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&body));
}

async fn json<T: serde::de::DeserializeOwned>(
    value: axum::response::Response,
    expected: StatusCode,
) -> T {
    let (status, body) = response(value).await;
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).unwrap()
}
