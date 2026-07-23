mod common;

use std::collections::BTreeSet;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::{Employee, SiteScope};
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::{routes, state::AppState};

fn api_request(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&body).unwrap())
        }
        None => Body::empty(),
    };
    request.body(body).unwrap()
}

async fn send_api(
    app: &axum::Router,
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    app.clone()
        .oneshot(api_request(token, tenant_id, method, uri, body))
        .await
        .unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn add_employee(
    db: &db::Db,
    tenant_id: TenantId,
    site_scope: &SiteScope,
    name: &str,
    facility_ids: &[i64],
) -> i64 {
    repo::employees::add_employee(
        db,
        tenant_id,
        site_scope,
        &repo::employees::NewEmployee {
            first_name: name,
            last_name: "Employee",
            title: "Associate",
            employee_type: "hourly",
            email: None,
            phone: None,
            hired: db::now_iso(),
            facility_ids,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn employee_routes_and_repositories_enforce_facility_scope() {
    let db = setup().await;
    let administrator = auth::register_user(
        &db,
        "employee-scope-admin@test.com",
        "supersecret",
        None,
        None,
    )
    .await
    .unwrap();
    let operator = auth::register_user(
        &db,
        "employee-scope-operator@test.com",
        "supersecret",
        None,
        None,
    )
    .await
    .unwrap();
    let tenant_id = tenant_for_user(&db, administrator.id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(operator.id)
        .execute(&db)
        .await
        .unwrap();

    let permission = repo::permissions::add_permission(&db, tenant_id, "admin", Some("Admin"))
        .await
        .unwrap();
    let role = repo::roles::add_role(
        &db,
        tenant_id,
        "employee-scope-operator",
        Some("Scoped workforce administrator"),
    )
    .await
    .unwrap();
    assert!(
        repo::roles::add_role_permission(&db, tenant_id, role, permission)
            .await
            .unwrap()
    );
    assert!(
        repo::roles::add_role_to_user(&db, tenant_id, operator.id, role)
            .await
            .unwrap()
    );

    let allowed_facility_a = repo::facilities::add_facility(&db, tenant_id, "Allowed Employee A")
        .await
        .unwrap();
    let allowed_facility_b = repo::facilities::add_facility(&db, tenant_id, "Allowed Employee B")
        .await
        .unwrap();
    let denied_facility = repo::facilities::add_facility(&db, tenant_id, "Denied Employee DC")
        .await
        .unwrap();

    let unrestricted_scope = SiteScope {
        all_facilities: true,
        facility_ids: Vec::new(),
    };
    let allowed_employee = add_employee(
        &db,
        tenant_id,
        &unrestricted_scope,
        "Allowed",
        &[allowed_facility_a],
    )
    .await;
    let denied_employee = add_employee(
        &db,
        tenant_id,
        &unrestricted_scope,
        "Denied",
        &[denied_facility],
    )
    .await;
    let mixed_employee = add_employee(
        &db,
        tenant_id,
        &unrestricted_scope,
        "Mixed",
        &[allowed_facility_a, denied_facility],
    )
    .await;

    assert!(repo::tenants::update_user_access_scope(
        &db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![allowed_facility_a, allowed_facility_b],
            all_inventory_owners: true,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap());
    let operator_access = repo::tenants::access_for_user(&db, operator.id, tenant_id)
        .await
        .unwrap()
        .unwrap();

    let scoped_employees =
        repo::employees::get_employees_in_scope(&db, tenant_id, &operator_access.site_scope, false)
            .await
            .unwrap();
    assert_eq!(
        scoped_employees
            .iter()
            .map(|employee| employee.id)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([allowed_employee, mixed_employee])
    );
    assert_eq!(
        scoped_employees
            .iter()
            .find(|employee| employee.id == mixed_employee)
            .unwrap()
            .facility_ids,
        vec![allowed_facility_a]
    );
    assert!(
        scoped_employees
            .iter()
            .find(|employee| employee.id == allowed_employee)
            .unwrap()
            .can_manage
    );
    assert!(
        !scoped_employees
            .iter()
            .find(|employee| employee.id == mixed_employee)
            .unwrap()
            .can_manage
    );

    let hidden_change = repo::employees::EmployeeChanges {
        first_name: Some("Hidden changed"),
        last_name: None,
        title: None,
        employee_type: None,
        email: None,
        phone: None,
        terminated: None,
        facility_ids: None,
    };
    assert!(!repo::employees::update_employee(
        &db,
        tenant_id,
        &operator_access.site_scope,
        denied_employee,
        &hidden_change,
    )
    .await
    .unwrap());
    assert!(!repo::employees::update_employee(
        &db,
        tenant_id,
        &operator_access.site_scope,
        mixed_employee,
        &hidden_change,
    )
    .await
    .unwrap());
    assert!(!repo::employees::set_employee_deleted(
        &db,
        tenant_id,
        &operator_access.site_scope,
        denied_employee,
        true,
    )
    .await
    .unwrap());
    assert!(!repo::employees::set_employee_deleted(
        &db,
        tenant_id,
        &operator_access.site_scope,
        i64::MAX,
        true,
    )
    .await
    .unwrap());
    assert!(repo::employees::update_employee(
        &db,
        tenant_id,
        &operator_access.site_scope,
        allowed_employee,
        &repo::employees::EmployeeChanges {
            first_name: None,
            last_name: None,
            title: None,
            employee_type: None,
            email: None,
            phone: None,
            terminated: None,
            facility_ids: Some(&[denied_facility]),
        },
    )
    .await
    .is_err());

    let token = auth::create_session(&db, operator.id).await.unwrap();
    let app = routes::app(AppState::new(db.clone()));
    let response = send_api(&app, &token, tenant_id, Method::GET, "/api/employees", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Vec<Employee>>(response)
            .await
            .into_iter()
            .map(|employee| employee.id)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([allowed_employee, mixed_employee])
    );

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/employees/add",
        Some(json!({
            "first_name": "Escalated",
            "last_name": "Employee",
            "title": "Associate",
            "type": "hourly",
            "facility_ids": [denied_facility]
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/employees/add",
        Some(json!({
            "first_name": "Route",
            "last_name": "Employee",
            "title": "Associate",
            "type": "hourly",
            "facility_ids": [allowed_facility_a]
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let route_employee = response_json::<i64>(response).await;

    for employee_id in [denied_employee, mixed_employee, i64::MAX] {
        let response = send_api(
            &app,
            &token,
            tenant_id,
            Method::POST,
            "/api/employees/update",
            Some(json!({
                "employee_id": employee_id,
                "first_name": "Unauthorized"
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response_json::<bool>(response).await);
    }

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/employees/update",
        Some(json!({
            "employee_id": route_employee,
            "facility_ids": [allowed_facility_a, denied_facility]
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/employees/update",
        Some(json!({
            "employee_id": route_employee,
            "first_name": "Route Updated",
            "facility_ids": [allowed_facility_b]
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<bool>(response).await);

    for uri in ["/api/employees/delete", "/api/employees/restore"] {
        let response = send_api(
            &app,
            &token,
            tenant_id,
            Method::POST,
            uri,
            Some(json!({"employee_id": denied_employee})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response_json::<bool>(response).await);
    }
    for uri in ["/api/employees/delete", "/api/employees/restore"] {
        let response = send_api(
            &app,
            &token,
            tenant_id,
            Method::POST,
            uri,
            Some(json!({"employee_id": route_employee})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_json::<bool>(response).await);
    }

    let route_employee =
        repo::employees::get_employees_in_scope(&db, tenant_id, &operator_access.site_scope, false)
            .await
            .unwrap()
            .into_iter()
            .find(|employee| employee.id == route_employee)
            .unwrap();
    assert_eq!(route_employee.first_name, "Route Updated");
    assert_eq!(route_employee.facility_ids, vec![allowed_facility_b]);

    let denied_employee =
        repo::employees::get_employees_in_scope(&db, tenant_id, &unrestricted_scope, false)
            .await
            .unwrap()
            .into_iter()
            .find(|employee| employee.id == denied_employee)
            .unwrap();
    assert_eq!(denied_employee.first_name, "Denied");
    assert_eq!(denied_employee.facility_ids, vec![denied_facility]);

    assert!(sqlx::query(
        r#"
        UPDATE employee_facilities
        SET employee_id = $1
        WHERE tenant_id = $2 AND employee_id = $3 AND deleted IS NULL
        "#,
    )
    .bind(route_employee.id)
    .bind(tenant_id.get())
    .bind(denied_employee.id)
    .execute(&db)
    .await
    .is_err());
    assert!(sqlx::query(
        r#"
        UPDATE employee_facilities
        SET deleted = clock_timestamp()
        WHERE tenant_id = $1 AND employee_id = $2 AND deleted IS NULL
        "#,
    )
    .bind(tenant_id.get())
    .bind(denied_employee.id)
    .execute(&db)
    .await
    .is_err());
    assert!(sqlx::query(
        "UPDATE facilities SET deleted = clock_timestamp() WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(denied_facility)
    .execute(&db)
    .await
    .is_err());
    assert!(sqlx::query(
        r#"
        INSERT INTO employees
            (tenant_id, created, first_name, last_name, title, type, hired)
        VALUES ($1, clock_timestamp(), 'Unscoped', 'Employee', 'Picker', 'employee', clock_timestamp())
        "#,
    )
    .bind(tenant_id.get())
    .execute(&db)
    .await
    .is_err());

    let retiring_facility = repo::facilities::add_facility(&db, tenant_id, "Retiring Employee DC")
        .await
        .unwrap();
    let remaining_facility =
        repo::facilities::add_facility(&db, tenant_id, "Remaining Employee DC")
            .await
            .unwrap();
    let reassigned_employee = add_employee(
        &db,
        tenant_id,
        &unrestricted_scope,
        "Facility Retirement",
        &[retiring_facility, remaining_facility],
    )
    .await;
    sqlx::query(
        "UPDATE facilities SET deleted = clock_timestamp() WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(retiring_facility)
    .execute(&db)
    .await
    .unwrap();
    let reassigned_employee =
        repo::employees::get_employees_in_scope(&db, tenant_id, &unrestricted_scope, false)
            .await
            .unwrap()
            .into_iter()
            .find(|employee| employee.id == reassigned_employee)
            .unwrap();
    assert_eq!(reassigned_employee.facility_ids, vec![remaining_facility]);

    let concurrent_facility_a =
        repo::facilities::add_facility(&db, tenant_id, "Concurrent Employee A")
            .await
            .unwrap();
    let concurrent_facility_b =
        repo::facilities::add_facility(&db, tenant_id, "Concurrent Employee B")
            .await
            .unwrap();
    let concurrent_employee = add_employee(
        &db,
        tenant_id,
        &unrestricted_scope,
        "Concurrent Assignment",
        &[concurrent_facility_a, concurrent_facility_b],
    )
    .await;
    let mut tx_a = db.begin().await.unwrap();
    let mut tx_b = db.begin().await.unwrap();
    sqlx::query(
        "UPDATE employee_facilities SET deleted = clock_timestamp() WHERE tenant_id = $1 AND employee_id = $2 AND facility_id = $3",
    )
    .bind(tenant_id.get())
    .bind(concurrent_employee)
    .bind(concurrent_facility_a)
    .execute(&mut *tx_a)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE employee_facilities SET deleted = clock_timestamp() WHERE tenant_id = $1 AND employee_id = $2 AND facility_id = $3",
    )
    .bind(tenant_id.get())
    .bind(concurrent_employee)
    .bind(concurrent_facility_b)
    .execute(&mut *tx_b)
    .await
    .unwrap();
    let (commit_a, commit_b) = tokio::join!(tx_a.commit(), tx_b.commit());
    assert_ne!(commit_a.is_ok(), commit_b.is_ok());
    let active_assignments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM employee_facilities WHERE tenant_id = $1 AND employee_id = $2 AND deleted IS NULL",
    )
    .bind(tenant_id.get())
    .bind(concurrent_employee)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(active_assignments, 1);
}
