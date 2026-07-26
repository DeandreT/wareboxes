mod common;

use common::*;
use wareboxes_domain::CommandContext;

#[tokio::test]
async fn order_and_load_mutations_write_activity_history() {
    let db = setup().await;
    let user = auth::register_user(&db, "activity@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let inventory_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Activity InventoryOwner",
        "activity@test",
    )
    .await
    .unwrap();

    repo::orders::add_order(&db, tenant_id, &new_order("ACT-ORDER", inventory_owner))
        .await
        .unwrap();
    let order_id = repo::orders::get_orders(&db, tenant_id).await.unwrap()[0].id;
    let update = OrderUpdate {
        order_id,
        order_key: None,
        rush: Some(true),
        ship_by: None,
        line1: None,
        line2: None,
        city: None,
        state: None,
        postal_code: None,
        country: None,
    };
    let access = repo::tenants::access_for_user(&db, user.id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    let command = CommandContext {
        tenant_id,
        actor_id: access.user_id,
        request_id: "activity-order-update".to_owned(),
        idempotency_key: Some("activity-order-update".to_owned()),
    };
    assert!(
        repo::orders::update_order_metadata(&db, &access, &command, &update)
            .await
            .unwrap()
    );
    assert!(repo::orders::delete_order(&db, tenant_id, order_id)
        .await
        .unwrap());
    assert!(repo::orders::restore_order(&db, tenant_id, order_id)
        .await
        .unwrap());

    let mut tx = tenant_tx(&db, tenant_id).await;
    let order_actions = sqlx::query_scalar::<_, String>(
        "SELECT action FROM order_activity WHERE order_id = $1 ORDER BY id",
    )
    .bind(order_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        order_actions,
        vec![
            "created order",
            "updated order metadata",
            "deleted order",
            "restored order",
        ]
    );

    let facility = repo::facilities::add_facility(&db, tenant_id, "Activity DC")
        .await
        .unwrap();
    let load_id = repo::loads::add_load(
        &db,
        tenant_id,
        user.id,
        facility,
        inventory_owner,
        LoadType::Inbound,
        Some("ACT-LOAD"),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(repo::loads::update_load(
        &db,
        tenant_id,
        user.id,
        load_id,
        Some(LoadStatus::Arrived),
        None,
        None,
        Some("INV-ACT"),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap());
    let note_id = repo::loads::add_note(&db, tenant_id, user.id, load_id, "activity note")
        .await
        .unwrap();
    assert!(
        repo::loads::set_load_note_deleted(&db, tenant_id, user.id, note_id, true)
            .await
            .unwrap()
    );
    assert!(
        repo::loads::set_load_deleted(&db, tenant_id, user.id, load_id, true)
            .await
            .unwrap()
    );

    let mut tx = tenant_tx(&db, tenant_id).await;
    let load_actions = sqlx::query_scalar::<_, String>(
        "SELECT action FROM load_activity WHERE load_id = $1 ORDER BY id",
    )
    .bind(load_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        load_actions,
        vec![
            "created",
            "updated",
            "note_added",
            "note_deleted",
            "deleted"
        ]
    );
}
