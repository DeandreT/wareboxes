use axum::http::{Method, StatusCode};
use wareboxes_api_contract::v1::PutawayCandidatePage;
use wareboxes_core::models::InventoryRelocationConfirmationResult;

use super::support::{change_json, command, json, Rig};

#[tokio::test]
async fn directed_putaway_and_relocation_freeze_and_move_the_entire_tree() {
    let rig = Rig::new("directed").await;
    change_json(
        rig.change_parent(
            rig.child_id,
            Some(rig.parent_id),
            0,
            "Load case onto pallet",
            "directed-attach-child",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    change_json(
        rig.change_parent(
            rig.grandchild_id,
            Some(rig.child_id),
            0,
            "Load inner container into case",
            "directed-attach-inner",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let balance_id = rig.receive_into_plate(rig.child_id, "directed").await;
    let access = crate::common::default_tenant_for_user(&rig.fixture.db, rig.user_id)
        .await
        .expect("WMS user has tenant access");
    let candidates: PutawayCandidatePage = json(
        rig.send::<serde_json::Value>(
            Method::GET,
            "/api/v1/putaway-candidates?workflow=license_plate&limit=100",
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let candidate = candidates
        .items
        .iter()
        .find(|candidate| candidate.license_plate_id == Some(rig.parent_id))
        .expect("the root container is the discoverable putaway candidate");
    assert_eq!(candidate.balance_count, 1);
    assert_eq!(candidate.available_quantity, 12);
    assert!(candidates
        .items
        .iter()
        .all(|candidate| candidate.license_plate_id != Some(rig.child_id)));

    let putaway_task_id = wareboxes_api::repo::tasks::create_license_plate_putaway_task_in_scope(
        &rig.fixture.db,
        &access,
        &command(&access, "directed-putaway-create"),
        rig.parent_id,
        rig.destination_location_id,
        50,
        Some(rig.user_id),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let mut tx = crate::common::tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let snapshot: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT license_plate_id,content_license_plate_id,inventory_balance_id
        FROM license_plate_putaway_task_contents
        WHERE tenant_id=$1 AND task_id=$2
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(putaway_task_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(snapshot, (rig.parent_id, rig.child_id, balance_id));

    assert!(wareboxes_api::repo::tasks::start_task_in_scope(
        &rig.fixture.db,
        &access,
        &command(&access, "directed-putaway-start"),
        putaway_task_id,
    )
    .await
    .unwrap());
    assert_eq!(
        rig.change_parent(
            rig.child_id,
            None,
            1,
            "Unsafe detach during movement",
            "directed-detach-active",
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let putaway = wareboxes_api::repo::tasks::confirm_license_plate_putaway_in_scope(
        &rig.fixture.db,
        &access,
        &command(&access, "directed-putaway-confirm"),
        putaway_task_id,
        "PALLET-directed",
        "LPN-HIERARCHY-DEST-directed",
    )
    .await
    .unwrap();
    assert_eq!(putaway.moved_balance_count, 1);

    let relocation_barcode = "LPN-HIERARCHY-RELOCATE-directed";
    let relocation_location_id = rig
        .fixture
        .location(rig.tenant_id, rig.facility_id, relocation_barcode)
        .await;
    let relocation_task_id =
        wareboxes_api::repo::tasks::create_license_plate_inventory_relocation_task_in_scope(
            &rig.fixture.db,
            &access,
            &command(&access, "directed-relocation-create"),
            rig.parent_id,
            relocation_location_id,
            50,
            Some(rig.user_id),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(wareboxes_api::repo::tasks::start_task_in_scope(
        &rig.fixture.db,
        &access,
        &command(&access, "directed-relocation-start"),
        relocation_task_id,
    )
    .await
    .unwrap());
    let relocation = wareboxes_api::repo::tasks::confirm_inventory_relocation_in_scope(
        &rig.fixture.db,
        &access,
        &command(&access, "directed-relocation-confirm"),
        relocation_task_id,
        relocation_barcode,
        Some("PALLET-directed"),
    )
    .await
    .unwrap();
    assert!(matches!(
        relocation.result,
        InventoryRelocationConfirmationResult::LicensePlate {
            license_plate_id,
            moved_balance_count: 1,
            ..
        } if license_plate_id == rig.parent_id
    ));

    let mut tx = crate::common::tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let plate_locations: Vec<Option<i64>> = sqlx::query_scalar(
        "SELECT location_id FROM license_plates WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id",
    )
    .bind(rig.tenant_id.get())
    .bind(vec![rig.parent_id, rig.child_id, rig.grandchild_id])
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(plate_locations, vec![Some(relocation_location_id); 3]);
    let balance_location: i64 = sqlx::query_scalar(
        "SELECT location_id FROM inventory_balances WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(balance_location, relocation_location_id);
    let leaf_entries: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM inventory_entries
        WHERE tenant_id=$1 AND license_plate_id=$2
          AND transaction_id IN ($3,$4)
        "#,
    )
    .bind(rig.tenant_id.get())
    .bind(rig.child_id)
    .bind(putaway.inventory_transaction_id)
    .bind(relocation.inventory_transaction_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(leaf_entries, 4);
}
