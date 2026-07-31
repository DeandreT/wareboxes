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
    CycleCountClaimHeartbeatResponse, CycleCountClaimReleaseResponse, CycleCountClaimResponse,
    CycleCountConfirmationResponse, ErrorReason, ErrorResponse,
};

struct CountContext {
    fixture: Fixture,
    tenant_id: TenantId,
    user_id: i64,
    token: String,
    app: axum::Router,
    location_id: i64,
    location_barcode: String,
    item_id: i64,
    item_barcode: String,
    balance_id: i64,
    task_id: i64,
}

impl CountContext {
    async fn new(suffix: &str, with_item_barcode: bool) -> Self {
        let fixture = Fixture::new().await;
        let user = fixture
            .wms_user(&format!("rf-count-{suffix}@test.local"))
            .await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        let facility_id = fixture
            .facility(tenant_id, &format!("RF Count {suffix} DC"))
            .await;
        let location_barcode = format!("COUNT-{}", suffix.to_ascii_uppercase());
        let location_id = fixture
            .location(tenant_id, facility_id, &location_barcode)
            .await;
        let owner_id = fixture
            .inventory_owner(tenant_id, &format!("RF Count {suffix} Owner"))
            .await;
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
        let item_id = fixture
            .item(tenant_id, &format!("RF Count {suffix} Item"), "each")
            .await;
        let item_barcode = format!("ITEM-{}", suffix.to_ascii_uppercase());
        if with_item_barcode {
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
        }
        let batch_id = repo::inventory::add_item_batch(
            &fixture.db,
            tenant_id,
            owner_id,
            item_id,
            None,
            Some(&format!("LOT-{suffix}")),
            None,
            None,
        )
        .await
        .unwrap();
        repo::inventory::receive_inventory(
            &fixture.db,
            tenant_id,
            user.id,
            batch_id,
            location_id,
            10,
            None,
            Some("RF count fixture"),
            None,
            None,
            &format!("rf-count-{suffix}-receipt"),
        )
        .await
        .unwrap();
        let balance_id = repo::inventory::get_balances(&fixture.db, tenant_id, false)
            .await
            .unwrap()
            .into_iter()
            .find(|balance| balance.item_batch_id == batch_id)
            .unwrap()
            .id;
        let task_id = repo::tasks::create_item_location_cycle_count_task(
            &fixture.db,
            tenant_id,
            user.id,
            location_id,
            item_id,
            Some("rf"),
            None,
            None,
            balance_id,
            None,
        )
        .await
        .unwrap();
        let token = auth::create_session(&fixture.db, user.id).await.unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            tenant_id,
            user_id: user.id,
            token,
            app,
            location_id,
            location_barcode,
            item_id,
            item_barcode,
            balance_id,
            task_id,
        }
    }

    async fn send(
        &self,
        method: Method,
        uri: &str,
        key: Option<&str>,
        body: Value,
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(TENANT_ID_HEADER, self.tenant_id.to_string())
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(key) = key {
            request = request.header(IDEMPOTENCY_KEY_HEADER, key);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    async fn post(&self, uri: &str, key: &str, body: Value) -> axum::response::Response {
        self.send(Method::POST, uri, Some(key), body).await
    }

    async fn status(&self, task_id: i64) -> String {
        let mut tx = tenant_tx(&self.fixture.db, self.tenant_id).await;
        let status =
            sqlx::query_scalar("SELECT status FROM work_tasks WHERE tenant_id = $1 AND id = $2")
                .bind(self.tenant_id.get())
                .bind(task_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        tx.rollback().await.unwrap();
        status
    }
}

async fn response_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    serde_json::from_slice(&to_bytes(response.into_body(), 128 * 1024).await.unwrap()).unwrap()
}

async fn assert_status(
    response: axum::response::Response,
    expected: StatusCode,
) -> axum::response::Response {
    if response.status() != expected {
        let actual = response.status();
        let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
        panic!(
            "expected {expected}, got {actual}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    response
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("wareboxes_api=debug")
        .try_init();
}

#[tokio::test]
async fn rf_count_is_scannable_blind_replay_safe_and_lease_managed() {
    init_tracing();
    let count = CountContext::new("lifecycle", false).await;

    let unavailable = count
        .post("/api/v1/cycle-count-claims/next", "count-empty", json!({}))
        .await;
    assert_eq!(unavailable.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Option<CycleCountClaimResponse>>(unavailable).await,
        None
    );
    assert_eq!(count.status(count.task_id).await, "open");

    repo::items::add_barcode(
        &count.fixture.db,
        count.tenant_id,
        count.item_id,
        &count.item_barcode,
        "code128",
        None,
    )
    .await
    .unwrap();
    let claimed = count
        .post("/api/v1/cycle-count-claims/next", "count-claim", json!({}))
        .await;
    let claimed = assert_status(claimed, StatusCode::OK).await;
    let claim = response_json::<Option<CycleCountClaimResponse>>(claimed)
        .await
        .unwrap();
    assert_eq!(claim.task_id, count.task_id);
    assert_eq!(claim.location.barcode, count.location_barcode);
    assert_eq!(claim.item.barcodes, vec![count.item_barcode.clone()]);
    let claim_json = serde_json::to_value(&claim).unwrap();
    assert!(claim_json.get("expected_quantity").is_none());
    assert!(claim_json["stock"].get("quantity").is_none());

    let current = count
        .send(
            Method::GET,
            "/api/v1/cycle-count-claims/current",
            None,
            json!({}),
        )
        .await;
    assert_eq!(current.status(), StatusCode::OK);
    assert_eq!(
        response_json::<Option<CycleCountClaimResponse>>(current)
            .await
            .unwrap()
            .task_id,
        count.task_id
    );

    let heartbeat_uri = format!("/api/v1/cycle-count-claims/{}/heartbeats", count.task_id);
    let heartbeat = count
        .post(&heartbeat_uri, "count-heartbeat", json!({}))
        .await;
    assert_eq!(heartbeat.status(), StatusCode::OK);
    let first_heartbeat = response_json::<CycleCountClaimHeartbeatResponse>(heartbeat).await;
    let heartbeat_replay = count
        .post(&heartbeat_uri, "count-heartbeat", json!({}))
        .await;
    assert_eq!(
        response_json::<CycleCountClaimHeartbeatResponse>(heartbeat_replay).await,
        first_heartbeat
    );

    let confirm_uri = format!("/api/v1/cycle-count-tasks/{}/confirmations", count.task_id);
    let wrong_scan = count
        .post(
            &confirm_uri,
            "count-wrong-scan",
            json!({
                "location_barcode": "WRONG",
                "item_barcode": count.item_barcode,
                "counted_quantity": 7
            }),
        )
        .await;
    assert_eq!(wrong_scan.status(), StatusCode::CONFLICT);
    assert_eq!(count.status(count.task_id).await, "in_progress");

    let confirmation_body = json!({
        "location_barcode": count.location_barcode,
        "item_barcode": count.item_barcode,
        "counted_quantity": 7
    });
    let confirmed = count
        .post(&confirm_uri, "count-confirm", confirmation_body.clone())
        .await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    let confirmation = response_json::<CycleCountConfirmationResponse>(confirmed).await;
    assert_eq!(confirmation.variance_quantity, -3);
    assert!(confirmation.inventory_transaction_id.is_some());
    let replay = count
        .post(&confirm_uri, "count-confirm", confirmation_body)
        .await;
    assert_eq!(
        response_json::<CycleCountConfirmationResponse>(replay).await,
        confirmation
    );
    let changed = count
        .post(
            &confirm_uri,
            "count-confirm",
            json!({
                "location_barcode": count.location_barcode,
                "item_barcode": count.item_barcode,
                "counted_quantity": 8
            }),
        )
        .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json::<ErrorResponse>(changed).await.reason,
        ErrorReason::IdempotencyKeyReused
    );

    let balance = repo::inventory::get_balances(&count.fixture.db, count.tenant_id, false)
        .await
        .unwrap()
        .into_iter()
        .find(|balance| balance.id == count.balance_id)
        .unwrap();
    assert_eq!(balance.qty_on_hand, 7);

    let release_task = repo::tasks::create_item_location_cycle_count_task(
        &count.fixture.db,
        count.tenant_id,
        count.user_id,
        count.location_id,
        count.item_id,
        Some("rf"),
        None,
        None,
        count.balance_id,
        None,
    )
    .await
    .unwrap();
    let claim_release = count
        .post(
            &format!("/api/v1/cycle-count-claims/{release_task}"),
            "count-release-claim",
            json!({}),
        )
        .await;
    assert_eq!(claim_release.status(), StatusCode::OK);
    let release_uri = format!("/api/v1/cycle-count-claims/{release_task}/releases");
    let released = count
        .post(
            &release_uri,
            "count-release",
            json!({"reason": "work_interrupted"}),
        )
        .await;
    assert_eq!(released.status(), StatusCode::OK);
    let release = response_json::<CycleCountClaimReleaseResponse>(released).await;
    assert_eq!(release.release_count, 1);
    let replay = count
        .post(
            &release_uri,
            "count-release",
            json!({"reason": "work_interrupted"}),
        )
        .await;
    assert_eq!(
        response_json::<CycleCountClaimReleaseResponse>(replay).await,
        release
    );
    assert_eq!(count.status(release_task).await, "open");
}

#[tokio::test]
async fn cycle_count_claim_by_id_conceals_foreign_tenant_tasks() {
    init_tracing();
    let owner = CountContext::new("owner", true).await;
    let foreign_user = owner.fixture.wms_user("rf-count-foreign@test.local").await;
    let foreign_tenant = tenant_for_user(&owner.fixture.db, foreign_user.id).await;
    let foreign_token = auth::create_session(&owner.fixture.db, foreign_user.id).await;
    let response = owner
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/cycle-count-claims/{}", owner.task_id))
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", foreign_token.unwrap()),
                )
                .header(TENANT_ID_HEADER, foreign_tenant.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .header(IDEMPOTENCY_KEY_HEADER, "foreign-count-guess")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_status(response, StatusCode::NOT_FOUND).await;
    assert_eq!(owner.status(owner.task_id).await, "open");
}
