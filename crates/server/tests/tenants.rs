mod common;

use axum::body::{to_bytes, Body};
use axum::extract::FromRequestParts;
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_server::auth::{CurrentTenant, TENANT_ID_HEADER};
use wareboxes_server::routes;
use wareboxes_server::state::AppState;

fn request_parts(token: &str, tenant_id: Option<i64>) -> axum::http::request::Parts {
    let mut request = Request::builder().header(header::AUTHORIZATION, format!("Bearer {token}"));
    if let Some(tenant_id) = tenant_id {
        request = request.header(TENANT_ID_HEADER, tenant_id.to_string());
    }
    request.body(()).unwrap().into_parts().0
}

#[tokio::test]
async fn registrations_receive_distinct_default_tenants() {
    let db = setup().await;
    let first = auth::register_user(&db, "first@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let second = auth::register_user(&db, "second@test.com", "supersecret", None, None)
        .await
        .unwrap();

    let first_access = repo::tenants::list_for_user(&db, first.id).await.unwrap();
    let second_access = repo::tenants::list_for_user(&db, second.id).await.unwrap();

    assert_eq!(first_access.len(), 1);
    assert_eq!(second_access.len(), 1);
    assert!(first_access[0].is_default);
    assert!(second_access[0].is_default);
    assert_ne!(first_access[0].tenant_id, second_access[0].tenant_id);
    assert_eq!(first_access[0].user_id.get(), first.id);
    assert_eq!(second_access[0].user_id.get(), second.id);
    assert!(first_access[0].site_scope.all_facilities);
    assert!(first_access[0].owner_scope.all_inventory_owners);
    assert!(first_access[0].site_scope.facility_ids.is_empty());
    assert!(first_access[0].owner_scope.inventory_owner_ids.is_empty());
}

#[tokio::test]
async fn membership_queries_do_not_return_other_tenants() {
    let db = setup().await;
    let owner = auth::register_user(&db, "owner@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let outsider = auth::register_user(&db, "outsider@test.com", "supersecret", None, None)
        .await
        .unwrap();

    let owner_tenant = repo::tenants::list_for_user(&db, owner.id).await.unwrap()[0].tenant_id;
    let outsider_access = repo::tenants::list_for_user(&db, outsider.id)
        .await
        .unwrap();

    assert!(outsider_access
        .iter()
        .all(|access| access.tenant_id != owner_tenant));
}

#[tokio::test]
async fn selected_tenant_context_requires_an_active_membership() {
    let db = setup().await;
    let member = auth::register_user(&db, "member@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let outsider = auth::register_user(&db, "other@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let member_tenant = repo::tenants::default_for_user(&db, member.id)
        .await
        .unwrap()
        .unwrap();
    let outsider_tenant = repo::tenants::default_for_user(&db, outsider.id)
        .await
        .unwrap()
        .unwrap();
    let token = auth::create_session(&db, member.id).await.unwrap();
    let state = AppState::new(db.clone());

    let mut valid_parts = request_parts(&token, Some(member_tenant.tenant_id.get()));
    let context = CurrentTenant::from_request_parts(&mut valid_parts, &state)
        .await
        .unwrap();
    assert_eq!(context.user.id, member.id);
    assert_eq!(context.tenant.tenant_id, member_tenant.tenant_id);

    let mut cross_tenant_parts = request_parts(&token, Some(outsider_tenant.tenant_id.get()));
    let error = CurrentTenant::from_request_parts(&mut cross_tenant_parts, &state)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Core(CoreError::Forbidden)));

    let mut missing_parts = request_parts(&token, None);
    let error = CurrentTenant::from_request_parts(&mut missing_parts, &state)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Core(CoreError::BadRequest(_))));

    sqlx::query("UPDATE tenants SET status = 'suspended' WHERE id = $1")
        .bind(member_tenant.tenant_id.get())
        .execute(&db)
        .await
        .unwrap();
    let mut suspended_parts = request_parts(&token, Some(member_tenant.tenant_id.get()));
    let error = CurrentTenant::from_request_parts(&mut suspended_parts, &state)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Core(CoreError::Forbidden)));
}

#[tokio::test]
async fn tenant_context_route_rejects_cross_tenant_requests() {
    let db = setup().await;
    let member = auth::register_user(&db, "route-member@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let outsider = auth::register_user(&db, "route-other@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let member_tenant = repo::tenants::default_for_user(&db, member.id)
        .await
        .unwrap()
        .unwrap();
    let outsider_tenant = repo::tenants::default_for_user(&db, outsider.id)
        .await
        .unwrap()
        .unwrap();
    let token = auth::create_session(&db, member.id).await.unwrap();
    let app = routes::app(AppState::new(db));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/auth/context")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, member_tenant.tenant_id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
    let access: wareboxes_core::models::TenantAccess = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(access.tenant_id, member_tenant.tenant_id);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/context")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, outsider_tenant.tenant_id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn owner_and_facility_master_data_is_tenant_scoped() {
    let db = setup().await;
    let operator = auth::register_user(&db, "master-data@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let other = auth::register_user(&db, "master-data-other@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let first_tenant = tenant_for_user(&db, operator.id).await;
    let second_tenant = tenant_for_user(&db, other.id).await;

    // The same business identifiers are valid in different tenant boundaries.
    let first_inventory_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        first_tenant,
        "Shared Customer",
        "first@test.com",
    )
    .await
    .unwrap();
    let second_inventory_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        second_tenant,
        "Shared Customer",
        "second@test.com",
    )
    .await
    .unwrap();
    let first_facility = repo::facilities::add_facility(&db, first_tenant, "Shared Facility")
        .await
        .unwrap();
    let second_facility = repo::facilities::add_facility(&db, second_tenant, "Shared Facility")
        .await
        .unwrap();

    let first_inventory_owners =
        repo::inventory_owners::get_inventory_owners(&db, first_tenant, false)
            .await
            .unwrap();
    assert_eq!(first_inventory_owners.len(), 1);
    assert_eq!(first_inventory_owners[0].id, first_inventory_owner);
    assert_eq!(first_inventory_owners[0].tenant_id, first_tenant);
    assert!(!repo::inventory_owners::active_inventory_owner_exists(
        &db,
        first_tenant,
        second_inventory_owner
    )
    .await
    .unwrap());
    assert!(!repo::inventory_owners::update_inventory_owner(
        &db,
        first_tenant,
        second_inventory_owner,
        Some("Cross-tenant update"),
        None,
    )
    .await
    .unwrap());

    let first_facilities = repo::facilities::get_facilities(&db, first_tenant, false)
        .await
        .unwrap();
    assert_eq!(first_facilities.len(), 1);
    assert_eq!(first_facilities[0].id, first_facility);
    assert_eq!(first_facilities[0].tenant_id, first_tenant);
    assert!(
        !repo::facilities::active_facility_exists(&db, first_tenant, second_facility)
            .await
            .unwrap()
    );

    // Give one operator access to both tenants to prove the selected header,
    // rather than the identity alone, controls the HTTP result set.
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(second_tenant.get())
    .bind(operator.id)
    .execute(&db)
    .await
    .unwrap();
    let permission = repo::permissions::add_permission(&db, second_tenant, "admin", Some("Admin"))
        .await
        .unwrap();
    let role = repo::roles::add_role(
        &db,
        second_tenant,
        "master-data-admin",
        Some("Master data admin"),
    )
    .await
    .unwrap();
    repo::roles::add_role_permission(&db, second_tenant, role, permission)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&db, second_tenant, operator.id, role)
        .await
        .unwrap();
    let token = auth::create_session(&db, operator.id).await.unwrap();
    let app = routes::app(AppState::new(db));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/inventory-owners")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, second_tenant.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let inventory_owners: Vec<wareboxes_core::models::InventoryOwner> =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(inventory_owners.len(), 1);
    assert_eq!(inventory_owners[0].id, second_inventory_owner);
    assert_eq!(inventory_owners[0].tenant_id, second_tenant);
}

#[tokio::test]
async fn locations_are_tenant_scoped_for_repositories_and_routes() {
    let db = setup().await;
    let operator = auth::register_user(&db, "locations@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let other = auth::register_user(&db, "locations-other@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let first_tenant = tenant_for_user(&db, operator.id).await;
    let second_tenant = tenant_for_user(&db, other.id).await;
    let first_facility = repo::facilities::add_facility(&db, first_tenant, "Shared Facility")
        .await
        .unwrap();
    let second_facility = repo::facilities::add_facility(&db, second_tenant, "Shared Facility")
        .await
        .unwrap();

    let first_location = repo::locations::add_location(
        &db,
        first_tenant,
        first_facility,
        None,
        Some("SHARED-BIN"),
        Some("First Tenant Bin"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let second_location = repo::locations::add_location(
        &db,
        second_tenant,
        second_facility,
        None,
        Some("SHARED-BIN"),
        Some("Second Tenant Bin"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();

    let first_locations = repo::locations::get_locations(&db, first_tenant, false)
        .await
        .unwrap();
    assert_eq!(first_locations.len(), 1);
    assert_eq!(first_locations[0].id, first_location);
    assert_eq!(first_locations[0].tenant_id, first_tenant);
    assert!(
        !repo::locations::active_location_exists(&db, first_tenant, second_location)
            .await
            .unwrap()
    );
    assert_eq!(
        repo::locations::location_active_state(&db, first_tenant, second_location)
            .await
            .unwrap(),
        None
    );
    assert!(!repo::locations::update_location(
        &db,
        first_tenant,
        second_location,
        None,
        Some("CROSS-TENANT"),
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap());
    assert!(
        !repo::locations::set_location_deleted(&db, first_tenant, second_location, true)
            .await
            .unwrap()
    );

    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(second_tenant.get())
    .bind(operator.id)
    .execute(&db)
    .await
    .unwrap();
    let permission = repo::permissions::add_permission(&db, second_tenant, "wms", Some("WMS"))
        .await
        .unwrap();
    let role = repo::roles::add_role(&db, second_tenant, "location-wms", Some("Location WMS"))
        .await
        .unwrap();
    repo::roles::add_role_permission(&db, second_tenant, role, permission)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&db, second_tenant, operator.id, role)
        .await
        .unwrap();
    let token = auth::create_session(&db, operator.id).await.unwrap();
    let app = routes::app(AppState::new(db));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/locations")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, second_tenant.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let locations: Vec<wareboxes_core::models::Location> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].id, second_location);
    assert_eq!(locations[0].tenant_id, second_tenant);
}

#[tokio::test]
async fn item_catalog_is_tenant_scoped_for_repositories_and_routes() {
    let db = setup().await;
    let operator = auth::register_user(&db, "catalog@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let other = auth::register_user(&db, "catalog-other@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let first_tenant = tenant_for_user(&db, operator.id).await;
    let second_tenant = tenant_for_user(&db, other.id).await;

    let first_item = repo::items::add_item(
        &db,
        first_tenant,
        "Shared Catalog Item",
        None,
        "each",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let second_item = repo::items::add_item(
        &db,
        second_tenant,
        "Shared Catalog Item",
        None,
        "each",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let first_case_item = repo::items::add_item(
        &db,
        first_tenant,
        "First Case",
        None,
        "case",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let barcode = "036000291452";
    repo::items::add_barcode(&db, first_tenant, first_item, barcode, "upc-a", None)
        .await
        .unwrap();
    repo::items::add_barcode(&db, second_tenant, second_item, barcode, "upc-a", None)
        .await
        .unwrap();
    repo::items::add_sku(&db, first_tenant, first_item, "SHARED-SKU", None)
        .await
        .unwrap();
    repo::items::add_sku(&db, second_tenant, second_item, "SHARED-SKU", None)
        .await
        .unwrap();

    let first_items = repo::items::get_items(&db, first_tenant, false)
        .await
        .unwrap();
    assert_eq!(first_items.len(), 2);
    assert!(first_items
        .iter()
        .all(|item| item.tenant_id == first_tenant));
    assert!(
        !repo::items::active_item_exists(&db, first_tenant, second_item)
            .await
            .unwrap()
    );
    assert_eq!(
        repo::items::active_barcode_item_by_name(&db, first_tenant, barcode)
            .await
            .unwrap(),
        Some(first_item)
    );
    assert_eq!(
        repo::items::active_barcode_item_by_name(&db, second_tenant, barcode)
            .await
            .unwrap(),
        Some(second_item)
    );
    assert!(!repo::items::update_item(
        &db,
        first_tenant,
        second_item,
        Some("Cross-tenant item update"),
        None,
        None,
    )
    .await
    .unwrap());
    assert!(
        !repo::items::set_item_deleted(&db, first_tenant, second_item, true)
            .await
            .unwrap()
    );
    let cross_pack_link =
        repo::items::add_item_pack_link(&db, first_tenant, first_case_item, second_item, 12, None)
            .await
            .unwrap_err();
    assert!(matches!(
        cross_pack_link,
        AppError::Core(CoreError::BadRequest(_))
    ));

    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(second_tenant.get())
    .bind(operator.id)
    .execute(&db)
    .await
    .unwrap();
    let permission = repo::permissions::add_permission(&db, second_tenant, "wms", Some("WMS"))
        .await
        .unwrap();
    let role = repo::roles::add_role(&db, second_tenant, "catalog-wms", Some("Catalog WMS"))
        .await
        .unwrap();
    repo::roles::add_role_permission(&db, second_tenant, role, permission)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&db, second_tenant, operator.id, role)
        .await
        .unwrap();
    let token = auth::create_session(&db, operator.id).await.unwrap();
    let app = routes::app(AppState::new(db));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/items")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, second_tenant.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let items: Vec<wareboxes_core::models::Item> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, second_item);
    assert_eq!(items[0].tenant_id, second_tenant);
    assert_eq!(items[0].barcodes.len(), 1);
    assert_eq!(items[0].barcodes[0].tenant_id, second_tenant);
    assert_eq!(items[0].skus.len(), 1);
    assert_eq!(items[0].skus[0].tenant_id, second_tenant);
}
