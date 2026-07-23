mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::LicensePlate;
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

#[tokio::test]
async fn license_plates_are_owner_and_facility_scoped() {
    let db = setup().await;
    let administrator = auth::register_user(
        &db,
        "license-plate-scope-admin@test.com",
        "supersecret",
        None,
        None,
    )
    .await
    .unwrap();
    let operator = auth::register_user(
        &db,
        "license-plate-scope-operator@test.com",
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
    let permission = repo::permissions::add_permission(&db, tenant_id, "wms", Some("WMS"))
        .await
        .unwrap();
    let role = repo::roles::add_role(
        &db,
        tenant_id,
        "license-plate-scope-operator",
        Some("Scoped license plate operator"),
    )
    .await
    .unwrap();
    repo::roles::add_role_permission(&db, tenant_id, role, permission)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&db, tenant_id, operator.id, role)
        .await
        .unwrap();

    let allowed_facility = repo::facilities::add_facility(&db, tenant_id, "Allowed LPN DC")
        .await
        .unwrap();
    let denied_facility = repo::facilities::add_facility(&db, tenant_id, "Denied LPN DC")
        .await
        .unwrap();
    let allowed_location = repo::locations::add_location(
        &db,
        tenant_id,
        allowed_facility,
        None,
        Some("LPN-SCOPE-ALLOWED"),
        Some("Allowed LPN Location"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let allowed_destination = repo::locations::add_location(
        &db,
        tenant_id,
        allowed_facility,
        None,
        Some("LPN-SCOPE-ALLOWED-DEST"),
        Some("Allowed LPN Destination"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let denied_location = repo::locations::add_location(
        &db,
        tenant_id,
        denied_facility,
        None,
        Some("LPN-SCOPE-DENIED"),
        Some("Denied LPN Location"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let allowed_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Allowed LPN Owner",
        "allowed-lpn-owner@test.com",
    )
    .await
    .unwrap();
    let denied_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Denied LPN Owner",
        "denied-lpn-owner@test.com",
    )
    .await
    .unwrap();

    let allowed_plate = repo::license_plates::add_license_plate(
        &db,
        tenant_id,
        allowed_owner,
        allowed_facility,
        Some("LPN-SCOPE-ALLOWED"),
    )
    .await
    .unwrap();
    let denied_facility_plate = repo::license_plates::add_license_plate(
        &db,
        tenant_id,
        allowed_owner,
        denied_facility,
        Some("LPN-SCOPE-DENIED-FACILITY"),
    )
    .await
    .unwrap();
    let denied_owner_plate = repo::license_plates::add_license_plate(
        &db,
        tenant_id,
        denied_owner,
        allowed_facility,
        Some("LPN-SCOPE-DENIED-OWNER"),
    )
    .await
    .unwrap();

    let mut tx = tenant_tx(&db, tenant_id).await;
    let mismatched_location = sqlx::query(
        r#"
        INSERT INTO license_plates
            (tenant_id, inventory_owner_id, created, barcode, facility_id, location_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(tenant_id.get())
    .bind(allowed_owner)
    .bind(db::now_iso())
    .bind("LPN-SCOPE-INVALID-LOCATION")
    .bind(allowed_facility)
    .bind(denied_location)
    .execute(&mut *tx)
    .await;
    assert!(mismatched_location.is_err());
    tx.rollback().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let resolved_plate = repo::license_plates::find_or_create_license_plate_tx(
        &mut tx,
        tenant_id,
        allowed_owner,
        None,
        Some(allowed_plate),
        allowed_location,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(resolved_plate, Some(allowed_plate));

    let mut tx = db.begin().await.unwrap();
    let cross_facility_assignment = repo::license_plates::find_or_create_license_plate_tx(
        &mut tx,
        tenant_id,
        allowed_owner,
        None,
        Some(allowed_plate),
        denied_location,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        cross_facility_assignment,
        AppError::Core(CoreError::Conflict(_))
    ));
    tx.rollback().await.unwrap();

    let cross_facility_move = repo::license_plates::move_license_plate(
        &db,
        tenant_id,
        administrator.id,
        allowed_plate,
        denied_location,
        Some("invalid cross-facility move"),
        Some("lpn-scope-cross-facility"),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        cross_facility_move,
        AppError::Core(CoreError::Conflict(_))
    ));
    assert!(repo::inventory::get_transactions(&db, tenant_id)
        .await
        .unwrap()
        .is_empty());
    let plate =
        repo::license_plates::get_license_plate_by_barcode(&db, tenant_id, "LPN-SCOPE-ALLOWED")
            .await
            .unwrap()
            .unwrap();
    assert_eq!(plate.facility_id, allowed_facility);
    assert_eq!(plate.location_id, Some(allowed_location));

    repo::tenants::update_user_access_scope(
        &db,
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
    let token = auth::create_session(&db, operator.id).await.unwrap();
    let app = routes::app(AppState::new(db.clone()));

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/license-plates",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let plates = response_json::<Vec<LicensePlate>>(response).await;
    assert_eq!(plates.len(), 1);
    assert_eq!(plates[0].id, allowed_plate);
    assert_eq!(plates[0].facility_id, allowed_facility);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::GET,
        "/api/license-plates/barcode/LPN-SCOPE-ALLOWED",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Option<LicensePlate>>(response)
            .await
            .unwrap()
            .id,
        allowed_plate
    );

    for barcode in ["LPN-SCOPE-DENIED-FACILITY", "LPN-SCOPE-DENIED-OWNER"] {
        let response = send_api(
            &app,
            &token,
            tenant_id,
            Method::GET,
            &format!("/api/license-plates/barcode/{barcode}"),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_json::<Option<LicensePlate>>(response)
            .await
            .is_none());
    }

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/license-plates/add",
        Some(json!({
            "inventory_owner_id": denied_owner,
            "facility_id": allowed_facility,
            "barcode": "LPN-SCOPE-ADD-DENIED-OWNER"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/license-plates/add",
        Some(json!({
            "inventory_owner_id": allowed_owner,
            "facility_id": denied_facility,
            "barcode": "LPN-SCOPE-ADD-DENIED-FACILITY"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/license-plates/add",
        Some(json!({
            "inventory_owner_id": allowed_owner,
            "facility_id": allowed_facility,
            "barcode": "LPN-SCOPE-ADDED"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let added_plate = response_json::<i64>(response).await;
    let added =
        repo::license_plates::get_license_plate_by_barcode(&db, tenant_id, "LPN-SCOPE-ADDED")
            .await
            .unwrap()
            .unwrap();
    assert_eq!(added.id, added_plate);
    assert_eq!(added.facility_id, allowed_facility);

    for hidden_plate in [denied_facility_plate, denied_owner_plate] {
        let response = send_api(
            &app,
            &token,
            tenant_id,
            Method::POST,
            "/api/license-plates/update",
            Some(json!({
                "license_plate_id": hidden_plate,
                "barcode": format!("HIDDEN-{hidden_plate}")
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response_json::<bool>(response).await);

        let response = send_api(
            &app,
            &token,
            tenant_id,
            Method::POST,
            "/api/license-plates/delete",
            Some(json!({"license_plate_id": hidden_plate})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response_json::<bool>(response).await);
    }

    assert!(repo::license_plates::set_license_plate_deleted(
        &db,
        tenant_id,
        denied_owner_plate,
        true,
    )
    .await
    .unwrap());
    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/license-plates/restore",
        Some(json!({"license_plate_id": denied_owner_plate})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response_json::<bool>(response).await);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/license-plates/move",
        Some(json!({
            "license_plate_id": denied_facility_plate,
            "to_location_id": allowed_destination,
            "idempotency_key": "lpn-scope-hidden-source"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/license-plates/move",
        Some(json!({
            "license_plate_id": allowed_plate,
            "to_location_id": denied_location,
            "idempotency_key": "lpn-scope-hidden-destination"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/license-plates/move",
        Some(json!({
            "license_plate_id": allowed_plate,
            "to_location_id": allowed_destination,
            "idempotency_key": "lpn-scope-empty-allowed-move"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/license-plates/update",
        Some(json!({
            "license_plate_id": allowed_plate,
            "barcode": "LPN-SCOPE-ALLOWED-UPDATED"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<bool>(response).await);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/license-plates/delete",
        Some(json!({"license_plate_id": allowed_plate})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<bool>(response).await);

    let response = send_api(
        &app,
        &token,
        tenant_id,
        Method::POST,
        "/api/license-plates/restore",
        Some(json!({"license_plate_id": allowed_plate})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_json::<bool>(response).await);

    let hidden_facility_plate = repo::license_plates::get_license_plate_by_barcode(
        &db,
        tenant_id,
        "LPN-SCOPE-DENIED-FACILITY",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(hidden_facility_plate.id, denied_facility_plate);
    let hidden_owner_plate = repo::license_plates::get_license_plates(&db, tenant_id, true)
        .await
        .unwrap()
        .into_iter()
        .find(|plate| plate.id == denied_owner_plate)
        .unwrap();
    assert!(hidden_owner_plate.deleted.is_some());
}

#[tokio::test]
async fn license_plates_require_a_transaction_local_tenant_context() {
    let fixture = Fixture::new().await;
    let user_a = fixture.user("license-plate-rls-a@test.com").await;
    let user_b = fixture.user("license-plate-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let owner_a = fixture
        .inventory_owner(tenant_a, "License Plate RLS Owner A")
        .await;
    let owner_b = fixture
        .inventory_owner(tenant_b, "License Plate RLS Owner B")
        .await;
    let facility_a = fixture
        .facility(tenant_a, "License Plate RLS Facility A")
        .await;
    let facility_b = fixture
        .facility(tenant_b, "License Plate RLS Facility B")
        .await;
    let location_b = fixture
        .location(tenant_b, facility_b, "LICENSE-PLATE-RLS-BIN-B")
        .await;

    let shared_plate_a = repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_a,
        owner_a,
        facility_a,
        Some("LICENSE-PLATE-RLS-SHARED"),
    )
    .await
    .unwrap();
    let private_plate_a = repo::license_plates::add_license_plate(
        &fixture.db,
        tenant_a,
        owner_a,
        facility_a,
        Some("LICENSE-PLATE-RLS-A-PRIVATE"),
    )
    .await
    .unwrap();

    let mut tx = fixture.db.begin().await.unwrap();
    let shared_plate_b = repo::license_plates::find_or_create_license_plate_tx(
        &mut tx,
        tenant_b,
        owner_b,
        Some("LICENSE-PLATE-RLS-SHARED"),
        None,
        location_b,
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    assert_ne!(shared_plate_a, shared_plate_b);

    let tenant_a_shared = repo::license_plates::get_license_plate_by_barcode(
        &fixture.db,
        tenant_a,
        "LICENSE-PLATE-RLS-SHARED",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(tenant_a_shared.id, shared_plate_a);
    let tenant_b_shared = repo::license_plates::get_license_plate_by_barcode(
        &fixture.db,
        tenant_b,
        "LICENSE-PLATE-RLS-SHARED",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(tenant_b_shared.id, shared_plate_b);
    assert!(repo::license_plates::get_license_plate_by_barcode(
        &fixture.db,
        tenant_b,
        "LICENSE-PLATE-RLS-A-PRIVATE",
    )
    .await
    .unwrap()
    .is_none());
    assert!(!repo::license_plates::update_license_plate(
        &fixture.db,
        tenant_b,
        private_plate_a,
        Some("LICENSE-PLATE-RLS-CROSS-TENANT"),
    )
    .await
    .unwrap());
    assert!(!repo::license_plates::set_license_plate_deleted(
        &fixture.db,
        tenant_b,
        private_plate_a,
        true,
    )
    .await
    .unwrap());

    let unbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM license_plates")
        .fetch_one(&fixture.db)
        .await
        .unwrap();
    assert_eq!(unbound_count, 0);
    let unbound_updates =
        sqlx::query("UPDATE license_plates SET barcode = 'UNBOUND' WHERE id = $1")
            .bind(private_plate_a)
            .execute(&fixture.db)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(unbound_updates, 0);
    let unbound_deletes = sqlx::query("DELETE FROM license_plates WHERE id = $1")
        .bind(private_plate_a)
        .execute(&fixture.db)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(unbound_deletes, 0);
    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(sqlx::query(
        r#"
        INSERT INTO license_plates
            (tenant_id, inventory_owner_id, created, barcode, facility_id)
        VALUES ($1, $2, $3, 'LICENSE-PLATE-RLS-UNBOUND', $4)
        "#,
    )
    .bind(tenant_a.get())
    .bind(owner_a)
    .bind(db::now_iso())
    .bind(facility_a)
    .execute(&mut *unbound_tx)
    .await
    .is_err());
    unbound_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let guessed_id: Option<i64> = sqlx::query_scalar("SELECT id FROM license_plates WHERE id = $1")
        .bind(private_plate_a)
        .fetch_optional(&mut *tenant_b_tx)
        .await
        .unwrap();
    assert_eq!(guessed_id, None);
    let guessed_barcode: Option<i64> =
        sqlx::query_scalar("SELECT id FROM license_plates WHERE barcode = $1")
            .bind("LICENSE-PLATE-RLS-A-PRIVATE")
            .fetch_optional(&mut *tenant_b_tx)
            .await
            .unwrap();
    assert_eq!(guessed_barcode, None);
    let cross_tenant_updates =
        sqlx::query("UPDATE license_plates SET barcode = 'CROSS-TENANT' WHERE id = $1")
            .bind(private_plate_a)
            .execute(&mut *tenant_b_tx)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(cross_tenant_updates, 0);
    let cross_tenant_deletes = sqlx::query("DELETE FROM license_plates WHERE id = $1")
        .bind(private_plate_a)
        .execute(&mut *tenant_b_tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(cross_tenant_deletes, 0);
    assert!(sqlx::query(
        r#"
        INSERT INTO license_plates
            (tenant_id, inventory_owner_id, created, barcode, facility_id)
        VALUES ($1, $2, $3, 'LICENSE-PLATE-RLS-CROSS-INSERT', $4)
        "#,
    )
    .bind(tenant_a.get())
    .bind(owner_a)
    .bind(db::now_iso())
    .bind(facility_a)
    .execute(&mut *tenant_b_tx)
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();

    let tenant_a_private = repo::license_plates::get_license_plate_by_barcode(
        &fixture.db,
        tenant_a,
        "LICENSE-PLATE-RLS-A-PRIVATE",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(tenant_a_private.id, private_plate_a);
    assert!(tenant_a_private.deleted.is_none());
}
