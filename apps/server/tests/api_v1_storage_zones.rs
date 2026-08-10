mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{StorageZonePage, StorageZoneResponse};
use wareboxes_core::models::Location;

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    user_id: i64,
    token: String,
    app: axum::Router,
    facility_id: i64,
    other_facility_id: i64,
    pick_locations: [i64; 3],
    reserve_location: i64,
}

impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let user = fixture.wms_user("storage-zone@test.local").await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        grant_supervisor(&fixture, tenant_id, user.id).await;
        let facility_id = fixture.facility(tenant_id, "Storage Zone Facility").await;
        let other_facility_id = fixture.facility(tenant_id, "Other Facility").await;
        let pick_locations = [
            fixture.location(tenant_id, facility_id, "PICK-01").await,
            fixture.location(tenant_id, facility_id, "PICK-02").await,
            fixture.location(tenant_id, facility_id, "PICK-03").await,
        ];
        let reserve_location = wareboxes_persistence_postgres::locations::add_location(
            &fixture.db,
            tenant_id,
            facility_id,
            None,
            Some("RESERVE-01"),
            Some("Reserve 01"),
            "reserve",
            true,
            false,
            false,
        )
        .await
        .unwrap();
        let token = wareboxes_api::auth::create_session(&fixture.db, user.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            tenant_id,
            user_id: user.id,
            token,
            app,
            facility_id,
            other_facility_id,
            pick_locations,
            reserve_location,
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(TENANT_ID_HEADER, self.tenant_id.to_string());
        if let Some(key) = key {
            request = request.header(IDEMPOTENCY_KEY_HEADER, key);
        }
        let body = match body {
            Some(body) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        self.app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }

    async fn locations(&self) -> Vec<Location> {
        let response = self
            .send(Method::GET, "/api/locations?show_deleted=true", None, None)
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        json_response(response).await
    }
}

async fn json_response<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "failed to decode {status} as {}: {error}; body={}",
            std::any::type_name::<T>(),
            String::from_utf8_lossy(&bytes)
        )
    })
}

async fn grant_supervisor(fixture: &Fixture, tenant_id: TenantId, user_id: i64) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        &fixture.db,
        tenant_id,
        "wms_supervisor",
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            "wms_supervisor",
            Some("WMS supervisor"),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        "storage-zone-supervisor",
        Some("Storage zone supervisor test role"),
    )
    .await
    .unwrap();
    assert!(wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap());
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        user_id,
        role,
    )
    .await
    .unwrap());
}

fn configure_body(
    facility_id: i64,
    code: &str,
    purpose: &str,
    travel_sequence: u32,
    location_ids: &[i64],
    expected_revision: Option<i64>,
) -> Value {
    json!({
        "facility_id": facility_id,
        "code": code,
        "name": format!("{code} zone"),
        "purpose": purpose,
        "travel_sequence": travel_sequence,
        "location_ids": location_ids,
        "expected_revision": expected_revision
    })
}

#[tokio::test]
async fn configure_replace_retire_and_cursor_history_are_exact() {
    let rig = Rig::new().await;
    let body = configure_body(
        rig.facility_id,
        "pick-a",
        "pick",
        20,
        &rig.pick_locations[..2],
        None,
    );
    let response = rig
        .send(
            Method::POST,
            "/api/v1/storage-zones",
            Some("zone-create"),
            Some(body.clone()),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let created: StorageZoneResponse = json_response(response).await;
    assert_eq!(created.code, "PICK-A");
    assert_eq!(created.revision.get(), 1);
    assert_eq!(created.locations.len(), 2);
    let projected = rig.locations().await;
    let first_pick = projected
        .iter()
        .find(|location| location.id == rig.pick_locations[0])
        .unwrap();
    assert_eq!(first_pick.storage_zone_id, Some(created.storage_zone_id));
    assert_eq!(first_pick.storage_zone_code.as_deref(), Some("PICK-A"));
    assert_eq!(first_pick.storage_zone_purpose.as_deref(), Some("pick"));
    assert_eq!(first_pick.storage_zone_travel_sequence, Some(20));
    let replay: StorageZoneResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/storage-zones",
            Some("zone-create"),
            Some(body),
        )
        .await,
    )
    .await;
    assert_eq!(replay, created);

    let replacement: StorageZoneResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/storage-zones",
            Some("zone-replace"),
            Some(configure_body(
                rig.facility_id,
                "PICK-A",
                "pick",
                10,
                &rig.pick_locations[1..],
                Some(1),
            )),
        )
        .await,
    )
    .await;
    assert_eq!(replacement.revision.get(), 2);
    assert_ne!(replacement.storage_zone_id, created.storage_zone_id);
    assert_eq!(replacement.locations[0].location_id, rig.pick_locations[1]);

    let reserve: StorageZoneResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/storage-zones",
            Some("zone-reserve"),
            Some(configure_body(
                rig.facility_id,
                "RES-A",
                "reserve",
                30,
                &[rig.reserve_location],
                None,
            )),
        )
        .await,
    )
    .await;
    let first: StorageZonePage = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/storage-zones?facility_id={}&limit=1",
                rig.facility_id
            ),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].storage_zone_id, replacement.storage_zone_id);
    let cursor = first.next_cursor.unwrap();
    let second: StorageZonePage = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/storage-zones?facility_id={}&limit=1&cursor={}",
                rig.facility_id,
                cursor.as_str()
            ),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(second.items[0].storage_zone_id, reserve.storage_zone_id);
    let mismatched = rig
        .send(
            Method::GET,
            &format!(
                "/api/v1/storage-zones?facility_id={}&purpose=pick&limit=1&cursor={}",
                rig.facility_id,
                cursor.as_str()
            ),
            None,
            None,
        )
        .await;
    assert_eq!(mismatched.status(), StatusCode::BAD_REQUEST);

    let retired: StorageZoneResponse = json_response(
        rig.send(
            Method::POST,
            &format!(
                "/api/v1/storage-zones/{}/retirements",
                replacement.storage_zone_id
            ),
            Some("zone-retire"),
            Some(json!({"expected_revision": 2})),
        )
        .await,
    )
    .await;
    assert_eq!(
        retired.status,
        wareboxes_api_contract::v1::StorageZoneStatus::Retired
    );
    let projected = rig.locations().await;
    for location_id in rig.pick_locations {
        let location = projected
            .iter()
            .find(|location| location.id == location_id)
            .unwrap();
        assert_eq!(location.storage_zone_id, None);
        assert_eq!(location.storage_zone_code, None);
    }
    let history: StorageZonePage = json_response(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/storage-zones?facility_id={}&status=retired&purpose=pick",
                rig.facility_id
            ),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(history.items.len(), 2);
}

#[tokio::test]
async fn incompatible_duplicate_and_direct_location_mutation_fail_closed() {
    let rig = Rig::new().await;
    let incompatible = rig
        .send(
            Method::POST,
            "/api/v1/storage-zones",
            Some("zone-incompatible"),
            Some(configure_body(
                rig.facility_id,
                "RES-BAD",
                "reserve",
                1,
                &[rig.pick_locations[0]],
                None,
            )),
        )
        .await;
    assert_eq!(incompatible.status(), StatusCode::CONFLICT);

    let created: StorageZoneResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/storage-zones",
            Some("zone-exclusive"),
            Some(configure_body(
                rig.facility_id,
                "PICK-X",
                "pick",
                1,
                &[rig.pick_locations[0]],
                None,
            )),
        )
        .await,
    )
    .await;
    assert!(created.storage_zone_id > 0);
    let duplicate = rig
        .send(
            Method::POST,
            "/api/v1/storage-zones",
            Some("zone-exclusive-other"),
            Some(configure_body(
                rig.facility_id,
                "PICK-Y",
                "pick",
                2,
                &[rig.pick_locations[0]],
                None,
            )),
        )
        .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let mutation = wareboxes_persistence_postgres::locations::update_location(
        &rig.fixture.db,
        rig.tenant_id,
        rig.pick_locations[0],
        None,
        None,
        None,
        None,
        Some(false),
        None,
        None,
    )
    .await;
    assert!(mutation.is_err());
}

#[tokio::test]
async fn scope_permission_rls_and_immutable_evidence_are_enforced() {
    let rig = Rig::new().await;
    let out_of_scope_location = rig
        .fixture
        .location(rig.tenant_id, rig.other_facility_id, "OTHER-PICK")
        .await;
    let wrong_facility = rig
        .send(
            Method::POST,
            "/api/v1/storage-zones",
            Some("zone-cross-facility"),
            Some(configure_body(
                rig.facility_id,
                "PICK-CROSS",
                "pick",
                1,
                &[out_of_scope_location],
                None,
            )),
        )
        .await;
    assert_eq!(wrong_facility.status(), StatusCode::NOT_FOUND);

    let created: StorageZoneResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/storage-zones",
            Some("zone-rls"),
            Some(configure_body(
                rig.facility_id,
                "PICK-RLS",
                "pick",
                4,
                &[rig.pick_locations[2]],
                None,
            )),
        )
        .await,
    )
    .await;
    let mut unbound = rig.fixture.db.begin().await.unwrap();
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM storage_zones),(SELECT count(*) FROM storage_zone_locations)",
    )
    .fetch_one(&mut *unbound)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0));
    unbound.rollback().await.unwrap();

    let admin = admin_db_for(&rig.fixture.db).await;
    let grants: Vec<bool> = sqlx::query_scalar(
        r#"
        SELECT ARRAY[
               has_table_privilege('wareboxes_app','storage_zones','SELECT'),
               has_table_privilege('wareboxes_app','storage_zones','INSERT'),
               has_table_privilege('wareboxes_app','storage_zones','UPDATE'),
               has_table_privilege('wareboxes_app','storage_zones','DELETE'),
               has_column_privilege('wareboxes_app','storage_zones','effective_to','UPDATE'),
               has_column_privilege('wareboxes_app','storage_zones','name','UPDATE'),
               has_table_privilege('wareboxes_app','storage_zone_locations','SELECT'),
               has_table_privilege('wareboxes_app','storage_zone_locations','INSERT'),
               has_table_privilege('wareboxes_app','storage_zone_locations','UPDATE'),
               has_table_privilege('wareboxes_app','storage_zone_locations','DELETE'),
               has_sequence_privilege('wareboxes_app','storage_zones_id_seq','USAGE'),
               has_sequence_privilege('wareboxes_app','storage_zones_id_seq','SELECT'),
               has_sequence_privilege('wareboxes_app','storage_zones_id_seq','UPDATE')
        ]
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        grants,
        vec![true, true, false, false, true, false, true, true, false, false, true, false, false]
    );
    let immutable = sqlx::query("UPDATE storage_zone_locations SET location_sequence=2 WHERE tenant_id=$1 AND storage_zone_id=$2")
        .bind(rig.tenant_id.get())
        .bind(created.storage_zone_id)
        .execute(&admin)
        .await;
    assert!(immutable.is_err());
    let evidence: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT count(*) FROM command_idempotency_records WHERE tenant_id=$1 AND operation='topology.storage_zone.configure.v1' AND idempotency_key='zone-rls'),
          (SELECT count(*) FROM outbox_events WHERE tenant_id=$1 AND event_type='topology.storage_zone.configured' AND aggregate_id=$2::TEXT),
          (SELECT count(*) FROM storage_zone_locations WHERE tenant_id=$1 AND storage_zone_id=$2)
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(created.storage_zone_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(evidence, (1, 1, 1));
    admin.close().await;

    assert!(rig.user_id > 0);
}
