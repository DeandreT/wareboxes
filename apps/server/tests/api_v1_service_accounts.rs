mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{auth, routes, state::AppState};
use wareboxes_api_contract::v1::{
    ChangeServiceAccountStatusRequest, CreateServiceAccountRequest, FulfillmentOrderDestination,
    IntegrationOrderEnvelopeLineRequest, IntegrationOrderEnvelopeRequest,
    IntegrationOrderIntakeResponse, IssueServiceAccountCredentialRequest,
    IssuedServiceAccountCredentialResponse, Revision, RevokeServiceAccountCredentialRequest,
    ServiceAccountAccessRequest, ServiceAccountEventPage, ServiceAccountOptionsResponse,
    ServiceAccountPage, ServiceAccountResponse, ServiceAccountStatus,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;

fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    key: Option<&str>,
    body: &T,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
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

async fn grant(fixture: &Fixture, tenant_id: TenantId, user_id: i64, name: &str) {
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
        &format!("service-account-test-{name}-{user_id}"),
        Some("Service account lifecycle acceptance role"),
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

async fn configure_owner_mapping(
    fixture: &Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    owner_id: i64,
    source_key: &str,
    external_owner_key: &str,
) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"INSERT INTO integration_order_owner_mappings
        (tenant_id,source_key,external_inventory_owner_key,inventory_owner_id,
         revision,effective_from,configured_by_user_id,configured_at)
        SELECT $1,$2,$3,$4,1,clock.moment,$5,clock.moment
        FROM (SELECT clock_timestamp() AS moment) clock"#,
    )
    .bind(tenant_id.get())
    .bind(source_key)
    .bind(external_owner_key)
    .bind(owner_id)
    .bind(actor_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

fn external_order(key: &str) -> IntegrationOrderEnvelopeRequest {
    IntegrationOrderEnvelopeRequest {
        order_key: key.into(),
        rush: false,
        ship_by: None,
        destination: FulfillmentOrderDestination {
            recipient_name: "Integration Receiving".into(),
            company: None,
            phone: None,
            email: None,
            line1: "125 Shipping Lane".into(),
            line2: None,
            city: "Reno".into(),
            region: "NV".into(),
            postal_code: "89502".into(),
            country: "US".into(),
        },
        lines: vec![IntegrationOrderEnvelopeLineRequest {
            line_key: "1".into(),
            external_item_key: "NOT-MAPPED".into(),
            external_uom: "EA".into(),
            quantity: 1,
        }],
    }
}

#[tokio::test]
async fn service_accounts_are_scoped_non_login_identities_with_rotatable_credentials() {
    let fixture = Fixture::new().await;
    let admin = fixture.user("service-account-admin@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, admin.id).await;
    grant(&fixture, tenant_id, admin.id, "admin").await;
    grant(&fixture, tenant_id, admin.id, "orders").await;
    let facility_id = fixture.facility(tenant_id, "Service account DC").await;
    let unlinked_facility_id = fixture
        .facility(tenant_id, "Unlinked service account DC")
        .await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Service account client")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, owner_id, facility_id)
        .await;
    let hidden_owner_id = fixture
        .inventory_owner(tenant_id, "Hidden service account client")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, hidden_owner_id, facility_id)
        .await;
    configure_owner_mapping(
        &fixture,
        tenant_id,
        admin.id,
        owner_id,
        "service-api",
        "VISIBLE",
    )
    .await;
    configure_owner_mapping(
        &fixture,
        tenant_id,
        admin.id,
        hidden_owner_id,
        "service-api",
        "HIDDEN",
    )
    .await;

    let admin_token = auth::create_session(&fixture.db, admin.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let create = CreateServiceAccountRequest {
        name: "ERP order intake".into(),
        description: Some("Inbound order adapter".into()),
        access: ServiceAccountAccessRequest {
            all_facilities: false,
            facility_ids: vec![facility_id],
            all_inventory_owners: false,
            inventory_owner_ids: vec![owner_id],
            permission_names: vec!["orders".into()],
        },
    };
    let created: ServiceAccountResponse = response(
        app.clone()
            .oneshot(request(
                &admin_token,
                tenant_id,
                Method::POST,
                "/api/v1/service-accounts",
                Some("create-erp-intake"),
                &create,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(created.status, ServiceAccountStatus::Active);
    assert_eq!(created.revision.get(), 1);
    assert!(created.credentials.is_empty());

    let options: ServiceAccountOptionsResponse = response(
        app.clone()
            .oneshot(request(
                &admin_token,
                tenant_id,
                Method::GET,
                "/api/v1/service-account-options",
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert!(options.permission_names.contains(&"orders".to_owned()));
    assert!(!options.permission_names.contains(&"admin".to_owned()));
    assert!(options.can_delegate_all_facilities);
    assert!(options.can_delegate_all_inventory_owners);

    let exact_create: ServiceAccountResponse = response(
        app.clone()
            .oneshot(request(
                &admin_token,
                tenant_id,
                Method::POST,
                "/api/v1/service-accounts",
                Some("create-erp-intake"),
                &create,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(exact_create, created);

    let invalid_pair = app
        .clone()
        .oneshot(request(
            &admin_token,
            tenant_id,
            Method::POST,
            "/api/v1/service-accounts",
            Some("create-invalid-owner-facility-pair"),
            &CreateServiceAccountRequest {
                name: "Invalid pair".into(),
                description: None,
                access: ServiceAccountAccessRequest {
                    all_facilities: false,
                    facility_ids: vec![unlinked_facility_id],
                    all_inventory_owners: false,
                    inventory_owner_ids: vec![owner_id],
                    permission_names: vec!["orders".into()],
                },
            },
        ))
        .await
        .unwrap();
    assert_eq!(invalid_pair.status(), StatusCode::NOT_FOUND);

    let bearer_a = format!("wbs_sa_{}", "A".repeat(48));
    let issue_uri = format!(
        "/api/v1/service-accounts/{}/credentials",
        created.service_account_id
    );
    let issue = IssueServiceAccountCredentialRequest {
        expected_revision: Revision::new(1).unwrap(),
        label: "primary partner key".into(),
        expires_at: None,
        bearer_token: bearer_a.clone(),
    };
    let issued: IssuedServiceAccountCredentialResponse = response(
        app.clone()
            .oneshot(request(
                &admin_token,
                tenant_id,
                Method::POST,
                &issue_uri,
                Some("issue-primary-key"),
                &issue,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(issued.service_account.revision.get(), 2);
    assert_eq!(issued.credential.token_prefix, &bearer_a[..15]);
    let exact_issue: IssuedServiceAccountCredentialResponse = response(
        app.clone()
            .oneshot(request(
                &admin_token,
                tenant_id,
                Method::POST,
                &issue_uri,
                Some("issue-primary-key"),
                &issue,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(exact_issue, issued);

    let admin_db = admin_db_for(&fixture.db).await;
    let (stored_hash, principal_user_id): (String, i64) = sqlx::query_as(
        r#"SELECT credential.token_hash,account.principal_user_id
        FROM service_account_credentials credential
        JOIN service_accounts account ON account.tenant_id=credential.tenant_id
          AND account.id=credential.service_account_id
        WHERE credential.tenant_id=$1 AND credential.id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(issued.credential.credential_id)
    .fetch_one(&admin_db)
    .await
    .unwrap();
    assert_eq!(
        stored_hash,
        hex::encode(Sha256::digest(bearer_a.as_bytes()))
    );
    assert_ne!(stored_hash, bearer_a);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_credentials WHERE user_id=$1",)
            .bind(principal_user_id)
            .fetch_one(&admin_db)
            .await
            .unwrap(),
        0
    );
    let human_users: Vec<wareboxes_core::models::User> = response(
        app.clone()
            .oneshot(request(
                &admin_token,
                tenant_id,
                Method::GET,
                "/api/users?show_deleted=true",
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert!(human_users.iter().all(|user| user.id != principal_user_id));
    let login_artifact = sqlx::query("SELECT create_session_record($1,$2)")
        .bind("f".repeat(64))
        .bind(principal_user_id)
        .execute(&fixture.db)
        .await;
    assert!(login_artifact.is_err());

    let scoped_access: AccessScopeWorkspace = response(
        app.clone()
            .oneshot(request(
                &bearer_a,
                tenant_id,
                Method::GET,
                "/api/web/access",
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(scoped_access.facilities.len(), 1);
    assert_eq!(scoped_access.facilities[0].id, facility_id);
    assert_eq!(scoped_access.inventory_owners.len(), 1);
    assert_eq!(scoped_access.inventory_owners[0].id, owner_id);

    let management_denied = app
        .clone()
        .oneshot(request(
            &bearer_a,
            tenant_id,
            Method::GET,
            "/api/v1/service-accounts",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(management_denied.status(), StatusCode::FORBIDDEN);
    let wrong_tenant = app
        .clone()
        .oneshot(request(
            &bearer_a,
            TenantId::new(tenant_id.get() + 999).unwrap(),
            Method::GET,
            "/api/web/access",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(wrong_tenant.status(), StatusCode::FORBIDDEN);

    let visible_intake =
        "/api/v1/integrations/order-intake/service-api/inventory-owners/VISIBLE/orders".to_owned();
    let accepted = app
        .clone()
        .oneshot(request(
            &bearer_a,
            tenant_id,
            Method::POST,
            &visible_intake,
            Some("service-visible-1"),
            &external_order("SERVICE-100"),
        ))
        .await
        .unwrap();
    let _: IntegrationOrderIntakeResponse = response(accepted, StatusCode::ACCEPTED).await;
    let hidden_intake =
        "/api/v1/integrations/order-intake/service-api/inventory-owners/HIDDEN/orders";
    let hidden = app
        .clone()
        .oneshot(request(
            &bearer_a,
            tenant_id,
            Method::POST,
            hidden_intake,
            Some("service-hidden-1"),
            &external_order("SERVICE-200"),
        ))
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    let revoke_uri = format!(
        "/api/v1/service-accounts/{}/credentials/{}/revocations",
        created.service_account_id, issued.credential.credential_id
    );
    let revoke = RevokeServiceAccountCredentialRequest {
        expected_revision: Revision::new(2).unwrap(),
        reason: "scheduled rotation".into(),
    };
    let revoked: ServiceAccountResponse = response(
        app.clone()
            .oneshot(request(
                &admin_token,
                tenant_id,
                Method::POST,
                &revoke_uri,
                Some("revoke-primary-key"),
                &revoke,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(revoked.revision.get(), 3);
    assert!(revoked.credentials[0].revoked_at.is_some());
    let rejected = app
        .clone()
        .oneshot(request(
            &bearer_a,
            tenant_id,
            Method::GET,
            "/api/web/access",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

    let bearer_b = format!("wbs_sa_{}", "B".repeat(48));
    let issued_b: IssuedServiceAccountCredentialResponse = response(
        app.clone()
            .oneshot(request(
                &admin_token,
                tenant_id,
                Method::POST,
                &issue_uri,
                Some("issue-rotated-key"),
                &IssueServiceAccountCredentialRequest {
                    expected_revision: Revision::new(3).unwrap(),
                    label: "rotated partner key".into(),
                    expires_at: None,
                    bearer_token: bearer_b.clone(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(issued_b.service_account.revision.get(), 4);
    let status_uri = format!(
        "/api/v1/service-accounts/{}/status-changes",
        created.service_account_id
    );
    let disabled: ServiceAccountResponse = response(
        app.clone()
            .oneshot(request(
                &admin_token,
                tenant_id,
                Method::POST,
                &status_uri,
                Some("disable-erp-intake"),
                &ChangeServiceAccountStatusRequest {
                    expected_revision: Revision::new(4).unwrap(),
                    status: ServiceAccountStatus::Disabled,
                    reason: "partner access suspended".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(disabled.revision.get(), 5);
    assert_eq!(disabled.status, ServiceAccountStatus::Disabled);
    assert!(disabled
        .credentials
        .iter()
        .all(|credential| credential.revoked_at.is_some()));
    let disabled_auth = app
        .clone()
        .oneshot(request(
            &bearer_b,
            tenant_id,
            Method::GET,
            "/api/web/access",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(disabled_auth.status(), StatusCode::UNAUTHORIZED);

    let enabled: ServiceAccountResponse = response(
        app.clone()
            .oneshot(request(
                &admin_token,
                tenant_id,
                Method::POST,
                &status_uri,
                Some("enable-erp-intake"),
                &ChangeServiceAccountStatusRequest {
                    expected_revision: Revision::new(5).unwrap(),
                    status: ServiceAccountStatus::Active,
                    reason: "partner access restored".into(),
                },
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(enabled.revision.get(), 6);
    assert_eq!(enabled.status, ServiceAccountStatus::Active);
    assert!(enabled
        .credentials
        .iter()
        .all(|credential| credential.revoked_at.is_some()));
    let still_revoked = app
        .clone()
        .oneshot(request(
            &bearer_b,
            tenant_id,
            Method::GET,
            "/api/web/access",
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(still_revoked.status(), StatusCode::UNAUTHORIZED);

    let concurrent_a = IssueServiceAccountCredentialRequest {
        expected_revision: Revision::new(6).unwrap(),
        label: "concurrent A".into(),
        expires_at: None,
        bearer_token: format!("wbs_sa_{}", "C".repeat(48)),
    };
    let concurrent_b = IssueServiceAccountCredentialRequest {
        expected_revision: Revision::new(6).unwrap(),
        label: "concurrent B".into(),
        expires_at: None,
        bearer_token: format!("wbs_sa_{}", "D".repeat(48)),
    };
    let app_a = app.clone();
    let app_b = app.clone();
    let token_a = admin_token.clone();
    let token_b = admin_token.clone();
    let uri_a = issue_uri.clone();
    let uri_b = issue_uri.clone();
    let (result_a, result_b) = tokio::join!(
        async move {
            app_a
                .oneshot(request(
                    &token_a,
                    tenant_id,
                    Method::POST,
                    &uri_a,
                    Some("concurrent-credential-a"),
                    &concurrent_a,
                ))
                .await
                .unwrap()
        },
        async move {
            app_b
                .oneshot(request(
                    &token_b,
                    tenant_id,
                    Method::POST,
                    &uri_b,
                    Some("concurrent-credential-b"),
                    &concurrent_b,
                ))
                .await
                .unwrap()
        }
    );
    let statuses = [result_a.status(), result_b.status()];
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

    let page: ServiceAccountPage = response(
        app.clone()
            .oneshot(request(
                &admin_token,
                tenant_id,
                Method::GET,
                "/api/v1/service-accounts?limit=10",
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].revision.get(), 7);
    assert_eq!(page.items[0].credentials.len(), 3);

    let events_uri = format!(
        "/api/v1/service-accounts/{}/events?limit=50",
        created.service_account_id
    );
    let events: ServiceAccountEventPage = response(
        app.clone()
            .oneshot(request(
                &admin_token,
                tenant_id,
                Method::GET,
                &events_uri,
                None,
                &(),
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(events.items.len(), 7);
    let disabled_event = events
        .items
        .iter()
        .find(|event| event.action == "disabled")
        .unwrap();
    assert_eq!(disabled_event.evidence["revoked_credential_count"], 1);

    let other_admin = fixture.user("service-account-other@test.local").await;
    let other_tenant_id = tenant_for_user(&fixture.db, other_admin.id).await;
    grant(&fixture, other_tenant_id, other_admin.id, "admin").await;
    let other_token = auth::create_session(&fixture.db, other_admin.id)
        .await
        .unwrap();
    let guessed = app
        .clone()
        .oneshot(request(
            &other_token,
            other_tenant_id,
            Method::GET,
            &format!("/api/v1/service-accounts/{}", created.service_account_id),
            None,
            &(),
        ))
        .await
        .unwrap();
    assert_eq!(guessed.status(), StatusCode::NOT_FOUND);
    let mut other_tx = tenant_tx(&fixture.db, other_tenant_id).await;
    for table in [
        "service_accounts",
        "service_account_facilities",
        "service_account_inventory_owners",
        "service_account_permissions",
        "service_account_credentials",
        "service_account_events",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&mut *other_tx)
            .await
            .unwrap();
        assert_eq!(count, 0, "cross-tenant rows visible in {table}");
    }
    other_tx.commit().await.unwrap();
    let event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM service_account_events WHERE tenant_id=$1 AND service_account_id=$2",
    )
    .bind(tenant_id.get())
    .bind(created.service_account_id)
    .fetch_one(&admin_db)
    .await
    .unwrap();
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events WHERE tenant_id=$1 AND aggregate_type='service_account' AND aggregate_id=$2",
    )
    .bind(tenant_id.get())
    .bind(created.service_account_id.to_string())
    .fetch_one(&admin_db)
    .await
    .unwrap();
    assert_eq!(event_count, 7);
    assert_eq!(outbox_count, 7);
    admin_db.close().await;
}
