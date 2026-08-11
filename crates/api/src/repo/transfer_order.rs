//! Interfacility transfer-order planning, lifecycle commands, and scoped reads.

mod execution;

pub use execution::{dispatch, execution_readiness, receive};

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::transfer_order::{
    CancelTransferOrderCommand, CancelTransferOrderResult, CreateTransferOrderCommand,
    CreateTransferOrderResult, CreatedTransferOrderLineResult, ReleaseTransferOrderCommand,
    ReleaseTransferOrderResult, TransferOrderLineReadModel, TransferOrderPage,
    TransferOrderPageFilter, TransferOrderReadModel, CANCEL_TRANSFER_ORDER_OPERATION,
    CREATE_TRANSFER_ORDER_OPERATION, RELEASE_TRANSFER_ORDER_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    cancel_transfer_order, release_transfer_order, CatalogItemId, FacilityId, InventoryOwnerId,
    Timestamp, TransferOrderCancellationId, TransferOrderCancellationReason,
    TransferOrderDispatchId, TransferOrderId, TransferOrderLineId, TransferOrderReleaseId,
    TransferOrderRevision, TransferOrderStatus, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use super::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::error::{AppError, AppResult};

pub async fn create(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreateTransferOrderCommand,
) -> AppResult<CreateTransferOrderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CREATE_TRANSFER_ORDER_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<CreateTransferOrderResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let order = &command.order;
    if !scope.includes_inventory_owner(order.inventory_owner_id().get())
        || !scope.includes_facility(order.source_facility_id().get())
        || !scope.includes_facility(order.destination_facility_id().get())
    {
        return Err(AppError::forbidden());
    }
    lock_identity(
        &mut tx,
        access,
        order.inventory_owner_id().get(),
        order.number().as_str(),
    )
    .await?;
    lock_scope(
        &mut tx,
        access,
        order.inventory_owner_id().get(),
        order.source_facility_id().get(),
        order.destination_facility_id().get(),
    )
    .await?;
    let item_uoms = lock_items(&mut tx, access, command).await?;
    let line_count = i64::try_from(order.lines().len())
        .map_err(|_| AppError::bad_request("transfer order line count exceeds i64"))?;
    let total_requested_quantity = order.lines().iter().try_fold(0_i64, |total, line| {
        total
            .checked_add(line.requested_quantity().get())
            .ok_or_else(|| AppError::bad_request("transfer order quantity exceeds i64"))
    })?;
    let created_at = now_iso();
    let transfer_order_id = TransferOrderId::new(
        sqlx::query_scalar(
            r#"
        INSERT INTO transfer_orders
            (tenant_id,inventory_owner_id,source_facility_id,destination_facility_id,
             number,expected_departure_at,expected_arrival_at,status,revision,line_count,
             total_requested_quantity,created_by_user_id,created_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,'draft',1,$8,$9,$10,$11)
        RETURNING id
        "#,
        )
        .bind(access.tenant_id.get())
        .bind(order.inventory_owner_id().get())
        .bind(order.source_facility_id().get())
        .bind(order.destination_facility_id().get())
        .bind(order.number().as_str())
        .bind(order.expected_departure_at())
        .bind(order.expected_arrival_at())
        .bind(line_count)
        .bind(total_requested_quantity)
        .bind(context.actor_id.get())
        .bind(created_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(internal)?;
    let mut lines = Vec::with_capacity(order.lines().len());
    for (index, line) in order.lines().iter().enumerate() {
        let sequence = i64::try_from(index + 1)
            .map_err(|_| AppError::bad_request("transfer line sequence exceeds i64"))?;
        let uom = item_uoms.get(&line.item_id().get()).ok_or_else(|| {
            AppError::conflict("transfer item is no longer available to this client")
        })?;
        let line_id = TransferOrderLineId::new(
            sqlx::query_scalar(
                r#"
            INSERT INTO transfer_order_lines
                (tenant_id,inventory_owner_id,source_facility_id,destination_facility_id,
                 transfer_order_id,sequence,item_id,uom,requested_quantity)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            RETURNING id
            "#,
            )
            .bind(access.tenant_id.get())
            .bind(order.inventory_owner_id().get())
            .bind(order.source_facility_id().get())
            .bind(order.destination_facility_id().get())
            .bind(transfer_order_id.get())
            .bind(sequence)
            .bind(line.item_id().get())
            .bind(uom)
            .bind(line.requested_quantity().get())
            .fetch_one(&mut *tx)
            .await?,
        )
        .map_err(internal)?;
        lines.push(CreatedTransferOrderLineResult {
            line_id,
            item_id: line.item_id(),
            requested_quantity: line.requested_quantity().get(),
        });
    }
    let result = CreateTransferOrderResult {
        transfer_order_id,
        number: order.number().as_str().to_owned(),
        status: TransferOrderStatus::Draft,
        revision: revision(1)?,
        lines,
        total_requested_quantity,
        created_by: context.actor_id,
        created_at,
    };
    enqueue_event(&mut tx, access, context, order.inventory_owner_id(), order.source_facility_id(), &result.transfer_order_id, result.revision, "created", "inventory.transfer_order.created", serde_json::json!({
        "transfer_order_id": result.transfer_order_id.get(), "number": result.number,
        "source_facility_id": order.source_facility_id().get(), "destination_facility_id": order.destination_facility_id().get(),
        "status": "draft", "revision": 1, "line_count": result.lines.len(),
        "total_requested_quantity": result.total_requested_quantity, "created_by": result.created_by.get(), "created_at": result.created_at,
    }), result.created_at).await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn release(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReleaseTransferOrderCommand,
) -> AppResult<ReleaseTransferOrderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, RELEASE_TRANSFER_ORDER_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ReleaseTransferOrderResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let row = lock_visible_order(&mut tx, access, &scope, command.transfer_order_id).await?;
    let current_revision = revision(row.try_get("revision")?)?;
    if current_revision != command.expected_revision {
        return Err(AppError::conflict(
            "transfer order changed; refresh before releasing",
        ));
    }
    let previous_status = parse_status(row.try_get::<String, _>("status")?.as_str())?;
    let resulting_revision = release_transfer_order(previous_status, current_revision)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    require_current_line_set(
        &mut tx,
        access,
        command.transfer_order_id,
        row.try_get("line_count")?,
    )
    .await?;
    let owner = InventoryOwnerId::new(row.try_get("inventory_owner_id")?).map_err(internal)?;
    let source = FacilityId::new(row.try_get("source_facility_id")?).map_err(internal)?;
    let destination = FacilityId::new(row.try_get("destination_facility_id")?).map_err(internal)?;
    lock_scope(
        &mut tx,
        access,
        owner.get(),
        source.get(),
        destination.get(),
    )
    .await?;
    let released_at = now_iso();
    let release_id = TransferOrderReleaseId::new(sqlx::query_scalar(
        r#"INSERT INTO transfer_order_releases
           (tenant_id,inventory_owner_id,source_facility_id,destination_facility_id,transfer_order_id,
            expected_revision,resulting_revision,released_by_user_id,released_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING id"#,
    ).bind(access.tenant_id.get()).bind(owner.get()).bind(source.get()).bind(destination.get())
    .bind(command.transfer_order_id.get()).bind(current_revision.get()).bind(resulting_revision.get())
    .bind(context.actor_id.get()).bind(released_at).fetch_one(&mut *tx).await?).map_err(internal)?;
    sqlx::query("UPDATE transfer_orders SET status='released',revision=$3,released_by_user_id=$4,released_at=$5 WHERE tenant_id=$1 AND id=$2")
        .bind(access.tenant_id.get()).bind(command.transfer_order_id.get()).bind(resulting_revision.get()).bind(context.actor_id.get()).bind(released_at).execute(&mut *tx).await?;
    let result = ReleaseTransferOrderResult {
        release_id,
        transfer_order_id: command.transfer_order_id,
        previous_status,
        status: TransferOrderStatus::Released,
        revision: resulting_revision,
        released_by: context.actor_id,
        released_at,
    };
    enqueue_event(&mut tx, access, context, owner, source, &result.transfer_order_id, result.revision, "released", "inventory.transfer_order.released", serde_json::json!({
        "release_id": result.release_id.get(), "transfer_order_id": result.transfer_order_id.get(), "destination_facility_id": destination.get(),
        "status": "released", "revision": result.revision.get(), "released_by": result.released_by.get(), "released_at": result.released_at,
    }), result.released_at).await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn cancel(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelTransferOrderCommand,
) -> AppResult<CancelTransferOrderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CANCEL_TRANSFER_ORDER_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<CancelTransferOrderResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let row = lock_visible_order(&mut tx, access, &scope, command.transfer_order_id()).await?;
    let current_revision = revision(row.try_get("revision")?)?;
    if current_revision != command.expected_revision() {
        return Err(AppError::conflict(
            "transfer order changed; refresh before cancelling",
        ));
    }
    let previous_status = parse_status(row.try_get::<String, _>("status")?.as_str())?;
    let resulting_revision = cancel_transfer_order(previous_status, current_revision)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let owner = InventoryOwnerId::new(row.try_get("inventory_owner_id")?).map_err(internal)?;
    let source = FacilityId::new(row.try_get("source_facility_id")?).map_err(internal)?;
    let destination = FacilityId::new(row.try_get("destination_facility_id")?).map_err(internal)?;
    let cancelled_at = now_iso();
    let cancellation_id = TransferOrderCancellationId::new(sqlx::query_scalar(
        r#"INSERT INTO transfer_order_cancellations
           (tenant_id,inventory_owner_id,source_facility_id,destination_facility_id,transfer_order_id,
            previous_status,reason_code,note,expected_revision,resulting_revision,cancelled_by_user_id,cancelled_at)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING id"#,
    ).bind(access.tenant_id.get()).bind(owner.get()).bind(source.get()).bind(destination.get())
    .bind(command.transfer_order_id().get()).bind(previous_status.as_str()).bind(command.details().reason().as_str())
    .bind(command.details().note().map(|note| note.as_str())).bind(current_revision.get()).bind(resulting_revision.get())
    .bind(context.actor_id.get()).bind(cancelled_at).fetch_one(&mut *tx).await?).map_err(internal)?;
    sqlx::query(
        "UPDATE transfer_orders SET status='cancelled',revision=$3 WHERE tenant_id=$1 AND id=$2",
    )
    .bind(access.tenant_id.get())
    .bind(command.transfer_order_id().get())
    .bind(resulting_revision.get())
    .execute(&mut *tx)
    .await?;
    let result = CancelTransferOrderResult {
        cancellation_id,
        transfer_order_id: command.transfer_order_id(),
        previous_status,
        status: TransferOrderStatus::Cancelled,
        revision: resulting_revision,
        reason: command.details().reason(),
        note: command
            .details()
            .note()
            .map(|note| note.as_str().to_owned()),
        cancelled_by: context.actor_id,
        cancelled_at,
    };
    enqueue_event(&mut tx, access, context, owner, source, &result.transfer_order_id, result.revision, "cancelled", "inventory.transfer_order.cancelled", serde_json::json!({
        "cancellation_id": result.cancellation_id.get(), "transfer_order_id": result.transfer_order_id.get(), "destination_facility_id": destination.get(),
        "previous_status": result.previous_status.as_str(), "status": "cancelled", "revision": result.revision.get(), "reason": result.reason.as_str(),
        "note": result.note, "cancelled_by": result.cancelled_by.get(), "cancelled_at": result.cancelled_at,
    }), result.cancelled_at).await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn page(
    db: &Db,
    access: &TenantAccess,
    filter: &TransferOrderPageFilter,
) -> AppResult<TransferOrderPage> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let offset = i64::try_from(filter.offset)
        .map_err(|_| AppError::bad_request("transfer page offset exceeds i64"))?;
    let rows = sqlx::query(&format!(
        "{} ORDER BY transfer.created_at DESC,transfer.id DESC OFFSET $13 LIMIT $14+1",
        header_query()
    ))
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.source_facility_id.map(FacilityId::get))
    .bind(filter.destination_facility_id.map(FacilityId::get))
    .bind(filter.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(filter.status.map(TransferOrderStatus::as_str))
    .bind(filter.search.as_deref())
    .bind(true)
    .bind(0_i64)
    .bind(offset)
    .bind(i64::from(filter.limit))
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > usize::from(filter.limit);
    let entries = rows
        .iter()
        .take(usize::from(filter.limit))
        .map(map_header)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(TransferOrderPage {
        entries,
        next_offset: has_more.then(|| filter.offset + u64::from(filter.limit)),
    })
}

pub async fn detail(
    db: &Db,
    access: &TenantAccess,
    transfer_order_id: TransferOrderId,
) -> AppResult<Option<TransferOrderReadModel>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let row = sqlx::query(&header_query())
        .bind(access.tenant_id.get())
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(None::<i64>)
        .bind(None::<i64>)
        .bind(None::<i64>)
        .bind(None::<&str>)
        .bind(None::<&str>)
        .bind(false)
        .bind(transfer_order_id.get())
        .fetch_optional(&mut *tx)
        .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let mut result = map_header(&row)?;
    let rows = sqlx::query(
        r#"SELECT line.id,line.sequence,line.item_id,
               COALESCE(item.description,'Item #' || item.id) AS item_description,
               line.uom,line.requested_quantity,
               COALESCE(dispatched.quantity,0) AS dispatched_quantity,
               COALESCE(received.quantity,0) AS received_quantity
        FROM transfer_order_lines line
        JOIN items item ON item.tenant_id=line.tenant_id AND item.id=line.item_id
        LEFT JOIN (
            SELECT tenant_id,transfer_order_line_id,SUM(quantity)::BIGINT AS quantity
            FROM transfer_order_dispatch_lines GROUP BY tenant_id,transfer_order_line_id
        ) dispatched ON dispatched.tenant_id=line.tenant_id
          AND dispatched.transfer_order_line_id=line.id
        LEFT JOIN (
            SELECT tenant_id,transfer_order_line_id,SUM(quantity)::BIGINT AS quantity
            FROM transfer_order_receipt_lines GROUP BY tenant_id,transfer_order_line_id
        ) received ON received.tenant_id=line.tenant_id
          AND received.transfer_order_line_id=line.id
        WHERE line.tenant_id=$1 AND line.transfer_order_id=$2 ORDER BY line.sequence,line.id"#,
    )
    .bind(access.tenant_id.get())
    .bind(transfer_order_id.get())
    .fetch_all(&mut *tx)
    .await?;
    result.lines = rows
        .iter()
        .map(|line| {
            Ok(TransferOrderLineReadModel {
                line_id: TransferOrderLineId::new(line.try_get("id")?).map_err(internal)?,
                sequence: line.try_get("sequence")?,
                item_id: CatalogItemId::new(line.try_get("item_id")?).map_err(internal)?,
                item_description: line.try_get("item_description")?,
                uom: line.try_get("uom")?,
                requested_quantity: line.try_get("requested_quantity")?,
                dispatched_quantity: line.try_get("dispatched_quantity")?,
                received_quantity: line.try_get("received_quantity")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(Some(result))
}

fn header_query() -> String {
    r#"SELECT transfer.id,transfer.inventory_owner_id,owner.name AS inventory_owner_name,
       transfer.source_facility_id,source.name AS source_facility_name,
       transfer.destination_facility_id,destination.name AS destination_facility_name,
       transfer.number,transfer.expected_departure_at,transfer.expected_arrival_at,
       transfer.status,transfer.revision,transfer.line_count,transfer.total_requested_quantity,
       transfer.created_by_user_id,transfer.created_at,transfer.released_by_user_id,transfer.released_at,
       cancellation.id AS cancellation_id,cancellation.reason_code AS cancellation_reason,
       cancellation.note AS cancellation_note,cancellation.cancelled_by_user_id,cancellation.cancelled_at,
       dispatch.id AS dispatch_id,dispatch.inventory_transaction_id AS dispatch_inventory_transaction_id,
       dispatch.transit_location_id,dispatch.transit_location_barcode,
       transfer.dispatched_by_user_id,transfer.dispatched_at,
       receipt.id AS receipt_id,receipt.inventory_transaction_id AS receipt_inventory_transaction_id,
       receipt.destination_location_id AS destination_receiving_location_id,
       receipt.destination_location_barcode AS destination_receiving_location_barcode,
       transfer.received_by_user_id,transfer.received_at
       FROM transfer_orders transfer
       JOIN inventory_owners owner ON owner.tenant_id=transfer.tenant_id AND owner.id=transfer.inventory_owner_id
       JOIN facilities source ON source.tenant_id=transfer.tenant_id AND source.id=transfer.source_facility_id
       JOIN facilities destination ON destination.tenant_id=transfer.tenant_id AND destination.id=transfer.destination_facility_id
       LEFT JOIN transfer_order_cancellations cancellation ON cancellation.tenant_id=transfer.tenant_id AND cancellation.transfer_order_id=transfer.id
       LEFT JOIN transfer_order_dispatches dispatch ON dispatch.tenant_id=transfer.tenant_id AND dispatch.transfer_order_id=transfer.id
       LEFT JOIN transfer_order_receipts receipt ON receipt.tenant_id=transfer.tenant_id AND receipt.transfer_order_id=transfer.id
       WHERE transfer.tenant_id=$1 AND ($2 OR (transfer.source_facility_id=ANY($3) AND transfer.destination_facility_id=ANY($3)))
       AND ($4 OR transfer.inventory_owner_id=ANY($5))
       AND ($6::BIGINT IS NULL OR transfer.source_facility_id=$6)
       AND ($7::BIGINT IS NULL OR transfer.destination_facility_id=$7)
       AND ($8::BIGINT IS NULL OR transfer.inventory_owner_id=$8)
       AND ($9::TEXT IS NULL OR transfer.status=$9)
       AND ($10::TEXT IS NULL OR transfer.number ILIKE '%' || $10 || '%')
       AND ($11 OR transfer.id=$12)"#.to_owned()
}

async fn lock_visible_order(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    scope: &ScopeBindings,
    transfer_order_id: TransferOrderId,
) -> AppResult<sqlx::postgres::PgRow> {
    sqlx::query(r#"SELECT inventory_owner_id,source_facility_id,destination_facility_id,status,revision,line_count FROM transfer_orders
        WHERE tenant_id=$1 AND id=$2 AND ($3 OR (source_facility_id=ANY($4) AND destination_facility_id=ANY($4)))
        AND ($5 OR inventory_owner_id=ANY($6)) FOR UPDATE"#)
        .bind(access.tenant_id.get()).bind(transfer_order_id.get()).bind(scope.all_facilities).bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners).bind(&scope.inventory_owner_ids).fetch_optional(&mut **tx).await?
        .ok_or_else(|| AppError::not_found("transfer order"))
}

async fn lock_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    owner: i64,
    number: &str,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "transfer-order:{}:{owner}:{}",
            access.tenant_id.get(),
            number.to_uppercase()
        ))
        .execute(&mut **tx)
        .await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM transfer_orders WHERE tenant_id=$1 AND inventory_owner_id=$2 AND number=$3)")
        .bind(access.tenant_id.get()).bind(owner).bind(number).fetch_one(&mut **tx).await?;
    if exists {
        Err(AppError::conflict(
            "transfer order number already exists for this client",
        ))
    } else {
        Ok(())
    }
}

async fn lock_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    owner: i64,
    source: i64,
    destination: i64,
) -> AppResult<()> {
    let facilities = vec![source.min(destination), source.max(destination)];
    let rows = sqlx::query(r#"SELECT facility.id FROM facilities facility
        JOIN inventory_owner_facilities link ON link.tenant_id=facility.tenant_id AND link.facility_id=facility.id
        WHERE facility.tenant_id=$1 AND facility.id=ANY($2) AND facility.deleted IS NULL
          AND link.inventory_owner_id=$3 AND link.deleted IS NULL ORDER BY facility.id FOR SHARE OF facility,link"#)
        .bind(access.tenant_id.get()).bind(&facilities).bind(owner).fetch_all(&mut **tx).await?;
    if rows.len() == 2 {
        Ok(())
    } else {
        Err(AppError::conflict(
            "transfer client must remain active at both facilities",
        ))
    }
}

async fn lock_items(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    command: &CreateTransferOrderCommand,
) -> AppResult<HashMap<i64, String>> {
    let item_ids = command
        .order
        .lines()
        .iter()
        .map(|line| line.item_id().get())
        .collect::<Vec<_>>();
    let rows = sqlx::query(r#"SELECT item.id,item.packaging_unit FROM inventory_owner_items owner_item
        JOIN items item ON item.tenant_id=owner_item.tenant_id AND item.id=owner_item.item_id
        WHERE owner_item.tenant_id=$1 AND owner_item.inventory_owner_id=$2 AND owner_item.item_id=ANY($3)
          AND owner_item.deleted IS NULL AND item.deleted IS NULL ORDER BY item.id FOR SHARE OF owner_item,item"#)
        .bind(access.tenant_id.get()).bind(command.order.inventory_owner_id().get()).bind(&item_ids).fetch_all(&mut **tx).await?;
    let map = rows
        .iter()
        .map(|row| Ok((row.try_get("id")?, row.try_get("packaging_unit")?)))
        .collect::<AppResult<HashMap<_, _>>>()?;
    if item_ids.iter().all(|id| map.contains_key(id)) {
        Ok(map)
    } else {
        Err(AppError::conflict(
            "every transfer item must remain active and linked to the client",
        ))
    }
}

async fn require_current_line_set(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    id: TransferOrderId,
    expected_count: i64,
) -> AppResult<()> {
    let count: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM transfer_order_lines line
        JOIN inventory_owner_items owner_item ON owner_item.tenant_id=line.tenant_id AND owner_item.inventory_owner_id=line.inventory_owner_id AND owner_item.item_id=line.item_id AND owner_item.deleted IS NULL
        JOIN items item ON item.tenant_id=line.tenant_id AND item.id=line.item_id AND item.deleted IS NULL
        WHERE line.tenant_id=$1 AND line.transfer_order_id=$2"#).bind(access.tenant_id.get()).bind(id.get()).fetch_one(&mut **tx).await?;
    if count == expected_count {
        Ok(())
    } else {
        Err(AppError::conflict(
            "transfer order line set is no longer executable",
        ))
    }
}

async fn require_stored_visible_before_replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let id: Option<i64> = sqlx::query_scalar("SELECT (result_json->>'transfer_order_id')::BIGINT FROM command_idempotency_records WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3")
        .bind(access.tenant_id.get()).bind(prepared.operation().as_str()).bind(prepared.idempotency_key()).fetch_optional(&mut **tx).await?;
    let Some(id) = id else {
        return Ok(());
    };
    let visible: bool = sqlx::query_scalar(r#"SELECT EXISTS(SELECT 1 FROM transfer_orders WHERE tenant_id=$1 AND id=$2
        AND ($3 OR (source_facility_id=ANY($4) AND destination_facility_id=ANY($4))) AND ($5 OR inventory_owner_id=ANY($6)))"#)
        .bind(access.tenant_id.get()).bind(id).bind(scope.all_facilities).bind(&scope.facility_ids).bind(scope.all_inventory_owners).bind(&scope.inventory_owner_ids).fetch_one(&mut **tx).await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("transfer order"))
    }
}

fn map_header(row: &sqlx::postgres::PgRow) -> AppResult<TransferOrderReadModel> {
    Ok(TransferOrderReadModel {
        transfer_order_id: TransferOrderId::new(row.try_get("id")?).map_err(internal)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(internal)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        source_facility_id: FacilityId::new(row.try_get("source_facility_id")?)
            .map_err(internal)?,
        source_facility_name: row.try_get("source_facility_name")?,
        destination_facility_id: FacilityId::new(row.try_get("destination_facility_id")?)
            .map_err(internal)?,
        destination_facility_name: row.try_get("destination_facility_name")?,
        number: row.try_get("number")?,
        expected_departure_at: row.try_get("expected_departure_at")?,
        expected_arrival_at: row.try_get("expected_arrival_at")?,
        status: parse_status(row.try_get::<String, _>("status")?.as_str())?,
        revision: revision(row.try_get("revision")?)?,
        line_count: row.try_get("line_count")?,
        total_requested_quantity: row.try_get("total_requested_quantity")?,
        created_by: UserId::new(row.try_get("created_by_user_id")?).map_err(internal)?,
        created_at: row.try_get("created_at")?,
        released_by: row
            .try_get::<Option<i64>, _>("released_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        released_at: row.try_get("released_at")?,
        cancellation_id: row
            .try_get::<Option<i64>, _>("cancellation_id")?
            .map(TransferOrderCancellationId::new)
            .transpose()
            .map_err(internal)?,
        cancellation_reason: row
            .try_get::<Option<String>, _>("cancellation_reason")?
            .map(|value| parse_reason(&value))
            .transpose()?,
        cancellation_note: row.try_get("cancellation_note")?,
        cancelled_by: row
            .try_get::<Option<i64>, _>("cancelled_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        cancelled_at: row.try_get("cancelled_at")?,
        dispatch_id: row
            .try_get::<Option<i64>, _>("dispatch_id")?
            .map(TransferOrderDispatchId::new)
            .transpose()
            .map_err(internal)?,
        dispatch_inventory_transaction_id: row.try_get("dispatch_inventory_transaction_id")?,
        transit_location_id: row
            .try_get::<Option<i64>, _>("transit_location_id")?
            .map(wareboxes_domain::LocationId::new)
            .transpose()
            .map_err(internal)?,
        transit_location_barcode: row.try_get("transit_location_barcode")?,
        dispatched_by: row
            .try_get::<Option<i64>, _>("dispatched_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        dispatched_at: row.try_get("dispatched_at")?,
        receipt_id: row
            .try_get::<Option<i64>, _>("receipt_id")?
            .map(wareboxes_domain::TransferOrderReceiptId::new)
            .transpose()
            .map_err(internal)?,
        receipt_inventory_transaction_id: row.try_get("receipt_inventory_transaction_id")?,
        destination_receiving_location_id: row
            .try_get::<Option<i64>, _>("destination_receiving_location_id")?
            .map(wareboxes_domain::LocationId::new)
            .transpose()
            .map_err(internal)?,
        destination_receiving_location_barcode: row
            .try_get("destination_receiving_location_barcode")?,
        received_by: row
            .try_get::<Option<i64>, _>("received_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(internal)?,
        received_at: row.try_get("received_at")?,
        lines: Vec::new(),
    })
}

fn parse_status(value: &str) -> AppResult<TransferOrderStatus> {
    TransferOrderStatus::parse(value)
        .ok_or_else(|| AppError::internal("stored transfer order status is invalid"))
}
fn parse_reason(value: &str) -> AppResult<TransferOrderCancellationReason> {
    TransferOrderCancellationReason::parse(value)
        .ok_or_else(|| AppError::internal("stored transfer cancellation reason is invalid"))
}
fn revision(value: i64) -> AppResult<TransferOrderRevision> {
    TransferOrderRevision::new(value).map_err(internal)
}
fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    owner: InventoryOwnerId,
    source: FacilityId,
    id: &TransferOrderId,
    revision: TransferOrderRevision,
    suffix: &str,
    event_type: &str,
    payload: serde_json::Value,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let event_key = format!("transfer-order:{}:{suffix}", id.get());
    let aggregate_id = id.to_string();
    let ordering_key = format!("transfer-order:{}", id.get());
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(owner),
            facility_id: Some(source),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "transfer_order",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: revision.get(),
            event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}
