use axum::http::{Method, StatusCode};
use serde_json::json;
use wareboxes_api_contract::v1::LicensePlateHierarchyResponse;

use super::support::{change_json, json, Rig};

#[tokio::test]
async fn nested_license_plates_are_auditable_replay_safe_and_move_as_one_tree() {
    let rig = Rig::new("lifecycle").await;

    let attached = change_json(
        rig.change_parent(
            rig.child_id,
            Some(rig.parent_id),
            0,
            "Loaded case onto outbound pallet",
            "attach-child",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(attached.resulting_revision, 1);
    assert_eq!(attached.root_license_plate_id, rig.parent_id);
    assert_eq!(attached.depth, 1);

    let replay = change_json(
        rig.change_parent(
            rig.child_id,
            Some(rig.parent_id),
            0,
            "Loaded case onto outbound pallet",
            "attach-child",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, attached);
    assert_eq!(
        rig.change_parent(
            rig.child_id,
            Some(rig.grandchild_id),
            0,
            "Different request",
            "attach-child",
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    change_json(
        rig.change_parent(
            rig.grandchild_id,
            Some(rig.child_id),
            0,
            "Packed inner container into case",
            "attach-grandchild",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let balance_id = rig.receive_into_plate(rig.child_id, "nested-stock").await;
    assert_eq!(
        rig.change_parent(
            rig.parent_id,
            Some(rig.grandchild_id),
            0,
            "Attempted cycle",
            "cycle",
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let hierarchy: LicensePlateHierarchyResponse = json(
        rig.send::<serde_json::Value>(
            Method::GET,
            &format!("/api/v1/license-plates/{}/hierarchy", rig.parent_id),
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        hierarchy.node.descendant_ids,
        vec![rig.child_id, rig.grandchild_id]
    );
    assert_eq!(hierarchy.node.contained_unit_quantity, 12);
    assert_eq!(
        hierarchy.descendants[0].parent_license_plate_id,
        Some(rig.parent_id)
    );

    let child_move = rig
        .send(
            Method::POST,
            "/api/license-plates/move",
            None,
            Some(&json!({
                "license_plate_id":rig.child_id,
                "to_location_id":rig.destination_location_id,
                "reason":"Move nested child independently",
                "idempotency_key":"move-child"
            })),
        )
        .await;
    assert_eq!(child_move.status(), StatusCode::CONFLICT);

    let _: i64 = json(
        rig.send(
            Method::POST,
            "/api/license-plates/move",
            None,
            Some(&json!({
                "license_plate_id":rig.parent_id,
                "to_location_id":rig.destination_location_id,
                "reason":"Move consolidated pallet",
                "idempotency_key":"move-root"
            })),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let mut tx = crate::common::tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let locations: Vec<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT id,location_id FROM license_plates WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id",
    )
    .bind(rig.tenant_id.get())
    .bind(vec![rig.parent_id, rig.child_id, rig.grandchild_id])
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert!(locations
        .iter()
        .all(|(_, location)| *location == Some(rig.destination_location_id)));
    let balance_location: i64 = sqlx::query_scalar(
        "SELECT location_id FROM inventory_balances WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(balance_location, rig.destination_location_id);
    let child_entry_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inventory_entries WHERE tenant_id=$1 AND license_plate_id=$2 AND transaction_id=(SELECT MAX(id) FROM inventory_transactions WHERE tenant_id=$1)",
    )
    .bind(rig.tenant_id.get())
    .bind(rig.child_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(child_entry_count, 2);
    tx.rollback().await.unwrap();

    let detached = change_json(
        rig.change_parent(
            rig.child_id,
            None,
            1,
            "Removed case subtree from pallet",
            "detach-child",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(detached.resulting_revision, 2);
    assert_eq!(detached.root_license_plate_id, rig.child_id);
    assert_eq!(detached.depth, 0);
    let detached_replay = change_json(
        rig.change_parent(
            rig.child_id,
            None,
            1,
            "Removed case subtree from pallet",
            "detach-child",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(detached_replay, detached);

    let child_hierarchy: LicensePlateHierarchyResponse = json(
        rig.send::<serde_json::Value>(
            Method::GET,
            &format!("/api/v1/license-plates/{}/hierarchy", rig.child_id),
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(child_hierarchy.events.len(), 2);
    assert_eq!(child_hierarchy.node.descendant_ids, vec![rig.grandchild_id]);
    let mut tx = crate::common::tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let effects: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM license_plate_hierarchy_events
           WHERE tenant_id=$1 AND child_license_plate_id=$2),
          (SELECT COUNT(*) FROM outbox_events
           WHERE tenant_id=$1 AND aggregate_type='license_plate'
             AND aggregate_id=$2::TEXT AND event_type LIKE 'inventory.license_plate.%'),
          (SELECT COUNT(*) FROM command_idempotency_records
           WHERE tenant_id=$1 AND operation='change_license_plate_parent'
             AND idempotency_key IN ('attach-child','detach-child'))
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.child_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(effects, (2, 2, 2));
}
