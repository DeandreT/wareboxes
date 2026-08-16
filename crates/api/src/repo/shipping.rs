//! Full-order shipment creation, manifesting, and departure confirmation.

mod cancellation;
mod creation;
mod departure;
mod document_policy;
mod documents;
mod manifest;
mod queue;
mod read_model;

pub use cancellation::cancel_shipment;
pub use creation::create_shipment;
pub use departure::confirm_departure;
pub(crate) use departure::{depart_for_outbound_load_tx, OutboundLoadShipmentTarget};
pub use documents::{
    generate_carton_label_set, generate_packing_slip, get_document_content, list_documents,
};
pub use manifest::record_manual_manifest;
pub use queue::{
    shipping_queue, ShippingQueueCursor, ShippingQueueEntry, ShippingQueuePage,
    ShippingQueueShipment,
};
pub use read_model::get_shipment;

use sqlx::Row;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::shipping::ShipmentReadModel;
use wareboxes_domain::{
    ActualPickQuantity, FacilityId, InventoryOwnerId, OrderId, OrderRevision, OrderStatus,
    PackSessionId, PickQuantity, ShipmentId, ShipmentRevision, ShipmentStatus,
    ShortShipDemandQuantities, TenantId, Timestamp,
};
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;
use crate::repo::orders::next_outbox_sequence_tx;

#[derive(Debug, Clone)]
struct LockedOrder {
    id: OrderId,
    inventory_owner_id: InventoryOwnerId,
    order_key: String,
    status: OrderStatus,
    revision: OrderRevision,
}

#[derive(Debug, Clone)]
struct LockedShipment {
    id: ShipmentId,
    attempt: i64,
    packing_session_id: PackSessionId,
    order_release_id: i64,
    order_id: OrderId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    status: ShipmentStatus,
    revision: ShipmentRevision,
    creation_expected_order_revision: OrderRevision,
    creation_resulting_order_revision: OrderRevision,
    carton_count: i64,
    content_count: i64,
    shipped_qty: i64,
    departed_carton_count: i64,
    departed_qty: i64,
    demand: ShortShipDemandQuantities,
}

async fn order_hint_for_session_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: PackSessionId,
) -> AppResult<OrderId> {
    let id = sqlx::query_scalar(
        "SELECT order_id FROM packing_sessions WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id.get())
    .bind(session_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("packing session"))?;
    positive(id, OrderId::new)
}

async fn order_hint_for_shipment_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
) -> AppResult<OrderId> {
    let id = sqlx::query_scalar("SELECT order_id FROM shipments WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id.get())
        .bind(shipment_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("shipment"))?;
    positive(id, OrderId::new)
}

async fn lock_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    order_id: OrderId,
    scope: &ScopeBindings,
) -> AppResult<LockedOrder> {
    let row = sqlx::query(
        r#"
        SELECT id, inventory_owner_id, order_key, status, revision
        FROM orders
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
          AND ($3 OR inventory_owner_id = ANY($4))
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(order_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("order"))?;
    let status_text: String = row.try_get("status")?;
    Ok(LockedOrder {
        id: positive(row.try_get("id")?, OrderId::new)?,
        inventory_owner_id: positive(row.try_get("inventory_owner_id")?, InventoryOwnerId::new)?,
        order_key: row.try_get("order_key")?,
        status: OrderStatus::parse(&status_text)
            .ok_or_else(|| AppError::internal("order has an invalid status"))?,
        revision: positive(row.try_get("revision")?, OrderRevision::new)?,
    })
}

async fn lock_shipment_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
    scope: &ScopeBindings,
) -> AppResult<LockedShipment> {
    let row = sqlx::query(
        r#"
        SELECT id, packing_session_id, order_release_id, order_id,
               inventory_owner_id, facility_id, attempt, state, revision,
               creation_expected_order_revision,creation_resulting_order_revision,
               carton_count,content_count,shipped_qty,departed_carton_count,departed_qty
        FROM shipments
        WHERE tenant_id = $1 AND id = $2
          AND ($3 OR facility_id = ANY($4))
          AND ($5 OR inventory_owner_id = ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("shipment"))?;
    let status_text: String = row.try_get("state")?;
    let inventory_owner_id = positive(row.try_get("inventory_owner_id")?, InventoryOwnerId::new)?;
    let order_id = positive(row.try_get("order_id")?, OrderId::new)?;
    let shipped_qty = row.try_get("shipped_qty")?;
    let demand = order_demand_tx(tx, tenant_id, inventory_owner_id, order_id).await?;
    if demand.effective().get() != shipped_qty {
        return Err(AppError::internal(
            "shipment quantity does not match effective order demand",
        ));
    }
    Ok(LockedShipment {
        id: positive(row.try_get("id")?, ShipmentId::new)?,
        attempt: row.try_get("attempt")?,
        packing_session_id: positive(row.try_get("packing_session_id")?, PackSessionId::new)?,
        order_release_id: row.try_get("order_release_id")?,
        order_id,
        inventory_owner_id,
        facility_id: positive(row.try_get("facility_id")?, FacilityId::new)?,
        status: ShipmentStatus::parse(&status_text)
            .ok_or_else(|| AppError::internal("shipment has an invalid status"))?,
        revision: positive(row.try_get("revision")?, ShipmentRevision::new)?,
        creation_expected_order_revision: positive(
            row.try_get("creation_expected_order_revision")?,
            OrderRevision::new,
        )?,
        creation_resulting_order_revision: positive(
            row.try_get("creation_resulting_order_revision")?,
            OrderRevision::new,
        )?,
        carton_count: row.try_get("carton_count")?,
        content_count: row.try_get("content_count")?,
        shipped_qty,
        departed_carton_count: row.try_get("departed_carton_count")?,
        departed_qty: row.try_get("departed_qty")?,
        demand,
    })
}

pub(super) async fn order_demand_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    order_id: OrderId,
) -> AppResult<ShortShipDemandQuantities> {
    let row = sqlx::query(
        r#"
        SELECT COALESCE(SUM(original_qty), 0)::BIGINT AS ordered_quantity,
               COALESCE(SUM(accepted_short_qty), 0)::BIGINT AS accepted_short_quantity,
               COALESCE(SUM(accepted_substitute_qty), 0)::BIGINT
                   AS accepted_substitute_quantity
        FROM outbound_effective_demand
        WHERE tenant_id = $1 AND inventory_owner_id = $2 AND order_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(order_id.get())
    .fetch_one(&mut **tx)
    .await?;
    ShortShipDemandQuantities::with_substitution(
        PickQuantity::new(row.try_get("ordered_quantity")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        ActualPickQuantity::new(row.try_get("accepted_short_quantity")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        ActualPickQuantity::new(row.try_get("accepted_substitute_quantity")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    )
    .map_err(|error| AppError::internal(error.to_string()))
}

async fn require_replayed_shipment_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &ShipmentReadModel,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM shipments
            WHERE tenant_id = $1 AND id = $2 AND order_id = $3
              AND packing_session_id = $4
              AND ($5 OR facility_id = ANY($6))
              AND ($7 OR inventory_owner_id = ANY($8))
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(result.shipment_id.get())
    .bind(result.order_id.get())
    .bind(result.packing_session_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("shipment"))
    }
}

async fn require_replayed_shipment_id_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
    order_id: OrderId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM shipments
            WHERE tenant_id = $1 AND id = $2 AND order_id = $3
              AND ($4 OR facility_id = ANY($5))
              AND ($6 OR inventory_owner_id = ANY($7))
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .bind(order_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("shipment"))
    }
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_order_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    actor_user_id: i64,
    order_id: OrderId,
    event_type: &str,
    event_key: &str,
    payload: serde_json::Value,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let aggregate_id = order_id.get().to_string();
    let ordering_key = format!("order:{}", order_id.get());
    let aggregate_sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(actor_user_id),
            event_key,
            aggregate_type: "order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

fn positive<T, E>(value: i64, constructor: impl FnOnce(i64) -> Result<T, E>) -> AppResult<T>
where
    E: std::fmt::Display,
{
    constructor(value).map_err(|error| AppError::internal(error.to_string()))
}
