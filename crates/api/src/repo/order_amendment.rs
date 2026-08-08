//! Atomic, optimistic pre-execution fulfillment-order amendments.

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::order_amendment::{
    AmendFulfillmentOrderCommand, AmendFulfillmentOrderResult, AMEND_FULFILLMENT_ORDER_OPERATION,
};
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    amend_fulfillment_order, AddressId, FulfillmentOrderHeader, InventoryOwnerId,
    OrderAmendmentError, OrderAmendmentId, OrderId, OrderRevision, OrderStatus,
    ShippingDestination, ShippingRecipient, TenantId, Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::address::{insert_address_tx, NewAddress};
use crate::repo::orders;

struct LockedOrder {
    inventory_owner_id: InventoryOwnerId,
    status: OrderStatus,
    revision: OrderRevision,
    address_id: AddressId,
    header: FulfillmentOrderHeader,
}

pub async fn amend_fulfillment_order_header(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &AmendFulfillmentOrderCommand,
) -> AppResult<AmendFulfillmentOrderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, AMEND_FULFILLMENT_ORDER_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "orders").await?;

    if let Some(result) = prepared
        .replayed::<AmendFulfillmentOrderResult>(&mut tx)
        .await?
    {
        require_replayed_amendment_visible_tx(&mut tx, access.tenant_id, &result, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order = lock_order_tx(&mut tx, access.tenant_id, command.order_id(), &scope).await?;
    if order.revision != command.expected_revision() {
        return Err(AppError::conflict(format!(
            "order revision changed from {} to {}",
            command.expected_revision().get(),
            order.revision.get()
        )));
    }
    let requested = FulfillmentOrderHeader::new(
        command.rush(),
        command.ship_by().copied(),
        command.destination().clone(),
    );
    let transition =
        amend_fulfillment_order(order.status, order.revision, &order.header, &requested)
            .map_err(map_transition_error)?;
    let amended_at = now_iso();
    let resulting_address_id = if order.header.destination() == command.destination() {
        order.address_id
    } else {
        insert_destination_tx(&mut tx, access.tenant_id, command.destination()).await?
    };
    update_order_tx(
        &mut tx,
        access.tenant_id,
        command,
        order.status,
        resulting_address_id,
    )
    .await?;
    let amendment_id = insert_amendment_tx(
        &mut tx,
        access.tenant_id,
        context,
        command,
        &order,
        resulting_address_id,
        transition.revision,
        amended_at,
    )
    .await?;
    orders::insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        command.order_id().get(),
        Some(context.actor_id.get()),
        "amended fulfillment order header",
    )
    .await?;

    let result = AmendFulfillmentOrderResult {
        amendment_id,
        order_id: command.order_id(),
        inventory_owner_id: order.inventory_owner_id,
        order_status: order.status,
        revision: transition.revision,
        rush: command.rush(),
        ship_by: command.ship_by().copied(),
        destination: command.destination().clone(),
        amended_by: context.actor_id,
        amended_at,
    };
    enqueue_amended_event_tx(&mut tx, access.tenant_id, &result).await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn lock_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    scope: &ScopeBindings,
) -> AppResult<LockedOrder> {
    let row = sqlx::query(
        r#"
        SELECT orders.inventory_owner_id, orders.status, orders.revision,
               orders.rush, orders.ship_by, orders.address_id,
               address.name, address.company, address.phone, address.email,
               address.line1, address.line2, address.city, address.state,
               address.postal_code, address.country
        FROM orders
        INNER JOIN addresses address
          ON address.tenant_id = orders.tenant_id
         AND address.id = orders.address_id
        WHERE orders.tenant_id = $1
          AND orders.id = $2
          AND orders.deleted IS NULL
          AND ($3 OR orders.inventory_owner_id = ANY($4))
        FOR UPDATE OF orders
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("order"))?;
    let status_value: String = row.try_get("status")?;
    let destination = ShippingDestination::new(
        ShippingRecipient::new(
            required_text(&row, "name", "recipient name")?,
            row.try_get("company")?,
            row.try_get("phone")?,
            row.try_get("email")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        row.try_get::<String, _>("line1")?,
        row.try_get("line2")?,
        required_text(&row, "city", "city")?,
        required_text(&row, "state", "region")?,
        required_text(&row, "postal_code", "postal code")?,
        row.try_get::<String, _>("country")?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(LockedOrder {
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status: OrderStatus::parse(&status_value)
            .ok_or_else(|| AppError::internal("order has an invalid status"))?,
        revision: OrderRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        address_id: AddressId::new(row.try_get("address_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        header: FulfillmentOrderHeader::new(
            row.try_get("rush")?,
            row.try_get("ship_by")?,
            destination,
        ),
    })
}

fn required_text(row: &sqlx::postgres::PgRow, column: &str, label: &str) -> AppResult<String> {
    row.try_get::<Option<String>, _>(column)?
        .ok_or_else(|| AppError::internal(format!("order destination is missing {label}")))
}

async fn insert_destination_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    destination: &ShippingDestination,
) -> AppResult<AddressId> {
    let recipient = destination.recipient();
    let id = insert_address_tx(
        tx,
        tenant_id,
        NewAddress {
            name: Some(recipient.name()),
            company: recipient.company(),
            line1: destination.line1(),
            line2: destination.line2(),
            city: Some(destination.city()),
            state: Some(destination.region()),
            postal_code: Some(destination.postal_code()),
            country: destination.country(),
            phone: recipient.phone(),
            email: recipient.email(),
        },
    )
    .await?;
    AddressId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn update_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: &AmendFulfillmentOrderCommand,
    status: OrderStatus,
    address_id: AddressId,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE orders
        SET rush = $1, ship_by = $2, address_id = $3, revision = revision + 1
        WHERE tenant_id = $4 AND id = $5 AND deleted IS NULL
          AND status = $6 AND revision = $7
        "#,
    )
    .bind(command.rush())
    .bind(command.ship_by())
    .bind(address_id.get())
    .bind(tenant_id.get())
    .bind(command.order_id().get())
    .bind(status.as_str())
    .bind(command.expected_revision().get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("order changed during amendment"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_amendment_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    context: &CommandContext,
    command: &AmendFulfillmentOrderCommand,
    order: &LockedOrder,
    resulting_address_id: AddressId,
    resulting_revision: OrderRevision,
    amended_at: Timestamp,
) -> AppResult<OrderAmendmentId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO order_amendments (
            tenant_id, inventory_owner_id, order_id,
            previous_address_id, resulting_address_id,
            previous_rush, resulting_rush,
            previous_ship_by, resulting_ship_by,
            order_status, expected_revision, resulting_revision,
            amended_by_user_id, amended_at
        ) VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14
        )
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(order.inventory_owner_id.get())
    .bind(command.order_id().get())
    .bind(order.address_id.get())
    .bind(resulting_address_id.get())
    .bind(order.header.rush())
    .bind(command.rush())
    .bind(order.header.ship_by())
    .bind(command.ship_by())
    .bind(order.status.as_str())
    .bind(command.expected_revision().get())
    .bind(resulting_revision.get())
    .bind(context.actor_id.get())
    .bind(amended_at)
    .fetch_one(&mut **tx)
    .await?;
    OrderAmendmentId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn require_replayed_amendment_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &AmendFulfillmentOrderResult,
    scope: &ScopeBindings,
) -> AppResult<()> {
    if !scope.includes_inventory_owner(result.inventory_owner_id.get()) {
        return Err(AppError::not_found("order amendment"));
    }
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM order_amendments amendment
            INNER JOIN orders order_header
              ON order_header.tenant_id = amendment.tenant_id
             AND order_header.inventory_owner_id = amendment.inventory_owner_id
             AND order_header.id = amendment.order_id
            WHERE amendment.tenant_id = $1
              AND amendment.inventory_owner_id = $2
              AND amendment.id = $3
              AND amendment.order_id = $4
              AND order_header.deleted IS NULL
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(result.inventory_owner_id.get())
    .bind(result.amendment_id.get())
    .bind(result.order_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("order amendment"))
    }
}

async fn enqueue_amended_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &AmendFulfillmentOrderResult,
) -> AppResult<()> {
    let ordering_key = format!("order:{}", result.order_id.get());
    let sequence = orders::next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!(
        "order:{}:amended:{}",
        result.order_id.get(),
        result.revision.get()
    );
    let aggregate_id = result.order_id.to_string();
    let destination = &result.destination;
    let payload = serde_json::json!({
        "amendment_id": result.amendment_id.get(),
        "order_id": result.order_id.get(),
        "inventory_owner_id": result.inventory_owner_id.get(),
        "status": result.order_status.as_str(),
        "revision": result.revision.get(),
        "rush": result.rush,
        "ship_by": result.ship_by,
        "destination": {
            "recipient_name": destination.recipient().name(),
            "company": destination.recipient().company(),
            "phone": destination.recipient().phone(),
            "email": destination.recipient().email(),
            "line1": destination.line1(),
            "line2": destination.line2(),
            "city": destination.city(),
            "region": destination.region(),
            "postal_code": destination.postal_code(),
            "country": destination.country(),
        },
        "amended_by": result.amended_by.get(),
        "amended_at": result.amended_at,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(result.inventory_owner_id),
            facility_id: None,
            actor_user_id: Some(result.amended_by.get()),
            event_key: &event_key,
            aggregate_type: "order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "outbound.order.amended",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.amended_at,
        },
    )
    .await?;
    Ok(())
}

fn map_transition_error(error: OrderAmendmentError) -> AppError {
    match error {
        OrderAmendmentError::InvalidOrderStatus => AppError::conflict(error.to_string()),
        OrderAmendmentError::NoChanges => AppError::bad_request(error.to_string()),
        OrderAmendmentError::RevisionOverflow => AppError::internal(error.to_string()),
    }
}
