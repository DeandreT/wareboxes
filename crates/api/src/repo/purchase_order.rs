//! Purchase-order source intake, release, and operational reads.

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::purchase_order::{
    CreatePurchaseOrderCommand, CreatePurchaseOrderResult, CreatedPurchaseOrderLineResult,
    PurchaseOrderLineReadModel, PurchaseOrderPage, PurchaseOrderPageFilter, PurchaseOrderReadModel,
    ReleasePurchaseOrderCommand, ReleasePurchaseOrderResult, CREATE_PURCHASE_ORDER_OPERATION,
    RELEASE_PURCHASE_ORDER_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    release_purchase_order, CatalogItemId, FacilityId, InventoryOwnerId, PurchaseOrderId,
    PurchaseOrderLineId, PurchaseOrderReleaseId, PurchaseOrderRevision, PurchaseOrderStatus,
    Timestamp, UserId,
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
    command: &CreatePurchaseOrderCommand,
) -> AppResult<CreatePurchaseOrderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CREATE_PURCHASE_ORDER_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<CreatePurchaseOrderResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let order = &command.order;
    if !scope.includes_inventory_owner(order.inventory_owner_id().get())
        || !scope.includes_facility(order.facility_id().get())
    {
        return Err(AppError::forbidden());
    }
    lock_source_identity(
        &mut tx,
        access,
        order.inventory_owner_id().get(),
        order.number().as_str(),
    )
    .await?;
    lock_source_scope(
        &mut tx,
        access,
        order.inventory_owner_id().get(),
        order.facility_id().get(),
    )
    .await?;
    let item_uoms = lock_source_items(&mut tx, access, command).await?;
    let line_count = i64::try_from(order.lines().len())
        .map_err(|_| AppError::bad_request("purchase order line count exceeds i64"))?;
    let total_ordered_quantity = order.lines().iter().try_fold(0_i64, |total, line| {
        total
            .checked_add(line.ordered_quantity().get())
            .ok_or_else(|| AppError::bad_request("purchase order quantity exceeds i64"))
    })?;
    let created_at = now_iso();
    let purchase_order_id = PurchaseOrderId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO purchase_orders
                (tenant_id,inventory_owner_id,facility_id,number,supplier,expected_by,
                 status,revision,line_count,total_ordered_quantity,created_by_user_id,created_at)
            VALUES ($1,$2,$3,$4,$5,$6,'draft',1,$7,$8,$9,$10)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(order.inventory_owner_id().get())
        .bind(order.facility_id().get())
        .bind(order.number().as_str())
        .bind(order.supplier().as_str())
        .bind(order.expected_by())
        .bind(line_count)
        .bind(total_ordered_quantity)
        .bind(context.actor_id.get())
        .bind(created_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let mut lines = Vec::with_capacity(order.lines().len());
    for (index, line) in order.lines().iter().enumerate() {
        let sequence = i64::try_from(index + 1)
            .map_err(|_| AppError::bad_request("purchase order line sequence exceeds i64"))?;
        let uom = item_uoms.get(&line.item_id().get()).ok_or_else(|| {
            AppError::conflict("purchase order item is no longer available to this client")
        })?;
        let line_id = PurchaseOrderLineId::new(
            sqlx::query_scalar(
                r#"
                INSERT INTO purchase_order_lines
                    (tenant_id,inventory_owner_id,facility_id,purchase_order_id,sequence,
                     item_id,uom,ordered_quantity)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
                RETURNING id
                "#,
            )
            .bind(access.tenant_id.get())
            .bind(order.inventory_owner_id().get())
            .bind(order.facility_id().get())
            .bind(purchase_order_id.get())
            .bind(sequence)
            .bind(line.item_id().get())
            .bind(uom)
            .bind(line.ordered_quantity().get())
            .fetch_one(&mut *tx)
            .await?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?;
        lines.push(CreatedPurchaseOrderLineResult {
            line_id,
            item_id: line.item_id(),
            ordered_quantity: line.ordered_quantity().get(),
        });
    }
    let result = CreatePurchaseOrderResult {
        purchase_order_id,
        number: order.number().as_str().to_owned(),
        status: PurchaseOrderStatus::Draft,
        revision: revision(1)?,
        lines,
        total_ordered_quantity,
        created_by: context.actor_id,
        created_at,
    };
    enqueue_created(
        &mut tx,
        access,
        context,
        order.inventory_owner_id(),
        order.facility_id(),
        &result,
    )
    .await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn release(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReleasePurchaseOrderCommand,
) -> AppResult<ReleasePurchaseOrderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, RELEASE_PURCHASE_ORDER_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<ReleasePurchaseOrderResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id,facility_id,status,revision,line_count
        FROM purchase_orders
        WHERE tenant_id=$1 AND id=$2
          AND ($3 OR facility_id=ANY($4))
          AND ($5 OR inventory_owner_id=ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.purchase_order_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("purchase order"))?;
    let current_revision = revision(row.try_get("revision")?)?;
    if current_revision != command.expected_revision {
        return Err(AppError::conflict(
            "purchase order changed; refresh before releasing",
        ));
    }
    let previous_status = parse_status(row.try_get::<String, _>("status")?.as_str())?;
    let resulting_revision = release_purchase_order(previous_status, current_revision)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let active_lines = sqlx::query(
        r#"
        SELECT owner_item.item_id
        FROM purchase_order_lines line
        INNER JOIN inventory_owner_items owner_item
          ON owner_item.tenant_id=line.tenant_id
         AND owner_item.inventory_owner_id=line.inventory_owner_id
         AND owner_item.item_id=line.item_id AND owner_item.deleted IS NULL
        INNER JOIN items item
          ON item.tenant_id=line.tenant_id AND item.id=line.item_id AND item.deleted IS NULL
        WHERE line.tenant_id=$1 AND line.purchase_order_id=$2
        ORDER BY owner_item.item_id
        FOR SHARE OF owner_item,item
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.purchase_order_id.get())
    .fetch_all(&mut *tx)
    .await?;
    if i64::try_from(active_lines.len()).map_err(|_| AppError::internal("line count overflow"))?
        != row.try_get::<i64, _>("line_count")?
    {
        return Err(AppError::conflict(
            "purchase order line set is no longer executable",
        ));
    }
    let inventory_owner_id = InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let released_at = now_iso();
    let release_id = PurchaseOrderReleaseId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO purchase_order_releases
                (tenant_id,inventory_owner_id,facility_id,purchase_order_id,
                 expected_revision,resulting_revision,released_by_user_id,released_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id.get())
        .bind(facility_id.get())
        .bind(command.purchase_order_id.get())
        .bind(current_revision.get())
        .bind(resulting_revision.get())
        .bind(context.actor_id.get())
        .bind(released_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    sqlx::query(
        r#"
        UPDATE purchase_orders
        SET status='released',revision=$3,released_by_user_id=$4,released_at=$5
        WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.purchase_order_id.get())
    .bind(resulting_revision.get())
    .bind(context.actor_id.get())
    .bind(released_at)
    .execute(&mut *tx)
    .await?;
    let result = ReleasePurchaseOrderResult {
        release_id,
        purchase_order_id: command.purchase_order_id,
        previous_status,
        status: PurchaseOrderStatus::Released,
        revision: resulting_revision,
        released_by: context.actor_id,
        released_at,
    };
    enqueue_released(
        &mut tx,
        access,
        context,
        inventory_owner_id,
        facility_id,
        &result,
    )
    .await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn page(
    db: &Db,
    access: &TenantAccess,
    filter: &PurchaseOrderPageFilter,
) -> AppResult<PurchaseOrderPage> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let limit = i64::from(filter.limit);
    let offset = i64::try_from(filter.offset)
        .map_err(|_| AppError::bad_request("purchase order page offset exceeds i64"))?;
    let rows = sqlx::query(
        r#"
        SELECT purchase.id,purchase.inventory_owner_id,owner.name AS inventory_owner_name,
               purchase.facility_id,facility.name AS facility_name,purchase.number,
               purchase.supplier,purchase.expected_by,purchase.status,purchase.revision,
               purchase.line_count,purchase.total_ordered_quantity,
               purchase.created_by_user_id,purchase.created_at,
               purchase.released_by_user_id,purchase.released_at
        FROM purchase_orders purchase
        INNER JOIN inventory_owners owner
          ON owner.tenant_id=purchase.tenant_id AND owner.id=purchase.inventory_owner_id
        INNER JOIN facilities facility
          ON facility.tenant_id=purchase.tenant_id AND facility.id=purchase.facility_id
        WHERE purchase.tenant_id=$1
          AND ($2 OR purchase.facility_id=ANY($3))
          AND ($4 OR purchase.inventory_owner_id=ANY($5))
          AND ($6::BIGINT IS NULL OR purchase.facility_id=$6)
          AND ($7::BIGINT IS NULL OR purchase.inventory_owner_id=$7)
          AND ($8::TEXT IS NULL OR purchase.status=$8)
          AND ($9::TEXT IS NULL OR purchase.number ILIKE '%' || $9 || '%'
               OR purchase.supplier ILIKE '%' || $9 || '%')
        ORDER BY purchase.created_at DESC,purchase.id DESC
        OFFSET $10 LIMIT $11+1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.facility_id.map(FacilityId::get))
    .bind(filter.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(filter.status.map(PurchaseOrderStatus::as_str))
    .bind(filter.search.as_deref())
    .bind(offset)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > usize::from(filter.limit);
    let entries = rows
        .iter()
        .take(usize::from(filter.limit))
        .map(map_header)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(PurchaseOrderPage {
        entries,
        next_offset: has_more.then(|| filter.offset + u64::from(filter.limit)),
    })
}

pub async fn detail(
    db: &Db,
    access: &TenantAccess,
    purchase_order_id: PurchaseOrderId,
) -> AppResult<Option<PurchaseOrderReadModel>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let row = sqlx::query(
        r#"
        SELECT purchase.id,purchase.inventory_owner_id,owner.name AS inventory_owner_name,
               purchase.facility_id,facility.name AS facility_name,purchase.number,
               purchase.supplier,purchase.expected_by,purchase.status,purchase.revision,
               purchase.line_count,purchase.total_ordered_quantity,
               purchase.created_by_user_id,purchase.created_at,
               purchase.released_by_user_id,purchase.released_at
        FROM purchase_orders purchase
        INNER JOIN inventory_owners owner
          ON owner.tenant_id=purchase.tenant_id AND owner.id=purchase.inventory_owner_id
        INNER JOIN facilities facility
          ON facility.tenant_id=purchase.tenant_id AND facility.id=purchase.facility_id
        WHERE purchase.tenant_id=$1 AND purchase.id=$2
          AND ($3 OR purchase.facility_id=ANY($4))
          AND ($5 OR purchase.inventory_owner_id=ANY($6))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(purchase_order_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let mut result = map_header(&row)?;
    let lines = sqlx::query(
        r#"
        SELECT line.id,line.sequence,line.item_id,
               COALESCE(item.description,'Item #' || item.id) AS item_description,
               line.uom,line.ordered_quantity
        FROM purchase_order_lines line
        INNER JOIN items item ON item.tenant_id=line.tenant_id AND item.id=line.item_id
        WHERE line.tenant_id=$1 AND line.purchase_order_id=$2
        ORDER BY line.sequence,line.id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(purchase_order_id.get())
    .fetch_all(&mut *tx)
    .await?;
    result.lines = lines
        .iter()
        .map(|line| {
            Ok(PurchaseOrderLineReadModel {
                line_id: PurchaseOrderLineId::new(line.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                sequence: line.try_get("sequence")?,
                item_id: CatalogItemId::new(line.try_get("item_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                item_description: line.try_get("item_description")?,
                uom: line.try_get("uom")?,
                ordered_quantity: line.try_get("ordered_quantity")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(Some(result))
}

async fn lock_source_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    inventory_owner_id: i64,
    number: &str,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "purchase-order:{}:{inventory_owner_id}:{}",
            access.tenant_id.get(),
            number.to_uppercase()
        ))
        .execute(&mut **tx)
        .await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM purchase_orders WHERE tenant_id=$1 AND inventory_owner_id=$2 AND number=$3)",
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id)
    .bind(number)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Err(AppError::conflict(
            "purchase order number already exists for this client",
        ))
    } else {
        Ok(())
    }
}

async fn lock_source_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    inventory_owner_id: i64,
    facility_id: i64,
) -> AppResult<()> {
    let owner: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM inventory_owners WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL FOR SHARE",
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id)
    .fetch_optional(&mut **tx)
    .await?;
    let facility: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM facilities WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL FOR SHARE",
    )
    .bind(access.tenant_id.get())
    .bind(facility_id)
    .fetch_optional(&mut **tx)
    .await?;
    if owner.is_none() || facility.is_none() {
        Err(AppError::not_found("purchase order client or facility"))
    } else {
        Ok(())
    }
}

async fn lock_source_items(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    command: &CreatePurchaseOrderCommand,
) -> AppResult<HashMap<i64, String>> {
    let item_ids = command
        .order
        .lines()
        .iter()
        .map(|line| line.item_id().get())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT item.id,item.packaging_unit
        FROM inventory_owner_items owner_item
        INNER JOIN items item
          ON item.tenant_id=owner_item.tenant_id AND item.id=owner_item.item_id
        WHERE owner_item.tenant_id=$1 AND owner_item.inventory_owner_id=$2
          AND owner_item.item_id=ANY($3) AND owner_item.deleted IS NULL AND item.deleted IS NULL
        ORDER BY item.id
        FOR SHARE OF owner_item,item
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.order.inventory_owner_id().get())
    .bind(&item_ids)
    .fetch_all(&mut **tx)
    .await?;
    let result = rows
        .iter()
        .map(|row| Ok((row.try_get("id")?, row.try_get("packaging_unit")?)))
        .collect::<AppResult<HashMap<_, _>>>()?;
    if item_ids.iter().all(|item_id| result.contains_key(item_id)) {
        Ok(result)
    } else {
        Err(AppError::conflict(
            "every purchase order item must remain active and linked to the client",
        ))
    }
}

async fn require_stored_visible_before_replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored_id: Option<i64> = sqlx::query_scalar(
        "SELECT (result_json->>'purchase_order_id')::BIGINT FROM command_idempotency_records WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3",
    )
    .bind(access.tenant_id.get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(stored_id) = stored_id else {
        return Ok(());
    };
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM purchase_orders
            WHERE tenant_id=$1 AND id=$2
              AND ($3 OR facility_id=ANY($4))
              AND ($5 OR inventory_owner_id=ANY($6)))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(stored_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("purchase order"))
    }
}

fn map_header(row: &sqlx::postgres::PgRow) -> AppResult<PurchaseOrderReadModel> {
    Ok(PurchaseOrderReadModel {
        purchase_order_id: PurchaseOrderId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_name: row.try_get("facility_name")?,
        number: row.try_get("number")?,
        supplier: row.try_get("supplier")?,
        expected_by: row.try_get("expected_by")?,
        status: parse_status(row.try_get::<String, _>("status")?.as_str())?,
        revision: revision(row.try_get("revision")?)?,
        line_count: row.try_get("line_count")?,
        total_ordered_quantity: row.try_get("total_ordered_quantity")?,
        created_by: UserId::new(row.try_get("created_by_user_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created_at: row.try_get("created_at")?,
        released_by: row
            .try_get::<Option<i64>, _>("released_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        released_at: row.try_get("released_at")?,
        lines: Vec::new(),
    })
}

fn parse_status(value: &str) -> AppResult<PurchaseOrderStatus> {
    PurchaseOrderStatus::parse(value)
        .ok_or_else(|| AppError::internal("stored purchase order status is invalid"))
}

fn revision(value: i64) -> AppResult<PurchaseOrderRevision> {
    PurchaseOrderRevision::new(value).map_err(|error| AppError::internal(error.to_string()))
}

async fn enqueue_created(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    result: &CreatePurchaseOrderResult,
) -> AppResult<()> {
    enqueue_event(
        tx,
        access,
        context,
        inventory_owner_id,
        facility_id,
        result.purchase_order_id,
        result.revision,
        "created",
        "inbound.purchase_order.created",
        serde_json::json!({
            "purchase_order_id": result.purchase_order_id.get(),
            "number": result.number,
            "status": "draft",
            "revision": result.revision.get(),
            "line_count": result.lines.len(),
            "total_ordered_quantity": result.total_ordered_quantity,
            "created_by": result.created_by.get(),
            "created_at": result.created_at,
        }),
        result.created_at,
    )
    .await
}

async fn enqueue_released(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    result: &ReleasePurchaseOrderResult,
) -> AppResult<()> {
    enqueue_event(
        tx,
        access,
        context,
        inventory_owner_id,
        facility_id,
        result.purchase_order_id,
        result.revision,
        "released",
        "inbound.purchase_order.released",
        serde_json::json!({
            "release_id": result.release_id.get(),
            "purchase_order_id": result.purchase_order_id.get(),
            "status": "released",
            "revision": result.revision.get(),
            "released_by": result.released_by.get(),
            "released_at": result.released_at,
        }),
        result.released_at,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    purchase_order_id: PurchaseOrderId,
    revision: PurchaseOrderRevision,
    event_suffix: &str,
    event_type: &str,
    payload: serde_json::Value,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let event_key = format!("purchase-order:{}:{event_suffix}", purchase_order_id.get());
    let aggregate_id = purchase_order_id.to_string();
    let ordering_key = format!("purchase-order:{}", purchase_order_id.get());
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "purchase_order",
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
