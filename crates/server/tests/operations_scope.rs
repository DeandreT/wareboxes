mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_core::models::{AuditLocationCount, AuditWave, Employee};
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::{routes, state::AppState};

async fn grant_admin(db: &db::Db, tenant_id: TenantId, user_id: i64, role_name: &str) {
    let permission = repo::permissions::add_permission(db, tenant_id, "admin", Some("Admin"))
        .await
        .unwrap();
    let role = repo::roles::add_role(db, tenant_id, role_name, Some("Operations admin"))
        .await
        .unwrap();
    assert!(
        repo::roles::add_role_permission(db, tenant_id, role, permission)
            .await
            .unwrap()
    );
    assert!(repo::roles::add_role_to_user(db, tenant_id, user_id, role)
        .await
        .unwrap());
}

#[tokio::test]
async fn workforce_and_inventory_audits_are_tenant_isolated() {
    let fixture = Fixture::new().await;
    let operator = fixture.user("operations-scope@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, operator.id).await;
    let tenant_b_user = fixture.user("operations-scope-b@test.com").await;
    let tenant_b = tenant_for_user(&fixture.db, tenant_b_user.id).await;
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(tenant_b.get())
    .bind(operator.id)
    .execute(&fixture.db)
    .await
    .unwrap();
    grant_admin(&fixture.db, tenant_a, operator.id, "operations-admin").await;
    grant_admin(&fixture.db, tenant_b, operator.id, "operations-admin").await;

    let employee_a = repo::employees::add_employee(
        &fixture.db,
        tenant_a,
        "Alex",
        "A",
        "Counter",
        "hourly",
        Some("alex@example.test"),
        None,
        db::now_iso(),
    )
    .await
    .unwrap();
    let employee_b = repo::employees::add_employee(
        &fixture.db,
        tenant_b,
        "Blair",
        "B",
        "Lead",
        "salary",
        Some("blair@example.test"),
        None,
        db::now_iso(),
    )
    .await
    .unwrap();
    assert_eq!(
        repo::employees::get_employees(&fixture.db, tenant_a, false)
            .await
            .unwrap()
            .into_iter()
            .map(|employee| (employee.id, employee.tenant_id))
            .collect::<Vec<_>>(),
        vec![(employee_a, tenant_a)]
    );
    assert!(!repo::employees::update_employee(
        &fixture.db,
        tenant_b,
        employee_a,
        Some("Cross tenant"),
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap());
    assert!(
        !repo::employees::set_employee_deleted(&fixture.db, tenant_b, employee_a, true)
            .await
            .unwrap()
    );

    let audit_a =
        repo::audits::add_audit_wave(&fixture.db, tenant_a, operator.id, "Tenant A count", None)
            .await
            .unwrap();
    let audit_b =
        repo::audits::add_audit_wave(&fixture.db, tenant_b, operator.id, "Tenant B count", None)
            .await
            .unwrap();
    assert!(!repo::audits::update_audit_wave(
        &fixture.db,
        tenant_b,
        audit_a,
        Some("Cross tenant"),
        None,
    )
    .await
    .unwrap());

    let owner_a = fixture.inventory_owner(tenant_a, "Count Owner A").await;
    let facility_a = fixture.facility(tenant_a, "Count Facility A").await;
    let location_a = fixture.location(tenant_a, facility_a, "COUNT-A").await;
    let item_a = fixture.item(tenant_a, "Count Item A", "each").await;
    sqlx::query(
        "INSERT INTO inventory_owner_items (tenant_id, created, inventory_owner_id, item_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant_a.get())
    .bind(db::now_iso())
    .bind(owner_a)
    .bind(item_a)
    .execute(&fixture.db)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO audit_location_counts (
            tenant_id, created, audit_id, inventory_owner_id, location_id, item_id,
            on_hand, count, approval_status
        )
        VALUES ($1, $2, $3, $4, $5, $6, 7, 6, 'pending')
        "#,
    )
    .bind(tenant_a.get())
    .bind(db::now_iso())
    .bind(audit_a)
    .bind(owner_a)
    .bind(location_a)
    .bind(item_a)
    .execute(&fixture.db)
    .await
    .unwrap();
    let counts = repo::audits::get_location_counts(&fixture.db, tenant_a, audit_a)
        .await
        .unwrap();
    assert_eq!(counts.len(), 1);
    assert_eq!(counts[0].tenant_id, tenant_a);
    assert_eq!(counts[0].inventory_owner_id, owner_a);
    assert!(
        repo::audits::get_location_counts(&fixture.db, tenant_b, audit_a)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(sqlx::query(
        "INSERT INTO audit_wave_items (tenant_id, created, item_id, audit_wave_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant_b.get())
    .bind(db::now_iso())
    .bind(item_a)
    .bind(audit_b)
    .execute(&fixture.db)
    .await
    .is_err());

    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let employees_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/employees")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_b.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(employees_response.status(), StatusCode::OK);
    let employees = serde_json::from_slice::<Vec<Employee>>(
        &to_bytes(employees_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(employees.len(), 1);
    assert_eq!(
        (employees[0].id, employees[0].tenant_id),
        (employee_b, tenant_b)
    );

    let audits_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/audits")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_b.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audits_response.status(), StatusCode::OK);
    let audits = serde_json::from_slice::<Vec<AuditWave>>(
        &to_bytes(audits_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(audits.len(), 1);
    assert_eq!((audits[0].id, audits[0].tenant_id), (audit_b, tenant_b));

    let count_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/audits/{audit_a}/counts"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_b.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(count_response.status(), StatusCode::OK);
    let counts = serde_json::from_slice::<Vec<AuditLocationCount>>(
        &to_bytes(count_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(counts.is_empty());
}
