mod common;

use common::*;

#[tokio::test]
async fn order_and_load_mutations_write_activity_history() {
    let db = setup().await;

    repo::orders::add_order(&db, &new_order("ACT-ORDER", None))
        .await
        .unwrap();
    let order_id = repo::orders::get_orders(&db).await.unwrap()[0].id;
    let update = OrderUpdate {
        order_id,
        order_key: None,
        status: Some(OrderStatus::Held),
        rush: None,
        confirmed: None,
        closed: None,
        ship_by: None,
        wave_id: None,
        account_id: None,
        line1: None,
        line2: None,
        city: None,
        state: None,
        postal_code: None,
        country: None,
    };
    assert!(repo::orders::update_order(&db, &update).await.unwrap());
    assert!(repo::orders::delete_order(&db, order_id).await.unwrap());
    assert!(repo::orders::restore_order(&db, order_id).await.unwrap());

    let order_actions = sqlx::query_scalar::<_, String>(
        "SELECT action FROM order_activity WHERE order_id = $1 ORDER BY id",
    )
    .bind(order_id)
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(
        order_actions,
        vec![
            "created order",
            "updated order status to held",
            "deleted order",
            "restored order",
        ]
    );

    let user = auth::register_user(&db, "activity@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let warehouse = repo::warehouses::add_warehouse(&db, "Activity DC")
        .await
        .unwrap();
    let account = repo::accounts::add_account(&db, "Activity Account", "activity@test")
        .await
        .unwrap();
    let load_id = repo::loads::add_load(
        &db,
        user.id,
        warehouse,
        account,
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
    let note_id = repo::loads::add_note(&db, user.id, load_id, "activity note")
        .await
        .unwrap();
    assert!(
        repo::loads::set_load_note_deleted(&db, user.id, note_id, true)
            .await
            .unwrap()
    );
    assert!(repo::loads::set_load_deleted(&db, user.id, load_id, true)
        .await
        .unwrap());

    let load_actions = sqlx::query_scalar::<_, String>(
        "SELECT action FROM load_activity WHERE load_id = $1 ORDER BY id",
    )
    .bind(load_id)
    .fetch_all(&db)
    .await
    .unwrap();
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
