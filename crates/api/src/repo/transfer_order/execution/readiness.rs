use sqlx::Row;
use wareboxes_application::transfer_order::{
    TransferDispatchCandidateReadModel, TransferExecutionLocationReadModel,
    TransferExecutionReadiness,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CatalogItemId, InventoryBalanceId, ItemBatchId, LocationId, Timestamp, TransferOrderId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};

use super::{internal, order_scope, ExecutionLocationKind};
use crate::error::AppResult;
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub async fn execution_readiness(
    db: &Db,
    access: &TenantAccess,
    transfer_order_id: TransferOrderId,
) -> AppResult<Option<TransferExecutionReadiness>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let row = sqlx::query(
        r#"SELECT inventory_owner_id,source_facility_id,destination_facility_id,status,revision
           FROM transfer_orders WHERE tenant_id=$1 AND id=$2
             AND ($3 OR (source_facility_id=ANY($4) AND destination_facility_id=ANY($4)))
             AND ($5 OR inventory_owner_id=ANY($6)) FOR SHARE"#,
    )
    .bind(access.tenant_id.get())
    .bind(transfer_order_id.get())
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
    let order = order_scope(&row)?;
    let candidates = sqlx::query(
        r#"SELECT line.id AS transfer_order_line_id,balance.id AS source_inventory_balance_id,
                  balance.location_id,location.barcode AS location_barcode,
                  COALESCE(location.name,location.barcode) AS location_name,
                  balance.item_batch_id,balance.item_id,
                  COALESCE(item.description,'Item #' || item.id) AS item_description,
                  balance.uom,batch.lot,batch.expiration,batch.serial,
                  balance.qty_on_hand-balance.qty_reserved-balance.qty_held AS free_quantity
           FROM transfer_order_lines line
           JOIN inventory_balances balance
             ON balance.tenant_id=line.tenant_id
            AND balance.inventory_owner_id=line.inventory_owner_id
            AND balance.facility_id=line.source_facility_id
            AND balance.item_id=line.item_id AND balance.uom=line.uom
           JOIN item_batches batch
             ON batch.tenant_id=balance.tenant_id AND batch.id=balance.item_batch_id
           JOIN locations location
             ON location.tenant_id=balance.tenant_id AND location.id=balance.location_id
           JOIN items item ON item.tenant_id=line.tenant_id AND item.id=line.item_id
           WHERE line.tenant_id=$1 AND line.transfer_order_id=$2
             AND balance.deleted IS NULL AND balance.license_plate_id IS NULL
             AND balance.status='available'
             AND balance.qty_on_hand-balance.qty_reserved-balance.qty_held>0
             AND batch.deleted IS NULL
             AND (batch.expiration IS NULL OR batch.expiration>statement_timestamp())
             AND location.active AND location.deleted IS NULL AND location.pickable
             AND location.barcode IS NOT NULL
           ORDER BY line.sequence,batch.expiration NULLS LAST,batch.created,balance.id
           LIMIT 500"#,
    )
    .bind(access.tenant_id.get())
    .bind(transfer_order_id.get())
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(map_candidate)
    .collect::<AppResult<Vec<_>>>()?;
    let transit_locations = execution_locations(
        &mut tx,
        access.tenant_id.get(),
        order.source_facility_id.get(),
        ExecutionLocationKind::Transit,
    )
    .await?;
    let receiving_locations = execution_locations(
        &mut tx,
        access.tenant_id.get(),
        order.destination_facility_id.get(),
        ExecutionLocationKind::Receiving,
    )
    .await?;
    tx.commit().await?;
    Ok(Some(TransferExecutionReadiness {
        transfer_order_id,
        revision: order.revision,
        status: order.status,
        dispatch_candidates: candidates,
        transit_locations,
        receiving_locations,
    }))
}

async fn execution_locations(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    facility_id: i64,
    kind: ExecutionLocationKind,
) -> AppResult<Vec<TransferExecutionLocationReadModel>> {
    let (require_receivable, require_transit) = match kind {
        ExecutionLocationKind::Transit => (false, true),
        ExecutionLocationKind::Receiving => (true, false),
    };
    sqlx::query(
        r#"SELECT id,barcode,COALESCE(name,barcode) AS name FROM locations
           WHERE tenant_id=$1 AND facility_id=$2 AND active AND deleted IS NULL
             AND barcode IS NOT NULL
             AND (($3 AND receivable) OR ($4 AND NOT pickable AND NOT receivable
                  AND lower(type)='transfer_in_transit'))
           ORDER BY name,id"#,
    )
    .bind(tenant_id)
    .bind(facility_id)
    .bind(require_receivable)
    .bind(require_transit)
    .fetch_all(&mut **tx)
    .await?
    .iter()
    .map(|row| {
        Ok(TransferExecutionLocationReadModel {
            location_id: LocationId::new(row.try_get("id")?).map_err(internal)?,
            barcode: row.try_get("barcode")?,
            name: row.try_get("name")?,
        })
    })
    .collect()
}

fn map_candidate(row: &sqlx::postgres::PgRow) -> AppResult<TransferDispatchCandidateReadModel> {
    Ok(TransferDispatchCandidateReadModel {
        transfer_order_line_id: wareboxes_domain::TransferOrderLineId::new(
            row.try_get("transfer_order_line_id")?,
        )
        .map_err(internal)?,
        source_inventory_balance_id: InventoryBalanceId::new(
            row.try_get("source_inventory_balance_id")?,
        )
        .map_err(internal)?,
        source_location_id: LocationId::new(row.try_get("location_id")?).map_err(internal)?,
        source_location_barcode: row.try_get("location_barcode")?,
        source_location_name: row.try_get("location_name")?,
        item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?).map_err(internal)?,
        item_id: CatalogItemId::new(row.try_get("item_id")?).map_err(internal)?,
        item_description: row.try_get("item_description")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        expiration: row.try_get::<Option<Timestamp>, _>("expiration")?,
        serial: row.try_get("serial")?,
        free_quantity: row.try_get("free_quantity")?,
    })
}
