mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_core::models::Order;
use wareboxes_server::auth::TENANT_ID_HEADER;
use wareboxes_server::{routes, state::AppState};

#[tokio::test]
async fn order_status_guards_and_soft_delete() {
    let fixture = Fixture::new().await;
    let db = &fixture.db;
    let user = fixture.user("order-guards@test.com").await;
    let tenant_id = tenant_for_user(db, user.id).await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Order Guards Owner")
        .await;

    assert!(
        repo::orders::add_order(db, tenant_id, &new_order("O1", owner_id))
            .await
            .unwrap()
    );
    let orders = repo::orders::get_orders(db, tenant_id).await.unwrap();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].status, OrderStatus::Open);
    let id = orders[0].id;

    // 'open' is mutable & not closed/confirmed -> deletable.
    assert!(repo::orders::delete_order(db, tenant_id, id).await.unwrap());
    assert!(repo::orders::get_orders(db, tenant_id)
        .await
        .unwrap()
        .is_empty());

    // A shipped order is no longer mutable or deletable.
    assert!(
        repo::orders::add_order(db, tenant_id, &new_order("O2", owner_id))
            .await
            .unwrap()
    );
    let id2 = repo::orders::get_orders(db, tenant_id).await.unwrap()[0].id;
    let to_shipped = OrderUpdate {
        order_id: id2,
        order_key: None,
        status: Some(OrderStatus::Shipped),
        rush: None,
        confirmed: None,
        closed: None,
        ship_by: None,
        wave_id: None,
        line1: None,
        line2: None,
        city: None,
        state: None,
        postal_code: None,
        country: None,
    };
    // 'open' -> allowed.
    assert!(repo::orders::update_order(db, tenant_id, &to_shipped)
        .await
        .unwrap());
    // Now 'shipped': further update is rejected by the status guard.
    let mut again = to_shipped.clone();
    again.status = Some(OrderStatus::Held);
    assert!(!repo::orders::update_order(db, tenant_id, &again)
        .await
        .unwrap());
    assert!(!repo::orders::delete_order(db, tenant_id, id2)
        .await
        .unwrap());
}

#[tokio::test]
async fn order_pagination_filters_and_reports_total() {
    let fixture = Fixture::new().await;
    let db = &fixture.db;
    let user = fixture.user("order-pages@test.com").await;
    let tenant_id = tenant_for_user(db, user.id).await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Fulfillment Owner")
        .await;

    for key in ["PAGE-A", "PAGE-B", "OTHER-C"] {
        repo::orders::add_order(db, tenant_id, &new_order(key, owner_id))
            .await
            .unwrap();
    }

    let page = repo::orders::get_orders_page(db, tenant_id, 2, 0, None, Some("PAGE"))
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

    let second_page = repo::orders::get_orders_page(db, tenant_id, 2, 2, None, None)
        .await
        .unwrap();
    assert_eq!(second_page.page.total, 3);
    assert_eq!(second_page.page.items.len(), 1);

    let open_page =
        repo::orders::get_orders_page(db, tenant_id, 10, 0, Some(OrderStatus::Open), None)
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
    repo::orders::add_order(&db, tenant_id, &new_order("A1", acc))
        .await
        .unwrap();

    let err = repo::inventory_owners::delete_inventory_owner(&db, tenant_id, acc)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Core(CoreError::Conflict(_))));

    // Ship the order, then deletion is allowed.
    let oid = repo::orders::get_orders(&db, tenant_id).await.unwrap()[0].id;
    let upd = OrderUpdate {
        order_id: oid,
        order_key: None,
        status: Some(OrderStatus::Shipped),
        rush: None,
        confirmed: None,
        closed: None,
        ship_by: None,
        wave_id: None,
        line1: None,
        line2: None,
        city: None,
        state: None,
        postal_code: None,
        country: None,
    };
    assert!(repo::orders::update_order(&db, tenant_id, &upd)
        .await
        .unwrap());
    assert!(
        repo::inventory_owners::delete_inventory_owner(&db, tenant_id, acc)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn order_aggregate_is_isolated_by_selected_tenant() {
    let fixture = Fixture::new().await;
    let operator = fixture.user("order-scope-operator@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, operator.id).await;
    let second_user = fixture.user("order-scope-second@test.com").await;
    let tenant_b = tenant_for_user(&fixture.db, second_user.id).await;
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, is_default) VALUES ($1, $2, FALSE)",
    )
    .bind(tenant_b.get())
    .bind(operator.id)
    .execute(&fixture.db)
    .await
    .unwrap();

    let permission = repo::permissions::add_permission(&fixture.db, "orders", Some("Orders"))
        .await
        .unwrap();
    let role = repo::roles::add_role(&fixture.db, "order-scope-role", Some("Order scope tests"))
        .await
        .unwrap();
    repo::roles::add_role_permission(&fixture.db, role, permission)
        .await
        .unwrap();
    repo::roles::add_role_to_user(&fixture.db, operator.id, role)
        .await
        .unwrap();

    let owner_a = fixture.inventory_owner(tenant_a, "Tenant A Owner").await;
    let owner_b = fixture.inventory_owner(tenant_b, "Tenant B Owner").await;
    let order_a = fixture.order(tenant_a, "TENANT-A-ORDER", owner_a).await;
    let order_b = fixture.order(tenant_b, "TENANT-B-ORDER", owner_b).await;

    assert!(repo::orders::get_order(&fixture.db, tenant_b, order_a)
        .await
        .unwrap()
        .is_none());
    assert!(!repo::orders::delete_order(&fixture.db, tenant_b, order_a)
        .await
        .unwrap());
    assert_eq!(
        repo::orders::get_orders(&fixture.db, tenant_a)
            .await
            .unwrap()
            .into_iter()
            .map(|order| order.id)
            .collect::<Vec<_>>(),
        vec![order_a]
    );
    assert_eq!(
        repo::orders::get_orders(&fixture.db, tenant_b)
            .await
            .unwrap()
            .into_iter()
            .map(|order| order.id)
            .collect::<Vec<_>>(),
        vec![order_b]
    );

    let token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let missing_tenant = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/orders/{order_a}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_tenant.status(), StatusCode::BAD_REQUEST);

    let cross_tenant = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/orders/{order_a}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_b.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_tenant.status(), StatusCode::OK);
    let body = to_bytes(cross_tenant.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        serde_json::Value::Null
    );

    let selected_tenant = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/orders/{order_a}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(TENANT_ID_HEADER, tenant_a.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected_tenant.status(), StatusCode::OK);
    let body = to_bytes(selected_tenant.into_body(), usize::MAX)
        .await
        .unwrap();
    let order = serde_json::from_slice::<Option<Order>>(&body)
        .unwrap()
        .unwrap();
    assert_eq!(order.id, order_a);
    assert_eq!(order.tenant_id, tenant_a);
}

#[tokio::test]
async fn concurrent_order_delete_and_restore_have_single_winners() {
    let fixture = Fixture::new().await;
    let user = fixture.user("order-concurrency@test.com").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let owner_id = fixture
        .inventory_owner(tenant_id, "Concurrent Order Owner")
        .await;
    let order_id = fixture.order(tenant_id, "CONCURRENT-ORDER", owner_id).await;

    let first_db = fixture.db.clone();
    let second_db = fixture.db.clone();
    let (first, second) = tokio::join!(
        repo::orders::delete_order(&first_db, tenant_id, order_id),
        repo::orders::delete_order(&second_db, tenant_id, order_id),
    );
    assert_ne!(first.unwrap(), second.unwrap());

    let first_db = fixture.db.clone();
    let second_db = fixture.db.clone();
    let (first, second) = tokio::join!(
        repo::orders::restore_order(&first_db, tenant_id, order_id),
        repo::orders::restore_order(&second_db, tenant_id, order_id),
    );
    assert_ne!(first.unwrap(), second.unwrap());

    let actions: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT action
        FROM order_activity
        WHERE tenant_id = $1 AND order_id = $2
        ORDER BY id
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id)
    .fetch_all(&fixture.db)
    .await
    .unwrap();
    assert_eq!(
        actions,
        vec!["created order", "deleted order", "restored order"]
    );
}
