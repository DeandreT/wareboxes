mod common;

use common::*;
use wareboxes_application::order_amendment::AmendFulfillmentOrderCommand;
use wareboxes_application::CommandContext;
use wareboxes_domain::{OrderId, OrderRevision, ShippingDestination, ShippingRecipient};

#[tokio::test]
async fn order_and_load_mutations_write_activity_history() {
    let db = setup().await;
    let user = auth::register_user(&db, "activity@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let permission =
        match wareboxes_persistence_postgres::permissions::find_by_name(&db, tenant_id, "orders")
            .await
            .unwrap()
        {
            Some(permission) => permission.id,
            None => wareboxes_persistence_postgres::permissions::add_permission(
                &db,
                tenant_id,
                "orders",
                Some("Orders"),
            )
            .await
            .unwrap(),
        };
    let role =
        wareboxes_persistence_postgres::roles::add_role(&db, tenant_id, "activity-orders", None)
            .await
            .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(&db, tenant_id, role, permission)
        .await
        .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(&db, tenant_id, user.id, role)
        .await
        .unwrap();
    let inventory_owner = repo::inventory_owners::add_inventory_owner(
        &db,
        tenant_id,
        "Activity InventoryOwner",
        "activity@test",
    )
    .await
    .unwrap();

    let order_id = insert_test_order_header(&db, tenant_id, "ACT-ORDER", inventory_owner).await;
    let update = AmendFulfillmentOrderCommand::new(
        OrderId::new(order_id).unwrap(),
        OrderRevision::new(1).unwrap(),
        true,
        None,
        ShippingDestination::new(
            ShippingRecipient::new("Test Recipient", None, None, None).unwrap(),
            "1 Main St",
            None,
            "Reno",
            "NV",
            "89501",
            "US",
        )
        .unwrap(),
    );
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
    repo::order_amendment::amend_fulfillment_order_header(&db, &access, &command, &update)
        .await
        .unwrap();
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
            "amended fulfillment order header",
            "deleted order",
            "restored order",
        ]
    );

    let facility =
        wareboxes_persistence_postgres::facilities::add_facility(&db, tenant_id, "Activity DC")
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
