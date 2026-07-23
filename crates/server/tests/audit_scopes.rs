mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_core::dto::{AddAuditLocationCount, AuditLocationCountUpdate, UpdateUserAccessScope};
use wareboxes_core::models::{AuditLocationCount, AuditWave};
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::{routes, state::AppState};

async fn grant_admin(db: &db::Db, tenant_id: TenantId, user_id: i64) {
    let permission = repo::permissions::add_permission(db, tenant_id, "admin", Some("Admin"))
        .await
        .unwrap();
    let role = repo::roles::add_role(db, tenant_id, "audit-scope-admin", Some("Audit admin"))
        .await
        .unwrap();
    repo::roles::add_role_permission(db, tenant_id, role, permission)
        .await
        .unwrap();
    repo::roles::add_role_to_user(db, tenant_id, user_id, role)
        .await
        .unwrap();
}

fn request(
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

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn assign_owner_to_facility(
    db: &db::Db,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO inventory_owner_facilities
            (tenant_id, created, inventory_owner_id, facility_id)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .execute(db)
    .await
    .unwrap();
}

async fn assign_item_to_owner(
    db: &db::Db,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    item_id: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO inventory_owner_items
            (tenant_id, created, inventory_owner_id, item_id)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(inventory_owner_id)
    .bind(item_id)
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn inventory_audits_enforce_facility_and_owner_scope_for_reads_and_writes() {
    let fixture = Fixture::new().await;
    let administrator = fixture.user("audit-scope-owner@test.com").await;
    let operator = fixture.user("audit-scope-operator@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, administrator.id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(operator.id)
        .execute(&fixture.db)
        .await
        .unwrap();
    grant_admin(&fixture.db, tenant_id, operator.id).await;

    let allowed_facility = fixture.facility(tenant_id, "Allowed Audit DC").await;
    let denied_facility = fixture.facility(tenant_id, "Denied Audit DC").await;
    let allowed_owner = fixture
        .inventory_owner(tenant_id, "Allowed Audit Owner")
        .await;
    let denied_owner = fixture
        .inventory_owner(tenant_id, "Denied Audit Owner")
        .await;
    assign_owner_to_facility(&fixture.db, tenant_id, allowed_owner, allowed_facility).await;
    assign_owner_to_facility(&fixture.db, tenant_id, denied_owner, denied_facility).await;

    let allowed_location = fixture
        .location(tenant_id, allowed_facility, "AUDIT-ALLOWED")
        .await;
    let denied_location = fixture
        .location(tenant_id, denied_facility, "AUDIT-DENIED")
        .await;
    let allowed_item = fixture.item(tenant_id, "Allowed Audit Item", "each").await;
    let denied_item = fixture.item(tenant_id, "Denied Audit Item", "each").await;
    assign_item_to_owner(&fixture.db, tenant_id, allowed_owner, allowed_item).await;
    assign_item_to_owner(&fixture.db, tenant_id, denied_owner, denied_item).await;

    let allowed_batch = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        allowed_owner,
        allowed_item,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        operator.id,
        allowed_batch,
        allowed_location,
        7,
        None,
        Some("audit snapshot setup"),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE items SET packaging_unit = 'case' WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id.get())
        .bind(allowed_item)
        .execute(&fixture.db)
        .await
        .unwrap();
    let case_batch = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        allowed_owner,
        allowed_item,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    repo::inventory::receive_inventory(
        &fixture.db,
        tenant_id,
        operator.id,
        case_batch,
        allowed_location,
        2,
        None,
        Some("audit UOM isolation setup"),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE items SET packaging_unit = 'each' WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id.get())
        .bind(allowed_item)
        .execute(&fixture.db)
        .await
        .unwrap();

    let unrestricted = repo::tenants::access_for_user(&fixture.db, operator.id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    let allowed_wave = repo::audits::add_audit_wave(
        &fixture.db,
        &unrestricted,
        operator.id,
        allowed_facility,
        allowed_owner,
        "Allowed count",
        None,
    )
    .await
    .unwrap()
    .unwrap();
    let denied_wave = repo::audits::add_audit_wave(
        &fixture.db,
        &unrestricted,
        operator.id,
        denied_facility,
        denied_owner,
        "Denied count",
        None,
    )
    .await
    .unwrap()
    .unwrap();
    let allowed_count = repo::audits::add_location_count(
        &fixture.db,
        &unrestricted,
        &AddAuditLocationCount {
            audit_wave_id: allowed_wave,
            location_id: allowed_location,
            item_id: allowed_item,
            uom: "each".to_owned(),
            lot: None,
            expiration: None,
            serial: None,
            count: 9,
        },
    )
    .await
    .unwrap()
    .unwrap();
    let denied_count = repo::audits::add_location_count(
        &fixture.db,
        &unrestricted,
        &AddAuditLocationCount {
            audit_wave_id: denied_wave,
            location_id: denied_location,
            item_id: denied_item,
            uom: "each".to_owned(),
            lot: None,
            expiration: None,
            serial: None,
            count: 4,
        },
    )
    .await
    .unwrap()
    .unwrap();

    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: operator.id,
            all_facilities: false,
            facility_ids: vec![allowed_facility],
            all_inventory_owners: false,
            inventory_owner_ids: vec![allowed_owner],
        },
    )
    .await
    .unwrap();
    let restricted = repo::tenants::access_for_user(&fixture.db, operator.id, tenant_id)
        .await
        .unwrap()
        .unwrap();

    let waves = repo::audits::get_audit_waves(&fixture.db, &restricted, false)
        .await
        .unwrap();
    assert_eq!(
        waves.iter().map(|wave| wave.id).collect::<Vec<_>>(),
        vec![allowed_wave]
    );
    let counts = repo::audits::get_location_counts(&fixture.db, &restricted, allowed_wave)
        .await
        .unwrap();
    assert_eq!(counts.len(), 1);
    assert_eq!((counts[0].on_hand, counts[0].revision), (7, 1));
    assert_eq!(counts[0].approval_status.to_string(), "pending");
    assert!(
        repo::audits::get_location_counts(&fixture.db, &restricted, denied_wave)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!repo::audits::update_audit_wave(
        &fixture.db,
        &restricted,
        denied_wave,
        Some("Guessed update"),
        None,
    )
    .await
    .unwrap());
    assert!(
        !repo::audits::set_audit_wave_deleted(&fixture.db, &restricted, denied_wave, true,)
            .await
            .unwrap()
    );
    assert!(!repo::audits::update_location_count(
        &fixture.db,
        &restricted,
        &AuditLocationCountUpdate {
            audit_location_count_id: denied_count,
            expected_revision: 1,
            count: 99,
        },
    )
    .await
    .unwrap());
    assert!(!repo::audits::set_location_count_deleted(
        &fixture.db,
        &restricted,
        denied_count,
        1,
        true,
    )
    .await
    .unwrap());

    assert!(sqlx::query(
        r#"
        INSERT INTO audit_location_counts
            (tenant_id, created, audit_id, inventory_owner_id, facility_id, location_id,
             item_id, uom, on_hand, count, approval_status)
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'each', 1, 1, 'pending')
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(allowed_wave)
    .bind(allowed_owner)
    .bind(denied_facility)
    .bind(denied_location)
    .bind(allowed_item)
    .execute(&fixture.db)
    .await
    .is_err());

    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let response = app
        .clone()
        .oneshot(request(&token, tenant_id, Method::GET, "/api/audits", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let waves = response_json::<Vec<AuditWave>>(response).await;
    assert_eq!(
        waves.iter().map(|wave| wave.id).collect::<Vec<_>>(),
        vec![allowed_wave]
    );

    let response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            "/api/audits/add",
            Some(json!({
                "facility_id": denied_facility,
                "inventory_owner_id": denied_owner,
                "name": "Forbidden wave"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            "/api/audits/update",
            Some(json!({"audit_wave_id": denied_wave, "name": "Guessed"})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response_json::<bool>(response).await);

    let response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            "/api/audits/delete",
            Some(json!({"audit_wave_id": denied_wave})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response_json::<bool>(response).await);

    let response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::GET,
            &format!("/api/audits/{denied_wave}/counts"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<Vec<AuditLocationCount>>(response)
        .await
        .is_empty());

    let response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            "/api/audits/counts/add",
            Some(json!({
                "audit_wave_id": allowed_wave,
                "location_id": allowed_location,
                "item_id": allowed_item,
                "uom": "each",
                "on_hand": 999,
                "count": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            "/api/audits/counts/add",
            Some(json!({
                "audit_wave_id": denied_wave,
                "location_id": denied_location,
                "item_id": denied_item,
                "uom": "each",
                "count": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            "/api/audits/counts/update",
            Some(json!({
                "audit_location_count_id": denied_count,
                "expected_revision": 1,
                "count": 88
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response_json::<bool>(response).await);

    let response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            "/api/audits/counts/delete",
            Some(json!({"audit_location_count_id": denied_count, "expected_revision": 1})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response_json::<bool>(response).await);

    let response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            "/api/audits/counts/update",
            Some(json!({
                "audit_location_count_id": allowed_count,
                "expected_revision": 1,
                "count": 10
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<bool>(response).await);

    let response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            "/api/audits/counts/update",
            Some(json!({
                "audit_location_count_id": allowed_count,
                "expected_revision": 1,
                "count": 11
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response_json::<bool>(response).await);
    let counts = repo::audits::get_location_counts(&fixture.db, &restricted, allowed_wave)
        .await
        .unwrap();
    assert_eq!((counts[0].count, counts[0].revision), (10, 2));

    let response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            "/api/audits/counts/delete",
            Some(json!({"audit_location_count_id": allowed_count, "expected_revision": 2})),
        ))
        .await
        .unwrap();
    assert!(response_json::<bool>(response).await);

    sqlx::query(
        r#"
        UPDATE inventory_owner_facilities
        SET deleted = clock_timestamp()
        WHERE tenant_id = $1 AND inventory_owner_id = $2 AND facility_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(allowed_owner)
    .bind(allowed_facility)
    .execute(&fixture.db)
    .await
    .unwrap();

    let response = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            Method::POST,
            "/api/audits/counts/restore",
            Some(json!({"audit_location_count_id": allowed_count, "expected_revision": 3})),
        ))
        .await
        .unwrap();
    assert!(!response_json::<bool>(response).await);
    assert!(
        repo::audits::set_audit_wave_deleted(&fixture.db, &restricted, allowed_wave, true,)
            .await
            .unwrap()
    );
    assert!(
        !repo::audits::set_audit_wave_deleted(&fixture.db, &restricted, allowed_wave, false,)
            .await
            .unwrap()
    );

    sqlx::query(
        r#"
        UPDATE inventory_owner_facilities
        SET deleted = NULL
        WHERE tenant_id = $1 AND inventory_owner_id = $2 AND facility_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(allowed_owner)
    .bind(allowed_facility)
    .execute(&fixture.db)
    .await
    .unwrap();
    assert!(
        repo::audits::set_audit_wave_deleted(&fixture.db, &restricted, allowed_wave, false,)
            .await
            .unwrap()
    );
    sqlx::query(
        "UPDATE locations SET deleted = clock_timestamp() WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(allowed_location)
    .execute(&fixture.db)
    .await
    .unwrap();
    assert!(!repo::audits::set_location_count_deleted(
        &fixture.db,
        &restricted,
        allowed_count,
        3,
        false,
    )
    .await
    .unwrap());
    sqlx::query("UPDATE locations SET deleted = NULL WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id.get())
        .bind(allowed_location)
        .execute(&fixture.db)
        .await
        .unwrap();
    assert!(repo::audits::set_location_count_deleted(
        &fixture.db,
        &restricted,
        allowed_count,
        3,
        false,
    )
    .await
    .unwrap());
    assert!(!repo::audits::update_location_count(
        &fixture.db,
        &restricted,
        &AuditLocationCountUpdate {
            audit_location_count_id: allowed_count,
            expected_revision: 2,
            count: 12,
        },
    )
    .await
    .unwrap());
    assert!(
        repo::audits::set_audit_wave_deleted(&fixture.db, &restricted, allowed_wave, true,)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE items SET deleted = clock_timestamp() WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id.get())
        .bind(allowed_item)
        .execute(&fixture.db)
        .await
        .unwrap();
    assert!(
        !repo::audits::set_audit_wave_deleted(&fixture.db, &restricted, allowed_wave, false,)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE items SET deleted = NULL WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id.get())
        .bind(allowed_item)
        .execute(&fixture.db)
        .await
        .unwrap();
    assert!(
        repo::audits::set_audit_wave_deleted(&fixture.db, &restricted, allowed_wave, false,)
            .await
            .unwrap()
    );
}
