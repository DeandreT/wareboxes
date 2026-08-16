use sqlx::Row;
use wareboxes_application::picking::{
    PickConfirmationHistoryCursor, PickConfirmationHistoryPage, PickConfirmationHistoryQuery,
    PickConfirmationHistoryReadModel, PickReversalHistoryReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    LicensePlateId, LocationId, OrderId, PickConfirmationId, PickContentId, PickQuantity,
    PickReversalId, PickReversalNote, PickReversalReason, PickTaskId, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::picking::policy::decision_policy_from_task_row;

pub async fn list_confirmation_history(
    db: &Db,
    access: &TenantAccess,
    query: PickConfirmationHistoryQuery,
) -> AppResult<PickConfirmationHistoryPage> {
    if query.limit == 0 || query.limit > 100 {
        return Err(AppError::bad_request(
            "pick confirmation history limit must be between 1 and 100",
        ));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "orders").await?;
    let order_scope = sqlx::query(
        r#"
        SELECT inventory_owner_id FROM orders
        WHERE tenant_id = $1 AND id = $2 AND deleted IS NULL
          AND ($3 OR inventory_owner_id = ANY($4))
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(query.order_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("order"))?;
    let owner_id: i64 = order_scope.try_get("inventory_owner_id")?;

    let fetch_limit = i64::from(query.limit) + 1;
    let rows = sqlx::query(
        r#"
        SELECT confirmation.id, confirmation.task_id,
               confirmation.pick_task_content_id, confirmation.order_id,
               confirmation.facility_id, confirmation.item_id,
               item.description AS item_description, confirmation.uom,
               batch.lot, batch.serial, confirmation.picked_qty,
               confirmation.source_location_id,
               source_location.name AS source_location_name,
               confirmation.source_license_plate_id IS NOT NULL
                   AS source_license_plate_required,
               confirmation.destination_location_id AS staged_location_id,
               staged_location.name AS staged_location_name,
               confirmation.destination_license_plate_id AS staged_license_plate_id,
               confirmation.pick_policy_source,
               confirmation.pick_configuration_id,
               confirmation.pick_configuration_revision,
               confirmation.pick_scope_level,
               confirmation.pick_inventory_owner_id,
               confirmation.pick_facility_id,
               confirmation.require_source_location_scan,
               confirmation.require_item_scan,
               confirmation.require_destination_container_scan,
               confirmation.pick_policy_hash,
               confirmation.source_location_scan_verified,
               confirmation.item_scan_verified,
               confirmation.destination_container_scan_verified,
               confirmation.confirmed_by_user_id, confirmation.confirmed_at,
               reversal.id AS reversal_id, reversal.reason AS reversal_reason,
               reversal.note AS reversal_note,
               reversal.reversed_by_user_id, reversal.reversed_at
        FROM pick_confirmations confirmation
        INNER JOIN items item
          ON item.tenant_id = confirmation.tenant_id
         AND item.id = confirmation.item_id
        INNER JOIN item_batches batch
          ON batch.tenant_id = confirmation.tenant_id
         AND batch.inventory_owner_id = confirmation.inventory_owner_id
         AND batch.id = confirmation.item_batch_id
        INNER JOIN locations source_location
          ON source_location.tenant_id = confirmation.tenant_id
         AND source_location.facility_id = confirmation.facility_id
         AND source_location.id = confirmation.source_location_id
        INNER JOIN locations staged_location
          ON staged_location.tenant_id = confirmation.tenant_id
         AND staged_location.facility_id = confirmation.facility_id
         AND staged_location.id = confirmation.destination_location_id
        LEFT JOIN pick_reversals reversal
          ON reversal.tenant_id = confirmation.tenant_id
         AND reversal.inventory_owner_id = confirmation.inventory_owner_id
         AND reversal.pick_confirmation_id = confirmation.id
        WHERE confirmation.tenant_id = $1
          AND confirmation.inventory_owner_id = $2
          AND confirmation.order_id = $3
          AND ($4 OR confirmation.facility_id = ANY($5))
          AND ($6::TIMESTAMPTZ IS NULL
               OR (confirmation.confirmed_at, confirmation.id) < ($6, $7))
        ORDER BY confirmation.confirmed_at DESC, confirmation.id DESC
        LIMIT $8
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(owner_id)
    .bind(query.order_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(query.cursor.map(|cursor| cursor.confirmed_at))
    .bind(query.cursor.map(|cursor| cursor.confirmation_id.get()))
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = rows
        .into_iter()
        .map(map_history_row)
        .collect::<AppResult<Vec<_>>>()?;
    let has_more = items.len() > usize::from(query.limit);
    if has_more {
        items.pop();
    }
    let next_cursor = has_more
        .then(|| {
            items.last().map(|item| PickConfirmationHistoryCursor {
                confirmed_at: item.confirmed_at,
                confirmation_id: item.confirmation_id,
            })
        })
        .flatten();
    tx.commit().await?;
    Ok(PickConfirmationHistoryPage { items, next_cursor })
}

fn map_history_row(row: sqlx::postgres::PgRow) -> AppResult<PickConfirmationHistoryReadModel> {
    let pick_policy = decision_policy_from_task_row(&row)?;
    let reversal = match row.try_get::<Option<i64>, _>("reversal_id")? {
        Some(id) => {
            let reason = PickReversalReason::parse(&row.try_get::<String, _>("reversal_reason")?)
                .ok_or_else(|| AppError::internal("pick reversal has invalid reason"))?;
            let note = row
                .try_get::<Option<String>, _>("reversal_note")?
                .map(PickReversalNote::new)
                .transpose()
                .map_err(|error| AppError::internal(error.to_string()))?;
            Some(PickReversalHistoryReadModel {
                reversal_id: PickReversalId::new(id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                reason,
                note,
                reversed_by: UserId::new(row.try_get("reversed_by_user_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                reversed_at: row.try_get("reversed_at")?,
            })
        }
        None => None,
    };
    Ok(PickConfirmationHistoryReadModel {
        confirmation_id: PickConfirmationId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        task_id: PickTaskId::new(row.try_get("task_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        content_id: PickContentId::new(row.try_get("pick_task_content_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        picked_quantity: PickQuantity::new(row.try_get("picked_qty")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_location_id: LocationId::new(row.try_get("source_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        source_location_name: row.try_get("source_location_name")?,
        source_license_plate_required: row.try_get("source_license_plate_required")?,
        staged_location_id: LocationId::new(row.try_get("staged_location_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        staged_location_name: row.try_get("staged_location_name")?,
        staged_license_plate_id: LicensePlateId::new(row.try_get("staged_license_plate_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        pick_policy,
        source_location_scan_verified: row.try_get("source_location_scan_verified")?,
        item_scan_verified: row.try_get("item_scan_verified")?,
        destination_container_scan_verified: row.try_get("destination_container_scan_verified")?,
        confirmed_by: UserId::new(row.try_get("confirmed_by_user_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        confirmed_at: row.try_get("confirmed_at")?,
        reversal,
    })
}
