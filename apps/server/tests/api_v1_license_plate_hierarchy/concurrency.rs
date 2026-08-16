use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::json;
use tokio::sync::Barrier;

use super::support::{add_plate, Rig};

#[tokio::test]
async fn concurrent_parent_changes_have_one_winner_and_one_audit_event() {
    let rig = Rig::new("concurrency").await;
    let competing_parent_id = add_plate(
        &rig.fixture,
        rig.tenant_id,
        rig.inventory_owner_id,
        rig.facility_id,
        rig.source_location_id,
        "PALLET-CONCURRENCY-B",
    )
    .await;
    let target_id = add_plate(
        &rig.fixture,
        rig.tenant_id,
        rig.inventory_owner_id,
        rig.facility_id,
        rig.source_location_id,
        "CASE-CONCURRENCY-TARGET",
    )
    .await;
    let barrier = Arc::new(Barrier::new(3));
    let mut joins = Vec::new();
    for (parent_id, key) in [
        (rig.parent_id, "attach-race-a"),
        (competing_parent_id, "attach-race-b"),
    ] {
        let app = rig.app.clone();
        let token = rig.token.clone();
        let tenant_id = rig.tenant_id;
        let barrier = barrier.clone();
        joins.push(tokio::spawn(async move {
            barrier.wait().await;
            use tower::ServiceExt;
            app.oneshot(super::support::request(
                &token,
                tenant_id,
                axum::http::Method::POST,
                &format!("/api/v1/license-plates/{target_id}/parent-changes"),
                Some(key),
                Some(
                    &wareboxes_api_contract::v1::ChangeLicensePlateParentRequest {
                        parent_license_plate_id: Some(parent_id),
                        expected_revision: 0,
                        reason: "Concurrent palletization".into(),
                    },
                ),
            ))
            .await
            .unwrap()
        }));
    }
    barrier.wait().await;
    let mut statuses = Vec::new();
    for join in joins {
        statuses.push(join.await.unwrap().status());
    }
    statuses.sort();
    assert_eq!(statuses, vec![StatusCode::OK, StatusCode::CONFLICT]);

    let mut tx = crate::common::tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let state: (Option<i64>, i64) = sqlx::query_as(
        "SELECT parent_license_plate_id,hierarchy_revision FROM license_plates WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(target_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(matches!(state.0, Some(id) if id==rig.parent_id || id==competing_parent_id));
    assert_eq!(state.1, 1);
    let effects: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM license_plate_hierarchy_events
           WHERE tenant_id=$1 AND child_license_plate_id=$2),
          (SELECT COUNT(*) FROM outbox_events
           WHERE tenant_id=$1 AND aggregate_type='license_plate' AND aggregate_id=$2::TEXT),
          (SELECT COUNT(*) FROM command_idempotency_records
           WHERE tenant_id=$1 AND operation='change_license_plate_parent'
             AND idempotency_key IN ('attach-race-a','attach-race-b'))
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(target_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (1, 1, 1));
}

#[tokio::test]
async fn concurrent_attachment_and_root_move_never_split_a_hierarchy() {
    let rig = Rig::new("move-race").await;
    rig.receive_into_plate(rig.parent_id, "move-race").await;
    let barrier = Arc::new(Barrier::new(3));

    let attach_app = rig.app.clone();
    let attach_token = rig.token.clone();
    let tenant_id = rig.tenant_id;
    let child_id = rig.child_id;
    let parent_id = rig.parent_id;
    let attach_barrier = barrier.clone();
    let attach = tokio::spawn(async move {
        attach_barrier.wait().await;
        use tower::ServiceExt;
        attach_app
            .oneshot(super::support::request(
                &attach_token,
                tenant_id,
                axum::http::Method::POST,
                &format!("/api/v1/license-plates/{child_id}/parent-changes"),
                Some("attach-during-move"),
                Some(
                    &wareboxes_api_contract::v1::ChangeLicensePlateParentRequest {
                        parent_license_plate_id: Some(parent_id),
                        expected_revision: 0,
                        reason: "Concurrent palletization".into(),
                    },
                ),
            ))
            .await
            .unwrap()
    });

    let move_app = rig.app.clone();
    let move_token = rig.token.clone();
    let destination_location_id = rig.destination_location_id;
    let move_barrier = barrier.clone();
    let movement = tokio::spawn(async move {
        move_barrier.wait().await;
        use tower::ServiceExt;
        move_app
            .oneshot(super::support::request(
                &move_token,
                tenant_id,
                axum::http::Method::POST,
                "/api/license-plates/move",
                None,
                Some(&json!({
                    "license_plate_id":parent_id,
                    "to_location_id":destination_location_id,
                    "reason":"Concurrent root move",
                    "idempotency_key":"move-during-attach"
                })),
            ))
            .await
            .unwrap()
    });
    barrier.wait().await;
    let attach_status = attach.await.unwrap().status();
    let move_status = movement.await.unwrap().status();
    assert!(matches!(
        attach_status,
        StatusCode::OK | StatusCode::CONFLICT
    ));
    assert_eq!(move_status, StatusCode::OK);

    let mut tx = crate::common::tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let state: (Option<i64>, Option<i64>, Option<i64>) = sqlx::query_as(
        r#"
        SELECT child.parent_license_plate_id,root.location_id,child.location_id
        FROM license_plates child
        JOIN license_plates root ON root.tenant_id=child.tenant_id AND root.id=$3
        WHERE child.tenant_id=$1 AND child.id=$2
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.child_id)
    .bind(rig.parent_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(state.1, Some(rig.destination_location_id));
    if attach_status == StatusCode::OK {
        assert_eq!(state.0, Some(rig.parent_id));
        assert_eq!(state.2, Some(rig.destination_location_id));
    } else {
        assert_eq!(state.0, None);
        assert_eq!(state.2, Some(rig.source_location_id));
    }
}
