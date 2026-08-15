mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    AssignYardVisitDoorRequest, ConfigureYardLocationRequest, CreateYardAppointmentRequest,
    GateInYardVisitRequest, MoveYardVisitRequest, PageLimit, RegisterYardAssetRequest, Revision,
    YardAppointmentResponse, YardAppointmentStatus, YardAssetKind, YardAssetResponse,
    YardDirection, YardDockOperationRequest, YardLifecycleRequest, YardLocationKind,
    YardLocationResponse, YardOperation, YardVisitResponse, YardVisitStatus, YardWorkspaceRequest,
    YardWorkspaceResponse,
};

async fn grant_wms(fixture: &Fixture, tenant_id: TenantId, user_id: i64, suffix: &str) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        "wms",
        Some("Warehouse operator"),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("yard-operator-{suffix}"),
        None,
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

fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    key: Option<&str>,
    body: Option<&T>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = key {
        builder = builder.header(IDEMPOTENCY_KEY_HEADER, key);
    }
    let body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(body).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
    expected: StatusCode,
) -> T {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        expected,
        "unexpected response: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    user_id: i64,
    token: String,
    owner_id: i64,
    facility_id: i64,
    app: axum::Router,
}

impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let user = fixture.user("yard-operator@test.local").await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        grant_wms(&fixture, tenant_id, user.id, "primary").await;
        let owner_id = fixture.inventory_owner(tenant_id, "Yard Client").await;
        let facility_id = fixture
            .facility(tenant_id, "Yard Distribution Center")
            .await;
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
        let token = wareboxes_api::auth::create_session(&fixture.db, user.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            tenant_id,
            user_id: user.id,
            token,
            owner_id,
            facility_id,
            app,
        }
    }

    async fn send<T: Serialize>(
        &self,
        method: Method,
        uri: &str,
        key: Option<&str>,
        body: Option<&T>,
    ) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(request(&self.token, self.tenant_id, method, uri, key, body))
            .await
            .unwrap()
    }

    async fn location(
        &self,
        key: &str,
        code: &str,
        kind: YardLocationKind,
    ) -> YardLocationResponse {
        response_json(
            self.send(
                Method::POST,
                "/api/v1/yard/locations",
                Some(key),
                Some(&ConfigureYardLocationRequest {
                    facility_id: self.facility_id,
                    code: code.into(),
                    name: format!("{code} location"),
                    kind,
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await
    }

    async fn asset(&self, key: &str, number: &str) -> YardAssetResponse {
        response_json(
            self.send(
                Method::POST,
                "/api/v1/yard/assets",
                Some(key),
                Some(&RegisterYardAssetRequest {
                    kind: YardAssetKind::Trailer,
                    asset_number: number.into(),
                    carrier: "Example Freight".into(),
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await
    }

    async fn appointment(&self, key: &str, number: &str) -> YardAppointmentResponse {
        let now = db::now_iso();
        response_json(
            self.send(
                Method::POST,
                "/api/v1/yard/appointments",
                Some(key),
                Some(&CreateYardAppointmentRequest {
                    inventory_owner_id: self.owner_id,
                    facility_id: self.facility_id,
                    direction: YardDirection::Inbound,
                    appointment_number: number.into(),
                    scheduled_from: (now - std::time::Duration::from_secs(300)).to_rfc3339(),
                    scheduled_until: (now + std::time::Duration::from_secs(3_600)).to_rfc3339(),
                    carrier: "Example Freight".into(),
                    expected_asset_kind: YardAssetKind::Trailer,
                    expected_asset_number: None,
                    inbound_load_id: None,
                    outbound_load_id: None,
                    free_minutes: 0,
                    note: Some("Expected inbound gate move".into()),
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await
    }

    async fn gate_in(
        &self,
        key: &str,
        appointment_id: Option<i64>,
        asset_id: i64,
        gate_id: i64,
    ) -> YardVisitResponse {
        response_json(
            self.send(
                Method::POST,
                "/api/v1/yard/visits",
                Some(key),
                Some(&GateInYardVisitRequest {
                    appointment_id,
                    inventory_owner_id: self.owner_id,
                    facility_id: self.facility_id,
                    direction: YardDirection::Inbound,
                    asset_id,
                    driver_name: "Morgan Driver".into(),
                    gate_location_id: gate_id,
                    note: Some("Security seal verified".into()),
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await
    }
}

#[tokio::test]
async fn appointment_gate_spot_door_unload_and_depart_are_replay_safe_and_auditable() {
    let rig = Rig::new().await;
    let gate = rig
        .location("yard-gate", "GATE-IN", YardLocationKind::Gate)
        .await;
    let parking = rig
        .location("yard-parking", "YARD-01", YardLocationKind::Parking)
        .await;
    let door = rig
        .location("yard-door", "DOOR-07", YardLocationKind::DockDoor)
        .await;
    let asset = rig.asset("yard-asset", "TRL-1007").await;
    let appointment = rig.appointment("yard-appointment", "APT-1007").await;
    assert_eq!(appointment.status, YardAppointmentStatus::Scheduled);
    assert_eq!(appointment.revision.get(), 1);

    let visit = rig
        .gate_in(
            "yard-gate-in",
            Some(appointment.appointment_id),
            asset.asset_id,
            gate.location_id,
        )
        .await;
    assert_eq!(visit.status, YardVisitStatus::GatedIn);
    assert_eq!(visit.revision.get(), 1);
    assert_eq!(visit.events.len(), 1);

    let replay = rig
        .gate_in(
            "yard-gate-in",
            Some(appointment.appointment_id),
            asset.asset_id,
            gate.location_id,
        )
        .await;
    assert_eq!(replay, visit);

    assert_eq!(
        rig.send(
            Method::POST,
            &format!("/api/v1/yard/visits/{}/gate-outs", visit.visit_id),
            Some("yard-premature-gate-out"),
            Some(&YardLifecycleRequest {
                expected_revision: visit.revision,
                note: "Attempted departure before unload".into(),
            }),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let spotted: YardVisitResponse = response_json(
        rig.send(
            Method::POST,
            &format!("/api/v1/yard/visits/{}/spot-moves", visit.visit_id),
            Some("yard-spot"),
            Some(&MoveYardVisitRequest {
                expected_revision: visit.revision,
                destination_location_id: parking.location_id,
                note: "Park pending door".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(spotted.status, YardVisitStatus::InYard);
    assert_eq!(spotted.current_location_code.as_deref(), Some("YARD-01"));

    let assigned: YardVisitResponse = response_json(
        rig.send(
            Method::POST,
            &format!("/api/v1/yard/visits/{}/door-assignments", visit.visit_id),
            Some("yard-door-assign"),
            Some(&AssignYardVisitDoorRequest {
                expected_revision: spotted.revision,
                door_location_id: door.location_id,
                note: "Door clear and chocked".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(assigned.status, YardVisitStatus::AtDoor);
    assert_eq!(assigned.dock_door_code.as_deref(), Some("DOOR-07"));

    assert_eq!(
        rig.send(
            Method::POST,
            &format!("/api/v1/yard/visits/{}/operation-starts", visit.visit_id),
            Some("yard-wrong-operation"),
            Some(&YardDockOperationRequest {
                expected_revision: assigned.revision,
                operation: YardOperation::Loading,
                note: "Wrong direction".into(),
            }),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let unloading: YardVisitResponse = response_json(
        rig.send(
            Method::POST,
            &format!("/api/v1/yard/visits/{}/operation-starts", visit.visit_id),
            Some("yard-unload-start"),
            Some(&YardDockOperationRequest {
                expected_revision: assigned.revision,
                operation: YardOperation::Unloading,
                note: "Restraint engaged".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(unloading.status, YardVisitStatus::Unloading);

    let ready: YardVisitResponse = response_json(
        rig.send(
            Method::POST,
            &format!(
                "/api/v1/yard/visits/{}/operation-completions",
                visit.visit_id
            ),
            Some("yard-unload-complete"),
            Some(&YardDockOperationRequest {
                expected_revision: unloading.revision,
                operation: YardOperation::Unloading,
                note: "Trailer empty and swept".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(ready.status, YardVisitStatus::ReadyToDepart);

    let departed: YardVisitResponse = response_json(
        rig.send(
            Method::POST,
            &format!("/api/v1/yard/visits/{}/gate-outs", visit.visit_id),
            Some("yard-gate-out"),
            Some(&YardLifecycleRequest {
                expected_revision: ready.revision,
                note: "Exit inspection complete".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(departed.status, YardVisitStatus::GatedOut);
    assert_eq!(departed.revision.get(), 6);
    assert_eq!(departed.events.len(), 6);
    assert!(departed.detention.is_some());
    assert!(departed.current_location_id.is_none());
    assert!(departed.dock_door_location_id.is_none());

    let workspace: YardWorkspaceResponse = response_json(
        rig.send::<()>(
            Method::GET,
            &format!(
                "/api/v1/yard/workspace?facility_id={}&inventory_owner_id={}&include_completed=true&limit=10",
                rig.facility_id, rig.owner_id
            ),
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(workspace.visits.len(), 1);
    assert_eq!(workspace.visits[0].status, YardVisitStatus::GatedOut);
    assert_eq!(workspace.appointments.len(), 1);
    assert_eq!(
        workspace.appointments[0].status,
        YardAppointmentStatus::Completed
    );

    let mut immutable = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    assert!(sqlx::query(
        "UPDATE yard_visit_events SET note='tampered' WHERE tenant_id=$1 AND visit_id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(visit.visit_id)
    .execute(&mut *immutable)
    .await
    .is_err());
    immutable.rollback().await.unwrap();

    let grants: (bool, bool, bool) = sqlx::query_as(
        r#"SELECT has_table_privilege('wareboxes_app','yard_visit_events','SELECT'),
                  has_table_privilege('wareboxes_app','yard_visit_events','INSERT'),
                  has_table_privilege('wareboxes_app','yard_visit_events','DELETE')"#,
    )
    .fetch_one(&rig.fixture.db)
    .await
    .unwrap();
    assert_eq!(grants, (true, true, false));

    let mut outbox_tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events WHERE tenant_id=$1 AND event_type LIKE 'yard.%'",
    )
    .bind(rig.tenant_id.get())
    .fetch_one(&mut *outbox_tx)
    .await
    .unwrap();
    assert!(outbox_count >= 12);
    outbox_tx.commit().await.unwrap();
}

#[tokio::test]
async fn terminal_appointments_and_dock_collisions_fail_closed() {
    let rig = Rig::new().await;
    let gate = rig
        .location("collision-gate", "GATE-C", YardLocationKind::Gate)
        .await;
    let door = rig
        .location("collision-door", "DOOR-C", YardLocationKind::DockDoor)
        .await;
    let asset_a = rig.asset("collision-asset-a", "TRL-C-A").await;
    let asset_b = rig.asset("collision-asset-b", "TRL-C-B").await;
    let appointment = rig.appointment("cancel-appointment", "APT-CANCEL").await;
    let cancelled: YardAppointmentResponse = response_json(
        rig.send(
            Method::POST,
            &format!(
                "/api/v1/yard/appointments/{}/cancellations",
                appointment.appointment_id
            ),
            Some("cancel-appointment-command"),
            Some(&YardLifecycleRequest {
                expected_revision: appointment.revision,
                note: "Carrier cancelled tender".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(cancelled.status, YardAppointmentStatus::Cancelled);
    assert_eq!(
        rig.send(
            Method::POST,
            "/api/v1/yard/visits",
            Some("cancelled-gate-in"),
            Some(&GateInYardVisitRequest {
                appointment_id: Some(cancelled.appointment_id),
                inventory_owner_id: rig.owner_id,
                facility_id: rig.facility_id,
                direction: YardDirection::Inbound,
                asset_id: asset_a.asset_id,
                driver_name: "Cancelled Driver".into(),
                gate_location_id: gate.location_id,
                note: None,
            }),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let visit_a = rig
        .gate_in("collision-gate-a", None, asset_a.asset_id, gate.location_id)
        .await;
    let visit_b = rig
        .gate_in("collision-gate-b", None, asset_b.asset_id, gate.location_id)
        .await;
    let assignment_a = AssignYardVisitDoorRequest {
        expected_revision: visit_a.revision,
        door_location_id: door.location_id,
        note: "First concurrent door assignment".into(),
    };
    let assignment_b = AssignYardVisitDoorRequest {
        expected_revision: visit_b.revision,
        door_location_id: door.location_id,
        note: "Second concurrent door assignment".into(),
    };
    let assignment_path_a = format!("/api/v1/yard/visits/{}/door-assignments", visit_a.visit_id);
    let assignment_path_b = format!("/api/v1/yard/visits/{}/door-assignments", visit_b.visit_id);
    let (response_a, response_b) = tokio::join!(
        rig.send(
            Method::POST,
            &assignment_path_a,
            Some("collision-assign-a"),
            Some(&assignment_a),
        ),
        rig.send(
            Method::POST,
            &assignment_path_b,
            Some("collision-assign-b"),
            Some(&assignment_b),
        ),
    );
    let mut statuses = [response_a.status(), response_b.status()];
    statuses.sort_by_key(|status| status.as_u16());
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::CONFLICT],
        "exactly one concurrent visit may occupy a dock door"
    );

    let mut spoofed_history = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    sqlx::query(
        r#"UPDATE yard_visits SET status='in_yard',revision=revision+1,
           current_location_id=$3,dock_door_location_id=NULL
           WHERE tenant_id=$1 AND id=$2 AND status='gated_in'"#,
    )
    .bind(rig.tenant_id.get())
    .bind(if response_a.status() == StatusCode::CONFLICT {
        visit_a.visit_id
    } else {
        visit_b.visit_id
    })
    .bind(gate.location_id)
    .execute(&mut *spoofed_history)
    .await
    .unwrap();
    let unassigned_visit = if response_a.status() == StatusCode::CONFLICT {
        visit_a.visit_id
    } else {
        visit_b.visit_id
    };
    assert!(sqlx::query(
        r#"INSERT INTO yard_visit_events
           (tenant_id,inventory_owner_id,facility_id,visit_id,event_kind,from_status,to_status,
            from_location_id,to_location_id,resulting_revision,actor_user_id,occurred_at)
           VALUES($1,$2,$3,$4,'door_assigned','gated_in','in_yard',$5,$5,2,$6,statement_timestamp())"#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.owner_id)
    .bind(rig.facility_id)
    .bind(unassigned_visit)
    .bind(gate.location_id)
    .bind(rig.user_id)
    .execute(&mut *spoofed_history)
    .await
    .is_err());
    spoofed_history.rollback().await.unwrap();

    let page = YardWorkspaceRequest {
        facility_id: Some(rig.facility_id),
        inventory_owner_id: Some(rig.owner_id),
        include_completed: false,
        cursor: None,
        limit: PageLimit::new(1).unwrap(),
    };
    assert_eq!(page.limit.get(), 1);
    assert!(Revision::new(1).is_ok());
}
