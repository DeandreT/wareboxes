mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use sqlx::Row;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    ConfigureFacilityShippingOriginRequest, ConfigureFacilityShippingOriginResponse, ErrorReason,
    ErrorResponse, Revision,
};
use wareboxes_application::facility_shipping_origin::FACILITY_SHIPPING_ORIGIN_CONFIGURE_OPERATION;
use wareboxes_core::dto::UpdateUserAccessScope;

fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    facility_id: i64,
    key: Option<&str>,
    body: &T,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/api/v1/facilities/{facility_id}/shipping-origin-configurations"
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

fn origin(revision: i64, line1: &str) -> ConfigureFacilityShippingOriginRequest {
    ConfigureFacilityShippingOriginRequest {
        expected_revision: Revision::new(revision).unwrap(),
        name: Some("West shipping office".into()),
        company: Some("Wareboxes Fulfillment".into()),
        line1: line1.into(),
        line2: Some("Dock 20".into()),
        city: "Reno".into(),
        state: Some("NV".into()),
        postal_code: "89502".into(),
        country: "US".into(),
        phone: Some("+1 775 555 0100".into()),
        email: Some("shipping@example.com".into()),
    }
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
        Some("Facility configuration administrator"),
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

#[tokio::test]
async fn configuration_is_exactly_replay_safe_atomic_audited_and_database_guarded() {
    let fixture = Fixture::new().await;
    let administrator = fixture.user("facility-origin-admin@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, administrator.id).await;
    grant_admin(
        &fixture.db,
        tenant_id,
        administrator.id,
        "facility-origin-admin",
    )
    .await;
    let facility_id = fixture.facility(tenant_id, "Reno DC").await;
    let token = auth::create_session(&fixture.db, administrator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let command = origin(1, "100 Distribution Way");

    let missing_key = app
        .clone()
        .oneshot(request(&token, tenant_id, facility_id, None, &command))
        .await
        .unwrap();
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json::<ErrorResponse>(missing_key).await.reason,
        ErrorReason::IdempotencyKeyRequired
    );

    let first = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            facility_id,
            Some("configure-origin"),
            &command,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: ConfigureFacilityShippingOriginResponse = response_json(first).await;
    assert_eq!(first.facility_id, facility_id);
    assert_eq!(first.revision.get(), 2);
    assert_eq!(first.origin.line1, "100 Distribution Way");
    assert_eq!(first.configured_by, administrator.id);

    let replay = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            facility_id,
            Some("configure-origin"),
            &command,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<ConfigureFacilityShippingOriginResponse>(replay).await,
        first
    );

    let reused = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            facility_id,
            Some("configure-origin"),
            &origin(1, "101 Distribution Way"),
        ))
        .await
        .unwrap();
    assert_eq!(reused.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(reused).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let stale = app
        .clone()
        .oneshot(request(
            &token,
            tenant_id,
            facility_id,
            Some("stale-origin"),
            &command,
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let state = sqlx::query(
        r#"
        SELECT facility.address_id, facility.revision,
               address.name, address.company, address.line1, address.city,
               address.postal_code, address.country,
               (SELECT COUNT(*) FROM facility_shipping_origin_configurations
                WHERE tenant_id = $1 AND facility_id = $2) AS audit_count,
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE tenant_id = $1 AND operation = $3) AS command_count,
               (SELECT COUNT(*) FROM outbox_events
                WHERE tenant_id = $1 AND aggregate_type = 'facility'
                  AND aggregate_id = $2::TEXT
                  AND event_type = 'facility.shipping_origin.configured') AS event_count
        FROM facilities facility
        INNER JOIN addresses address
            ON address.tenant_id = facility.tenant_id
           AND address.id = facility.address_id
        WHERE facility.tenant_id = $1 AND facility.id = $2
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(FACILITY_SHIPPING_ORIGIN_CONFIGURE_OPERATION)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        state.try_get::<i64, _>("address_id").unwrap(),
        first.address_id
    );
    assert_eq!(state.try_get::<i64, _>("revision").unwrap(), 2);
    assert_eq!(
        state
            .try_get::<Option<String>, _>("name")
            .unwrap()
            .as_deref(),
        Some("West shipping office")
    );
    assert_eq!(
        state
            .try_get::<Option<String>, _>("company")
            .unwrap()
            .as_deref(),
        Some("Wareboxes Fulfillment")
    );
    assert_eq!(
        state.try_get::<String, _>("line1").unwrap(),
        "100 Distribution Way"
    );
    assert_eq!(
        state
            .try_get::<Option<String>, _>("city")
            .unwrap()
            .as_deref(),
        Some("Reno")
    );
    assert_eq!(
        state
            .try_get::<Option<String>, _>("postal_code")
            .unwrap()
            .as_deref(),
        Some("89502")
    );
    assert_eq!(state.try_get::<String, _>("country").unwrap(), "US");
    assert_eq!(state.try_get::<i64, _>("audit_count").unwrap(), 1);
    assert_eq!(state.try_get::<i64, _>("command_count").unwrap(), 1);
    assert_eq!(state.try_get::<i64, _>("event_count").unwrap(), 1);
    tx.commit().await.unwrap();

    let column_privileges: (bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_column_privilege(current_user, 'public.facilities', 'address_id', 'UPDATE'),
               has_column_privilege(current_user, 'public.facilities', 'revision', 'UPDATE'),
               has_column_privilege(current_user, 'public.facilities', 'name', 'UPDATE')
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(column_privileges, (true, true, false));

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    assert!(
        sqlx::query("UPDATE addresses SET line1 = line1 WHERE id = $1")
            .bind(first.address_id)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    assert!(
        sqlx::query("UPDATE facilities SET name = name WHERE id = $1")
            .bind(facility_id)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let bypass_address_id = repo::address::insert_address_tx(
        &mut tx,
        tenant_id,
        repo::address::NewAddress {
            name: Some("Bypass origin"),
            company: None,
            line1: "300 Distribution Way",
            line2: None,
            city: Some("Reno"),
            state: Some("NV"),
            postal_code: Some("89502"),
            country: "US",
            phone: None,
            email: None,
        },
    )
    .await
    .unwrap();
    let bypass_update = sqlx::query(
        "UPDATE facilities SET address_id = $1, revision = 3 WHERE tenant_id = $2 AND id = $3",
    )
    .bind(bypass_address_id)
    .bind(tenant_id.get())
    .bind(facility_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(bypass_update.rows_affected(), 1);
    assert!(tx.commit().await.is_err());

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let protected_state: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT address_id, revision,
               (SELECT COUNT(*) FROM addresses WHERE id = $1)
        FROM facilities
        WHERE tenant_id = $2 AND id = $3
        "#,
    )
    .bind(bypass_address_id)
    .bind(tenant_id.get())
    .bind(facility_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(protected_state, (first.address_id, 2, 0));

    let audit_privileges: (bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_table_privilege(current_user, 'public.facility_shipping_origin_configurations', 'SELECT'),
               has_table_privilege(current_user, 'public.facility_shipping_origin_configurations', 'INSERT'),
               has_table_privilege(current_user, 'public.facility_shipping_origin_configurations', 'UPDATE'),
               has_table_privilege(current_user, 'public.facility_shipping_origin_configurations', 'DELETE')
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(audit_privileges, (true, true, false, false));
}

#[tokio::test]
async fn configuration_is_admin_only_scope_concealed_and_validated_without_effects() {
    let fixture = Fixture::new().await;
    let administrator = fixture.user("facility-origin-scope-admin@test.local").await;
    let operator = fixture.user("facility-origin-operator@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, administrator.id).await;
    grant_admin(
        &fixture.db,
        tenant_id,
        administrator.id,
        "facility-origin-scope-admin",
    )
    .await;
    let visible_facility = fixture.facility(tenant_id, "Visible DC").await;
    let hidden_facility = fixture.facility(tenant_id, "Hidden DC").await;

    let mut membership_tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_id.get())
        .bind(operator.id)
        .execute(&mut *membership_tx)
        .await
        .unwrap();
    membership_tx.commit().await.unwrap();
    repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id: administrator.id,
            all_facilities: false,
            facility_ids: vec![visible_facility],
            all_inventory_owners: true,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap();

    let administrator_token = auth::create_session(&fixture.db, administrator.id)
        .await
        .unwrap();
    let operator_token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let command = origin(1, "100 Distribution Way");

    let forbidden = app
        .clone()
        .oneshot(request(
            &operator_token,
            tenant_id,
            visible_facility,
            Some("operator-origin"),
            &command,
        ))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    for (facility_id, key) in [
        (hidden_facility, "hidden-origin"),
        (i64::MAX, "guessed-origin"),
    ] {
        let concealed = app
            .clone()
            .oneshot(request(
                &administrator_token,
                tenant_id,
                facility_id,
                Some(key),
                &command,
            ))
            .await
            .unwrap();
        assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
    }

    let invalid = ConfigureFacilityShippingOriginRequest {
        name: None,
        company: None,
        ..command
    };
    let invalid_response = app
        .clone()
        .oneshot(request(
            &administrator_token,
            tenant_id,
            visible_facility,
            Some("invalid-origin"),
            &invalid,
        ))
        .await
        .unwrap();
    assert_eq!(invalid_response.status(), StatusCode::BAD_REQUEST);

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM facility_shipping_origin_configurations),
               (SELECT COUNT(*) FROM outbox_events
                WHERE event_type = 'facility.shipping_origin.configured'),
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE operation = $1),
               (SELECT COUNT(*) FROM facilities WHERE address_id IS NOT NULL)
        "#,
    )
    .bind(FACILITY_SHIPPING_ORIGIN_CONFIGURE_OPERATION)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (0, 0, 0, 0));
}

#[tokio::test]
async fn concurrent_configuration_accepts_one_revision_and_rolls_back_the_loser() {
    let fixture = Fixture::new().await;
    let administrator = fixture.user("facility-origin-race-admin@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, administrator.id).await;
    grant_admin(
        &fixture.db,
        tenant_id,
        administrator.id,
        "facility-origin-race-admin",
    )
    .await;
    let facility_id = fixture.facility(tenant_id, "Race DC").await;
    let token = auth::create_session(&fixture.db, administrator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));

    let first = app.clone().oneshot(request(
        &token,
        tenant_id,
        facility_id,
        Some("origin-race-a"),
        &origin(1, "100 Distribution Way"),
    ));
    let second = app.clone().oneshot(request(
        &token,
        tenant_id,
        facility_id,
        Some("origin-race-b"),
        &origin(1, "200 Distribution Way"),
    ));
    let (first, second) = tokio::join!(first, second);
    let statuses = [first.unwrap().status(), second.unwrap().status()];
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

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let effects: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT revision FROM facilities WHERE id = $1),
               (SELECT COUNT(*) FROM addresses),
               (SELECT COUNT(*) FROM facility_shipping_origin_configurations),
               (SELECT COUNT(*) FROM command_idempotency_records
                WHERE operation = $2)
        "#,
    )
    .bind(facility_id)
    .bind(FACILITY_SHIPPING_ORIGIN_CONFIGURE_OPERATION)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (2, 1, 1, 1));
}
