mod common;

use common::*;

#[tokio::test]
async fn order_status_guards_and_soft_delete() {
    let db = setup().await;

    assert!(repo::orders::add_order(&db, &new_order("O1", None))
        .await
        .unwrap());
    let orders = repo::orders::get_orders(&db).await.unwrap();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].status, OrderStatus::Open);
    let id = orders[0].id;

    // 'open' is mutable & not closed/confirmed -> deletable.
    assert!(repo::orders::delete_order(&db, id).await.unwrap());
    assert!(repo::orders::get_orders(&db).await.unwrap().is_empty());

    // A shipped order is no longer mutable or deletable.
    assert!(repo::orders::add_order(&db, &new_order("O2", None))
        .await
        .unwrap());
    let id2 = repo::orders::get_orders(&db).await.unwrap()[0].id;
    let to_shipped = OrderUpdate {
        order_id: id2,
        order_key: None,
        status: Some(OrderStatus::Shipped),
        rush: None,
        confirmed: None,
        closed: None,
        ship_by: None,
        wave_id: None,
        inventory_owner_id: None,
        line1: None,
        line2: None,
        city: None,
        state: None,
        postal_code: None,
        country: None,
    };
    // 'open' -> allowed.
    assert!(repo::orders::update_order(&db, &to_shipped).await.unwrap());
    // Now 'shipped': further update is rejected by the status guard.
    let mut again = to_shipped.clone();
    again.status = Some(OrderStatus::Held);
    assert!(!repo::orders::update_order(&db, &again).await.unwrap());
    assert!(!repo::orders::delete_order(&db, id2).await.unwrap());
}

#[tokio::test]
async fn order_pagination_filters_and_reports_total() {
    let db = setup().await;

    for key in ["PAGE-A", "PAGE-B", "OTHER-C"] {
        repo::orders::add_order(&db, &new_order(key, None))
            .await
            .unwrap();
    }

    let page = repo::orders::get_orders_page(&db, 2, 0, None, Some("PAGE"))
        .await
        .unwrap();
    assert_eq!(page.page.total, 2);
    assert_eq!(page.page.limit, 2);
    assert_eq!(page.page.offset, 0);
    assert_eq!(page.page.items.len(), 2);
    assert!(page
        .page
        .items
        .iter()
        .all(|order| order.order_key.starts_with("PAGE")));
    assert_eq!(page.summaries.len(), 1);
    assert_eq!(page.summaries[0].key, "open");
    assert_eq!(page.summaries[0].count, 2);

    let second_page = repo::orders::get_orders_page(&db, 2, 2, None, None)
        .await
        .unwrap();
    assert_eq!(second_page.page.total, 3);
    assert_eq!(second_page.page.items.len(), 1);

    let open_page = repo::orders::get_orders_page(&db, 10, 0, Some(OrderStatus::Open), None)
        .await
        .unwrap();
    assert_eq!(open_page.page.total, 3);
}

#[tokio::test]
async fn inventory_owner_delete_blocked_by_open_orders() {
    let db = setup().await;

    let user = auth::register_user(&db, "orders-owner@test.com", "supersecret", None, None)
        .await
        .unwrap();
    let tenant_id = tenant_for_user(&db, user.id).await;
    let acc = repo::inventory_owners::add_inventory_owner(&db, tenant_id, "Acme", "ops@acme.test")
        .await
        .unwrap();
    repo::orders::add_order(&db, &new_order("A1", Some(acc)))
        .await
        .unwrap();

    let err = repo::inventory_owners::delete_inventory_owner(&db, tenant_id, acc)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    // Ship the order, then deletion is allowed.
    let oid = repo::orders::get_orders(&db).await.unwrap()[0].id;
    let upd = OrderUpdate {
        order_id: oid,
        order_key: None,
        status: Some(OrderStatus::Shipped),
        rush: None,
        confirmed: None,
        closed: None,
        ship_by: None,
        wave_id: None,
        inventory_owner_id: None,
        line1: None,
        line2: None,
        city: None,
        state: None,
        postal_code: None,
        country: None,
    };
    assert!(repo::orders::update_order(&db, &upd).await.unwrap());
    assert!(
        repo::inventory_owners::delete_inventory_owner(&db, tenant_id, acc)
            .await
            .unwrap()
    );
}
