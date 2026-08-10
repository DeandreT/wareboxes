mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::{json, Value};
use std::time::Duration;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{repo, routes, state::AppState};
use wareboxes_api_contract::v1::{
    ErrorReason, ErrorResponse, InventoryRecallPage, InventoryRecallResponse,
};
use wareboxes_core::dto::UpdateUserAccessScope;

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    user_id: i64,
    token: String,
    app: axum::Router,
    owner_id: i64,
    facility_id: i64,
    batch_id: i64,
    balance_ids: Vec<i64>,
}

impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let user = fixture.wms_user("inventory-recall@test.local").await;
        let tenant_id = tenant_for_user(&fixture.db, user.id).await;
        grant_permission(
            &fixture,
            tenant_id,
            user.id,
            "wms_supervisor",
            "inventory-recall-supervisor",
        )
        .await;
        let owner_id = fixture.inventory_owner(tenant_id, "Recall Client").await;
        let facility_id = fixture.facility(tenant_id, "Recall Facility").await;
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
        let item_id = fixture.item(tenant_id, "Recall Widget", "case").await;
        let batch_id = repo::inventory::add_item_batch(
            &fixture.db,
            tenant_id,
            owner_id,
            item_id,
            None,
            Some("RECALL-LOT-42"),
            None,
            None,
        )
        .await
        .unwrap();
        let mut balance_ids = Vec::new();
        for (index, quantity) in [7_i64, 5].into_iter().enumerate() {
            let location_id = fixture
                .location(tenant_id, facility_id, &format!("RECALL-{:02}", index + 1))
                .await;
            repo::inventory::receive_inventory(
                &fixture.db,
                tenant_id,
                user.id,
                batch_id,
                location_id,
                quantity,
                None,
                Some("recall test receipt"),
                Some("inventory-recall-test"),
                Some(batch_id),
                &format!("recall-receive-{index}"),
            )
            .await
            .unwrap();
            let mut tx = tenant_tx(&fixture.db, tenant_id).await;
            balance_ids.push(
                sqlx::query_scalar(
                    "SELECT id FROM inventory_balances WHERE tenant_id=$1 AND item_batch_id=$2 AND location_id=$3 AND deleted IS NULL",
                )
                .bind(tenant_id.get())
                .bind(batch_id)
                .bind(location_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap(),
            );
            tx.rollback().await.unwrap();
        }
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
            owner_id,
            facility_id,
            batch_id,
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
        Some("Inventory recall supervisor test role"),
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

#[tokio::test]
async fn recall_create_release_replay_and_projection_are_exact() {
    let rig = Rig::new().await;
    let create_body = json!({
        "facility_id": rig.facility_id,
        "item_batch_id": rig.batch_id,
        "reason": "supplier_notice",
        "note": "Supplier recall bulletin 42"
    });
    let created = rig
        .send(
            Method::POST,
            "/api/v1/inventory/recalls",
            Some("recall-create"),
            Some(create_body.clone()),
        )
        .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created: InventoryRecallResponse = json_response(created).await;
    assert_eq!(created.inventory_owner_id, rig.owner_id);
    assert_eq!(created.affected_position_count, 2);
    assert_eq!(created.held_quantity, 12);
    assert_eq!(created.revision.get(), 1);

    let replay: InventoryRecallResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/inventory/recalls",
            Some("recall-create"),
            Some(create_body.clone()),
        )
        .await,
    )
    .await;
    assert_eq!(replay, created);
    let changed = rig
        .send(
            Method::POST,
            "/api/v1/inventory/recalls",
            Some("recall-create"),
            Some(json!({
                "facility_id": rig.facility_id,
                "item_batch_id": rig.batch_id,
                "reason": "quality_concern",
                "note": "Changed body"
            })),
        )
        .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let projection: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT qty_on_hand, qty_reserved, qty_held FROM inventory_balances WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id",
    )
    .bind(rig.tenant_id.get())
    .bind(&rig.balance_ids)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(projection, vec![(7, 0, 7), (5, 0, 5)]);
    let evidence: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT COUNT(*)::BIGINT, SUM(link.held_qty)::BIGINT,
               COUNT(*) FILTER (WHERE hold.status='active')::BIGINT
        FROM inventory_recall_case_holds link
        JOIN inventory_holds hold ON hold.tenant_id=link.tenant_id AND hold.id=link.inventory_hold_id
        WHERE link.tenant_id=$1 AND link.recall_case_id=$2
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(created.recall_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence, (2, 12, 2));
    let first_hold: i64 = sqlx::query_scalar(
        "SELECT inventory_hold_id FROM inventory_recall_case_holds WHERE tenant_id=$1 AND recall_case_id=$2 ORDER BY id LIMIT 1",
    )
    .bind(rig.tenant_id.get())
    .bind(created.recall_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    let late_location = rig
        .fixture
        .location(rig.tenant_id, rig.facility_id, "RECALL-LATE")
        .await;
    let late_receipt = repo::inventory::receive_inventory(
        &rig.fixture.db,
        rig.tenant_id,
        rig.user_id,
        rig.batch_id,
        late_location,
        3,
        None,
        Some("must not escape active recall"),
        Some("inventory-recall-test"),
        Some(rig.batch_id),
        "recall-late-receipt",
    )
    .await;
    assert!(late_receipt.is_err());
    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let positive_positions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inventory_balances WHERE tenant_id=$1 AND facility_id=$2 AND item_batch_id=$3 AND deleted IS NULL AND qty_on_hand>0",
    )
    .bind(rig.tenant_id.get())
    .bind(rig.facility_id)
    .bind(rig.batch_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(positive_positions, 2);
    tx.rollback().await.unwrap();

    let generic_release = rig
        .send(
            Method::POST,
            &format!("/api/v1/inventory/holds/{first_hold}/releases"),
            Some("generic-release"),
            Some(json!({})),
        )
        .await;
    assert_eq!(generic_release.status(), StatusCode::CONFLICT);

    let released = rig
        .send(
            Method::POST,
            &format!("/api/v1/inventory/recalls/{}/releases", created.recall_id),
            Some("recall-release"),
            Some(json!({"expected_revision": 1})),
        )
        .await;
    if released.status() != StatusCode::OK {
        let status = released.status();
        let body = to_bytes(released.into_body(), 256 * 1024).await.unwrap();
        panic!(
            "release returned {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let released: InventoryRecallResponse = json_response(released).await;
    assert_eq!(
        released.status,
        wareboxes_api_contract::v1::InventoryRecallStatus::Released
    );
    assert_eq!(released.revision.get(), 2);
    let release_replay: InventoryRecallResponse = json_response(
        rig.send(
            Method::POST,
            &format!("/api/v1/inventory/recalls/{}/releases", created.recall_id),
            Some("recall-release"),
            Some(json!({"expected_revision": 1})),
        )
        .await,
    )
    .await;
    assert_eq!(release_replay, released);

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let projection: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT qty_on_hand, qty_held FROM inventory_balances WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id",
    )
    .bind(rig.tenant_id.get())
    .bind(&rig.balance_ids)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(projection, vec![(7, 0), (5, 0)]);
    let events: Vec<(String, i64)> = sqlx::query_as(
        "SELECT event_type, aggregate_sequence FROM outbox_events WHERE tenant_id=$1 AND aggregate_type='inventory_recall' AND aggregate_id=$2 ORDER BY aggregate_sequence",
    )
    .bind(rig.tenant_id.get())
    .bind(created.recall_id.to_string())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        events,
        vec![
            ("inventory.recall.created".to_owned(), 1),
            ("inventory.recall.released".to_owned(), 2)
        ]
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn recall_scope_cursor_rls_and_immutability_fail_closed() {
    let rig = Rig::new().await;
    let first: InventoryRecallResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/inventory/recalls",
            Some("scope-first"),
            Some(json!({
                "facility_id": rig.facility_id,
                "item_batch_id": rig.batch_id,
                "reason": "regulatory",
                "note": null
            })),
        )
        .await,
    )
    .await;
    let released = rig
        .send(
            Method::POST,
            &format!("/api/v1/inventory/recalls/{}/releases", first.recall_id),
            Some("scope-release"),
            Some(json!({"expected_revision": 1})),
        )
        .await;
    if released.status() != StatusCode::OK {
        let status = released.status();
        let body = to_bytes(released.into_body(), 256 * 1024).await.unwrap();
        panic!(
            "release returned {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let second: InventoryRecallResponse = json_response(
        rig.send(
            Method::POST,
            "/api/v1/inventory/recalls",
            Some("scope-second"),
            Some(json!({
                "facility_id": rig.facility_id,
                "item_batch_id": rig.batch_id,
                "reason": "regulatory",
                "note": null
            })),
        )
        .await,
    )
    .await;

    let page = rig
        .send(Method::GET, "/api/v1/inventory/recalls?limit=1", None, None)
        .await;
    assert_eq!(page.status(), StatusCode::OK);
    let page: InventoryRecallPage = json_response(page).await;
    assert_eq!(page.items[0].recall_id, second.recall_id);
    let cursor = page.next_cursor.unwrap();
    let changed = rig
        .send(
            Method::GET,
            &format!("/api/v1/inventory/recalls?status=active&limit=1&cursor={cursor}"),
            None,
            None,
        )
        .await;
    assert_eq!(changed.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = json_response(changed).await;
    assert_eq!(error.reason, ErrorReason::InvalidCursor);

    let other_user = rig
        .fixture
        .wms_user("inventory-recall-cross-tenant@test.local")
        .await;
    let other_tenant = tenant_for_user(&rig.fixture.db, other_user.id).await;
    grant_permission(
        &rig.fixture,
        other_tenant,
        other_user.id,
        "wms_supervisor",
        "inventory-recall-cross-tenant-supervisor",
    )
    .await;
    let other_token = wareboxes_api::auth::create_session(&rig.fixture.db, other_user.id)
        .await
        .unwrap();
    let guessed = rig
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/inventory/recalls/{}/releases",
                    second.recall_id
                ))
                .header(header::AUTHORIZATION, format!("Bearer {other_token}"))
                .header(TENANT_ID_HEADER, other_tenant.to_string())
                .header(IDEMPOTENCY_KEY_HEADER, "cross-tenant-release")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"expected_revision": 1}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guessed.status(), StatusCode::NOT_FOUND);
    let mut other_tx = tenant_tx(&rig.fixture.db, other_tenant).await;
    let cross_tenant_counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM inventory_recall_cases)::BIGINT, (SELECT COUNT(*) FROM inventory_recall_case_holds)::BIGINT",
    )
    .fetch_one(&mut *other_tx)
    .await
    .unwrap();
    assert_eq!(cross_tenant_counts, (0, 0));
    other_tx.rollback().await.unwrap();

    let admin = admin_db_for(&rig.fixture.db).await;
    let immutable = sqlx::query(
        "UPDATE inventory_recall_case_holds SET held_qty=held_qty+1 WHERE recall_case_id=$1",
    )
    .bind(second.recall_id)
    .execute(&admin)
    .await;
    assert!(immutable.is_err());
    let forced: Vec<(String, bool)> = sqlx::query_as(
        "SELECT relname, relforcerowsecurity FROM pg_class WHERE relname=ANY($1) ORDER BY relname",
    )
    .bind(vec![
        "inventory_recall_case_holds",
        "inventory_recall_cases",
    ])
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(
        forced,
        vec![
            ("inventory_recall_case_holds".to_owned(), true),
            ("inventory_recall_cases".to_owned(), true)
        ]
    );
    let grants: (bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT
          has_table_privilege('wareboxes_app','inventory_recall_cases','SELECT,INSERT'),
          has_table_privilege('wareboxes_app','inventory_recall_cases','DELETE'),
          has_table_privilege('wareboxes_app','inventory_recall_case_holds','SELECT,INSERT'),
          has_table_privilege('wareboxes_app','inventory_recall_case_holds','UPDATE,DELETE')
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(grants, (true, false, true, false));
    admin.close().await;

    assert!(repo::tenants::update_user_access_scope(
        &rig.fixture.db,
        rig.tenant_id,
        &UpdateUserAccessScope {
            user_id: rig.user_id,
            all_facilities: false,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());
    let concealed = rig
        .send(
            Method::POST,
            &format!("/api/v1/inventory/recalls/{}/releases", second.recall_id),
            Some("scope-concealed"),
            Some(json!({"expected_revision": 1})),
        )
        .await;
    assert_eq!(concealed.status(), StatusCode::NOT_FOUND);

    for body in [
        json!({
            "facility_id": rig.facility_id,
            "item_batch_id": rig.batch_id,
            "reason": "regulatory",
            "note": null
        }),
        json!({
            "facility_id": rig.facility_id,
            "item_batch_id": rig.batch_id,
            "reason": "quality_concern",
            "note": "changed after scope revocation"
        }),
    ] {
        let replay = rig
            .send(
                Method::POST,
                "/api/v1/inventory/recalls",
                Some("scope-second"),
                Some(body),
            )
            .await;
        assert_eq!(replay.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn concurrent_recall_creation_has_one_exact_winner() {
    let rig = Rig::new().await;
    let body = json!({
        "facility_id": rig.facility_id,
        "item_batch_id": rig.batch_id,
        "reason": "quality_concern",
        "note": "Concurrent containment"
    });
    let first = rig.send(
        Method::POST,
        "/api/v1/inventory/recalls",
        Some("race-first"),
        Some(body.clone()),
    );
    let second = rig.send(
        Method::POST,
        "/api/v1/inventory/recalls",
        Some("race-second"),
        Some(body),
    );
    let (first, second) = tokio::join!(first, second);
    let mut statuses = [first.status(), second.status()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::CONFLICT]);

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let effects: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM inventory_recall_cases WHERE tenant_id=$1)::BIGINT,
          (SELECT COUNT(*) FROM inventory_recall_case_holds WHERE tenant_id=$1)::BIGINT,
          (SELECT COUNT(*) FROM inventory_holds WHERE tenant_id=$1 AND reference_type='inventory_recall' AND status='active')::BIGINT
        "#,
    )
    .bind(rig.tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(effects, (1, 2, 2));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn recall_creation_and_late_receipt_complete_without_deadlock_or_partial_coverage() {
    let rig = Rig::new().await;
    let late_location = rig
        .fixture
        .location(rig.tenant_id, rig.facility_id, "RECALL-RACE-LATE")
        .await;
    let create = rig.send(
        Method::POST,
        "/api/v1/inventory/recalls",
        Some("recall-receipt-race"),
        Some(json!({
            "facility_id": rig.facility_id,
            "item_batch_id": rig.batch_id,
            "reason": "supplier_notice",
            "note": "Race containment"
        })),
    );
    let receive = repo::inventory::receive_inventory(
        &rig.fixture.db,
        rig.tenant_id,
        rig.user_id,
        rig.batch_id,
        late_location,
        3,
        None,
        Some("concurrent recall receipt"),
        Some("inventory-recall-test"),
        Some(rig.batch_id),
        "recall-concurrent-receipt",
    );
    let (create, receive) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(create, receive)
    })
    .await
    .expect("recall and receipt race must not deadlock");
    assert!(matches!(
        create.status(),
        StatusCode::OK | StatusCode::CONFLICT
    ));
    assert!(create.status() == StatusCode::OK || receive.is_ok());

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let case: Option<(i64, i32, i64)> = sqlx::query_as(
        "SELECT id, affected_position_count, held_qty FROM inventory_recall_cases WHERE tenant_id=$1 AND state='active'",
    )
    .bind(rig.tenant_id.get())
    .fetch_optional(&mut *tx)
    .await
    .unwrap();
    if let Some((case_id, expected_count, expected_qty)) = case {
        let exact: (i64, i64, i64, i64) = sqlx::query_as(
            r#"
            SELECT COUNT(*)::BIGINT, COALESCE(SUM(balance.qty_on_hand),0)::BIGINT,
                   COALESCE(SUM(balance.qty_held),0)::BIGINT, COUNT(link.id)::BIGINT
            FROM inventory_balances balance
            LEFT JOIN inventory_recall_case_holds link
              ON link.tenant_id=balance.tenant_id
             AND link.inventory_balance_id=balance.id
             AND link.recall_case_id=$2
            WHERE balance.tenant_id=$1 AND balance.facility_id=$3
              AND balance.item_batch_id=$4 AND balance.deleted IS NULL
              AND balance.qty_on_hand>0
            "#,
        )
        .bind(rig.tenant_id.get())
        .bind(case_id)
        .bind(rig.facility_id)
        .bind(rig.batch_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(exact.0, i64::from(expected_count));
        assert_eq!(exact.1, expected_qty);
        assert_eq!(exact.2, expected_qty);
        assert_eq!(exact.3, i64::from(expected_count));
    } else {
        assert!(receive.is_ok());
    }
    tx.rollback().await.unwrap();
}
