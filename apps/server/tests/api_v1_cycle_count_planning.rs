mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CreateCycleCountTaskResponse, CycleCountCandidatePage, CycleCountClaimResponse,
    CycleCountConfirmationResponse, CycleCountWorkPage, CycleCountWorkStatus, ErrorReason,
    ErrorResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    user_id: i64,
    token: String,
    app: axum::Router,
    facility_id: i64,
    item_barcode: String,
    location_barcodes: Vec<String>,
    balance_ids: Vec<i64>,
}

impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let user = fixture.wms_user("cycle-count-planner@test.local").await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        grant_permission(
            &fixture,
            tenant_id,
            user.id,
            "wms_supervisor",
            "cycle-count-supervisor",
        )
        .await;
        let facility_id = fixture.facility(tenant_id, "Count Planning DC").await;
        let owner_id = fixture
            .inventory_owner(tenant_id, "Count Planning Client")
            .await;
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
        let item_id = fixture
            .item(tenant_id, "Count Planning Widget", "each")
            .await;
        let item_barcode = "COUNT-PLAN-SKU".to_owned();
        repo::items::add_barcode(
            &fixture.db,
            tenant_id,
            item_id,
            &item_barcode,
            "code128",
            None,
        )
        .await
        .unwrap();
        let batch_id = repo::inventory::add_item_batch(
            &fixture.db,
            tenant_id,
            owner_id,
            item_id,
            None,
            Some("COUNT-PLAN-LOT"),
            None,
            None,
        )
        .await
        .unwrap();
        let mut location_barcodes = Vec::new();
        let mut balance_ids = Vec::new();
        for (sequence, quantity) in [10_i64, 30, 20].into_iter().enumerate() {
            let barcode = format!("COUNT-PLAN-{:02}", sequence + 1);
            let location_id = fixture.location(tenant_id, facility_id, &barcode).await;
            repo::inventory::receive_inventory(
                &fixture.db,
                tenant_id,
                user.id,
                batch_id,
                location_id,
                quantity,
                None,
                Some("cycle-count planning fixture"),
                None,
                None,
                &format!("cycle-count-plan-receive-{sequence}"),
            )
            .await
            .unwrap();
            let balance_id = repo::inventory::get_balances(&fixture.db, tenant_id, false)
                .await
                .unwrap()
                .into_iter()
                .find(|balance| {
                    balance.item_batch_id == batch_id && balance.location_id == location_id
                })
                .unwrap()
                .id;
            location_barcodes.push(barcode);
            balance_ids.push(balance_id);
        }
        let token = auth::create_session(&fixture.db, user.id).await.unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            tenant_id,
            user_id: user.id,
            token,
            app,
            facility_id,
            item_barcode,
            location_barcodes,
            balance_ids,
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
}

#[tokio::test]
async fn supervisor_plans_blind_count_and_monitors_immutable_result() {
    let rig = Rig::new().await;
    let first = expect_status(
        rig.send(
            Method::GET,
            "/api/v1/cycle-count-candidates?limit=2&sort=quantity&direction=desc",
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let first: CycleCountCandidatePage = response_json(first).await;
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.quantity.on_hand)
            .collect::<Vec<_>>(),
        vec![30, 20],
        "sorting must happen before the page limit"
    );
    let cursor = first.next_cursor.unwrap();
    let second: CycleCountCandidatePage = response_json(
        expect_status(
            rig.send(
                Method::GET,
                &format!(
                    "/api/v1/cycle-count-candidates?limit=2&sort=quantity&direction=desc&cursor={}",
                    cursor.as_str()
                ),
                None,
                None,
            )
            .await,
            StatusCode::OK,
        )
        .await,
    )
    .await;
    assert_eq!(
        second
            .items
            .iter()
            .map(|item| item.quantity.on_hand)
            .collect::<Vec<_>>(),
        vec![10]
    );
    expect_status(
        rig.send(
            Method::GET,
            &format!(
                "/api/v1/cycle-count-candidates?limit=2&sort=quantity&direction=asc&cursor={}",
                cursor.as_str()
            ),
            None,
            None,
        )
        .await,
        StatusCode::BAD_REQUEST,
    )
    .await;

    let target = rig.balance_ids[0];
    let request = json!({
        "inventory_balance_id": target,
        "note": "Quarterly blind count"
    });
    let created: CreateCycleCountTaskResponse = response_json(
        expect_status(
            rig.send(
                Method::POST,
                "/api/v1/cycle-count-tasks",
                Some("cycle-count-create"),
                Some(request.clone()),
            )
            .await,
            StatusCode::OK,
        )
        .await,
    )
    .await;
    let replay: CreateCycleCountTaskResponse = response_json(
        expect_status(
            rig.send(
                Method::POST,
                "/api/v1/cycle-count-tasks",
                Some("cycle-count-create"),
                Some(request),
            )
            .await,
            StatusCode::OK,
        )
        .await,
    )
    .await;
    assert_eq!(replay, created);
    let changed = expect_status(
        rig.send(
            Method::POST,
            "/api/v1/cycle-count-tasks",
            Some("cycle-count-create"),
            Some(json!({"inventory_balance_id": target})),
        )
        .await,
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(
        response_json::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let candidates: CycleCountCandidatePage = response_json(
        rig.send(
            Method::GET,
            "/api/v1/cycle-count-candidates?limit=100",
            None,
            None,
        )
        .await,
    )
    .await;
    assert!(!candidates
        .items
        .iter()
        .any(|item| item.stock.inventory_balance_id == target));
    let open_work: CycleCountWorkPage = response_json(
        rig.send(
            Method::GET,
            "/api/v1/cycle-count-tasks?limit=100",
            None,
            None,
        )
        .await,
    )
    .await;
    let work = open_work
        .items
        .iter()
        .find(|item| item.task_id == created.task_id)
        .unwrap();
    assert_eq!(work.status, CycleCountWorkStatus::Pending);
    assert_eq!(work.current_quantity.as_ref().unwrap().on_hand, 10);

    let claim: Option<CycleCountClaimResponse> = response_json(
        rig.send(
            Method::POST,
            "/api/v1/cycle-count-claims/next",
            Some("cycle-count-claim"),
            Some(json!({})),
        )
        .await,
    )
    .await;
    let claim = claim.unwrap();
    assert_eq!(claim.task_id, created.task_id);
    assert!(serde_json::to_value(&claim)
        .unwrap()
        .get("expected_quantity")
        .is_none());
    let confirmation: CycleCountConfirmationResponse = response_json(
        expect_status(
            rig.send(
                Method::POST,
                &format!(
                    "/api/v1/cycle-count-tasks/{}/confirmations",
                    created.task_id
                ),
                Some("cycle-count-confirm"),
                Some(json!({
                    "location_barcode": rig.location_barcodes[0],
                    "item_barcode": rig.item_barcode,
                    "counted_quantity": 7,
                    "note": "Verified twice"
                })),
            )
            .await,
            StatusCode::OK,
        )
        .await,
    )
    .await;
    assert_eq!(confirmation.variance_quantity, -3);

    let completed: CycleCountWorkPage = response_json(
        rig.send(
            Method::GET,
            "/api/v1/cycle-count-tasks?limit=100&status=completed",
            None,
            None,
        )
        .await,
    )
    .await;
    let completed = completed
        .items
        .iter()
        .find(|item| item.task_id == created.task_id)
        .unwrap();
    assert_eq!(completed.system_quantity.as_ref().unwrap().on_hand, 10);
    assert_eq!(completed.counted_quantity, Some(7));
    assert_eq!(completed.variance_quantity, Some(-3));
    assert_eq!(completed.note.as_deref(), Some("Quarterly blind count"));
    assert!(completed.inventory_transaction_id.is_some());
}

#[tokio::test]
async fn supervisor_permission_and_current_scope_fail_closed() {
    let rig = Rig::new().await;
    let worker = rig.fixture.wms_user("cycle-count-worker@test.local").await;
    let worker_tenant = tenant_for_user(&rig.fixture.db, worker.id).await;
    let worker_token = auth::create_session(&rig.fixture.db, worker.id)
        .await
        .unwrap();
    let response = rig
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/cycle-count-candidates?limit=100")
                .header(header::AUTHORIZATION, format!("Bearer {worker_token}"))
                .header(TENANT_ID_HEADER, worker_tenant.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let target = rig.balance_ids[0];
    let request = json!({"inventory_balance_id": target});
    let created: CreateCycleCountTaskResponse = response_json(
        rig.send(
            Method::POST,
            "/api/v1/cycle-count-tasks",
            Some("cycle-count-scoped"),
            Some(request.clone()),
        )
        .await,
    )
    .await;
    assert!(repo::tenants::update_user_access_scope(
        &rig.fixture.db,
        rig.tenant_id,
        &UpdateUserAccessScope {
            user_id: rig.user_id,
            all_facilities: false,
            facility_ids: vec![rig.facility_id],
            all_inventory_owners: false,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap());
    expect_status(
        rig.send(
            Method::POST,
            "/api/v1/cycle-count-tasks",
            Some("cycle-count-scoped"),
            Some(request),
        )
        .await,
        StatusCode::NOT_FOUND,
    )
    .await;
    let work: CycleCountWorkPage = response_json(
        rig.send(
            Method::GET,
            "/api/v1/cycle-count-tasks?limit=100",
            None,
            None,
        )
        .await,
    )
    .await;
    assert!(!work
        .items
        .iter()
        .any(|item| item.task_id == created.task_id));
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    serde_json::from_slice(&to_bytes(response.into_body(), 512 * 1024).await.unwrap()).unwrap()
}

async fn expect_status(
    response: axum::response::Response,
    expected: StatusCode,
) -> axum::response::Response {
    if response.status() != expected {
        let actual = response.status();
        let body = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
        panic!(
            "expected {expected}, got {actual}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    response
}

async fn grant_permission(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    permission_name: &str,
    role_name: &str,
) {
    let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
        &fixture.db,
        tenant_id,
        permission_name,
    )
    .await
    .unwrap()
    {
        Some(permission) => permission.id,
        None => wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            permission_name,
            Some(permission_name),
        )
        .await
        .unwrap(),
    };
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        role_name,
        Some("Cycle-count supervisor test role"),
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
