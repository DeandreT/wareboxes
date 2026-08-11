use sqlx::Row;
use wareboxes_application::customer_return::{
    CustomerReturnExecutionStatus, CustomerReturnLineReadModel, CustomerReturnPage,
    CustomerReturnPageFilter, CustomerReturnReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CatalogItemId, CustomerReturnCancellationId, CustomerReturnCancellationReason,
    CustomerReturnId, CustomerReturnLineId, CustomerReturnReason, FacilityId, InboundLoadId,
    InventoryHoldId, InventoryOwnerId, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, Db};

use super::{return_revision, return_status};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub async fn page(
    db: &Db,
    access: &TenantAccess,
    filter: &CustomerReturnPageFilter,
) -> AppResult<CustomerReturnPage> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let limit = i64::from(filter.limit);
    let offset = i64::try_from(filter.offset)
        .map_err(|_| AppError::bad_request("customer return page offset exceeds i64"))?;
    let rows = sqlx::query(
        r#"
        SELECT customer_return.id,customer_return.inventory_owner_id,
               owner.name AS inventory_owner_name,customer_return.facility_id,
               facility.name AS facility_name,asn.number,customer_return.customer_reference,
               asn.expected_at,asn.status,asn.revision,asn.line_count,
               asn.total_expected_quantity AS total_authorized_quantity,
               COALESCE(receipt.received_quantity,0)::BIGINT AS total_received_quantity,
               COALESCE(receipt.rejected_quantity,0)::BIGINT AS total_rejected_quantity,
               COALESCE(receipt.missing_quantity,0)::BIGINT AS total_missing_quantity,
               CASE WHEN asn.status='cancelled' THEN 0 ELSE
                   asn.total_expected_quantity
                       - COALESCE(receipt.received_quantity,0)::BIGINT
                       - COALESCE(receipt.rejected_quantity,0)::BIGINT
                       - COALESCE(receipt.missing_quantity,0)::BIGINT
               END AS total_remaining_quantity,
               asn.load_id,load.status AS execution_status,
               asn.created_by_user_id,asn.created_at,asn.planned_by_user_id,asn.planned_at,
               cancellation.id AS cancellation_id,
               cancellation.reason_code AS cancellation_reason,
               cancellation.note AS cancellation_note,
               cancellation.cancelled_by_user_id,cancellation.cancelled_at
        FROM customer_returns customer_return
        INNER JOIN inbound_asns asn
          ON asn.tenant_id=customer_return.tenant_id
         AND asn.id=customer_return.inbound_asn_id
        INNER JOIN inventory_owners owner
          ON owner.tenant_id=customer_return.tenant_id
         AND owner.id=customer_return.inventory_owner_id
        INNER JOIN facilities facility
          ON facility.tenant_id=customer_return.tenant_id
         AND facility.id=customer_return.facility_id
        LEFT JOIN loads load
          ON load.tenant_id=asn.tenant_id AND load.id=asn.load_id AND load.deleted IS NULL
        LEFT JOIN customer_return_cancellations cancellation
          ON cancellation.tenant_id=customer_return.tenant_id
         AND cancellation.customer_return_id=customer_return.id
        LEFT JOIN LATERAL (
            SELECT SUM(line.received_qty)::BIGINT AS received_quantity,
                   SUM(line.rejected_qty)::BIGINT AS rejected_quantity,
                   SUM(line.missing_qty)::BIGINT AS missing_quantity
            FROM customer_return_load_plans return_plan
            INNER JOIN inbound_asn_load_plan_lines mapping
              ON mapping.tenant_id=return_plan.tenant_id
             AND mapping.plan_id=return_plan.inbound_asn_load_plan_id
            INNER JOIN load_lines line
              ON line.tenant_id=mapping.tenant_id AND line.id=mapping.load_line_id
             AND line.deleted IS NULL
            WHERE return_plan.tenant_id=customer_return.tenant_id
              AND return_plan.customer_return_id=customer_return.id
        ) receipt ON TRUE
        WHERE customer_return.tenant_id=$1
          AND ($2 OR customer_return.facility_id=ANY($3))
          AND ($4 OR customer_return.inventory_owner_id=ANY($5))
          AND ($6::BIGINT IS NULL OR customer_return.facility_id=$6)
          AND ($7::BIGINT IS NULL OR customer_return.inventory_owner_id=$7)
          AND ($8::TEXT IS NULL OR asn.status=$8)
          AND ($9::TEXT IS NULL OR asn.number ILIKE '%' || $9 || '%'
               OR customer_return.customer_reference ILIKE '%' || $9 || '%')
        ORDER BY asn.created_at DESC,customer_return.id DESC
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
    .bind(filter.status.map(|status| status.as_str()))
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
    Ok(CustomerReturnPage {
        entries,
        next_offset: has_more.then(|| filter.offset + u64::from(filter.limit)),
    })
}

pub async fn detail(
    db: &Db,
    access: &TenantAccess,
    customer_return_id: CustomerReturnId,
) -> AppResult<Option<CustomerReturnReadModel>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let header = sqlx::query(
        r#"
        SELECT customer_return.id,customer_return.inventory_owner_id,
               owner.name AS inventory_owner_name,customer_return.facility_id,
               facility.name AS facility_name,asn.number,customer_return.customer_reference,
               asn.expected_at,asn.status,asn.revision,asn.line_count,
               asn.total_expected_quantity AS total_authorized_quantity,
               COALESCE(receipt.received_quantity,0)::BIGINT AS total_received_quantity,
               COALESCE(receipt.rejected_quantity,0)::BIGINT AS total_rejected_quantity,
               COALESCE(receipt.missing_quantity,0)::BIGINT AS total_missing_quantity,
               CASE WHEN asn.status='cancelled' THEN 0 ELSE
                   asn.total_expected_quantity
                       - COALESCE(receipt.received_quantity,0)::BIGINT
                       - COALESCE(receipt.rejected_quantity,0)::BIGINT
                       - COALESCE(receipt.missing_quantity,0)::BIGINT
               END AS total_remaining_quantity,
               asn.load_id,load.status AS execution_status,
               asn.created_by_user_id,asn.created_at,asn.planned_by_user_id,asn.planned_at,
               cancellation.id AS cancellation_id,
               cancellation.reason_code AS cancellation_reason,
               cancellation.note AS cancellation_note,
               cancellation.cancelled_by_user_id,cancellation.cancelled_at
        FROM customer_returns customer_return
        INNER JOIN inbound_asns asn
          ON asn.tenant_id=customer_return.tenant_id
         AND asn.id=customer_return.inbound_asn_id
        INNER JOIN inventory_owners owner
          ON owner.tenant_id=customer_return.tenant_id
         AND owner.id=customer_return.inventory_owner_id
        INNER JOIN facilities facility
          ON facility.tenant_id=customer_return.tenant_id
         AND facility.id=customer_return.facility_id
        LEFT JOIN loads load
          ON load.tenant_id=asn.tenant_id AND load.id=asn.load_id AND load.deleted IS NULL
        LEFT JOIN customer_return_cancellations cancellation
          ON cancellation.tenant_id=customer_return.tenant_id
         AND cancellation.customer_return_id=customer_return.id
        LEFT JOIN LATERAL (
            SELECT SUM(line.received_qty)::BIGINT AS received_quantity,
                   SUM(line.rejected_qty)::BIGINT AS rejected_quantity,
                   SUM(line.missing_qty)::BIGINT AS missing_quantity
            FROM customer_return_load_plans return_plan
            INNER JOIN inbound_asn_load_plan_lines mapping
              ON mapping.tenant_id=return_plan.tenant_id
             AND mapping.plan_id=return_plan.inbound_asn_load_plan_id
            INNER JOIN load_lines line
              ON line.tenant_id=mapping.tenant_id AND line.id=mapping.load_line_id
             AND line.deleted IS NULL
            WHERE return_plan.tenant_id=customer_return.tenant_id
              AND return_plan.customer_return_id=customer_return.id
        ) receipt ON TRUE
        WHERE customer_return.tenant_id=$1 AND customer_return.id=$2
          AND ($3 OR customer_return.facility_id=ANY($4))
          AND ($5 OR customer_return.inventory_owner_id=ANY($6))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(customer_return_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(header) = header else {
        tx.commit().await?;
        return Ok(None);
    };
    let mut result = map_header(&header)?;
    let rows = sqlx::query(
        r#"
        SELECT return_line.id,return_line.sequence,source.item_id,
               COALESCE(item.description,'Item #' || item.id) AS item_description,
               source.uom,source.expected_quantity AS authorized_quantity,
               COALESCE(load_line.received_qty,0)::BIGINT AS received_quantity,
               COALESCE(load_line.rejected_qty,0)::BIGINT AS rejected_quantity,
               COALESCE(load_line.missing_qty,0)::BIGINT AS missing_quantity,
               CASE WHEN $3 THEN 0 ELSE
                   source.expected_quantity
                       - COALESCE(load_line.received_qty,0)::BIGINT
                       - COALESCE(load_line.rejected_qty,0)::BIGINT
                       - COALESCE(load_line.missing_qty,0)::BIGINT
               END AS remaining_quantity,
               return_line.reason_code,return_line.note,source.lot,source.serial,
               CASE WHEN mapping.load_line_id IS NULL THEN ARRAY[]::BIGINT[] ELSE
                   ARRAY(SELECT hold.id FROM inventory_holds hold
                         WHERE hold.tenant_id=return_line.tenant_id
                           AND hold.reference_type='expected_receipt_line'
                           AND hold.reference_id=mapping.load_line_id
                         ORDER BY hold.id)
               END AS inspection_hold_ids
        FROM customer_return_lines return_line
        INNER JOIN inbound_asn_lines source
          ON source.tenant_id=return_line.tenant_id
         AND source.id=return_line.inbound_asn_line_id
        INNER JOIN items item ON item.tenant_id=source.tenant_id AND item.id=source.item_id
        LEFT JOIN inbound_asn_load_plan_lines mapping
          ON mapping.tenant_id=source.tenant_id AND mapping.asn_line_id=source.id
        LEFT JOIN load_lines load_line
          ON load_line.tenant_id=mapping.tenant_id AND load_line.id=mapping.load_line_id
         AND load_line.deleted IS NULL
        WHERE return_line.tenant_id=$1 AND return_line.customer_return_id=$2
        ORDER BY return_line.sequence,return_line.id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(customer_return_id.get())
    .bind(result.status == wareboxes_domain::CustomerReturnStatus::Cancelled)
    .fetch_all(&mut *tx)
    .await?;
    result.lines = rows.iter().map(map_line).collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(Some(result))
}

fn map_header(row: &sqlx::postgres::PgRow) -> AppResult<CustomerReturnReadModel> {
    Ok(CustomerReturnReadModel {
        customer_return_id: CustomerReturnId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_name: row.try_get("facility_name")?,
        number: row.try_get("number")?,
        customer_reference: row.try_get("customer_reference")?,
        expected_at: row.try_get("expected_at")?,
        status: return_status(row.try_get::<String, _>("status")?.as_str())?,
        revision: return_revision(row.try_get("revision")?)?,
        line_count: row.try_get("line_count")?,
        total_authorized_quantity: row.try_get("total_authorized_quantity")?,
        total_received_quantity: row.try_get("total_received_quantity")?,
        total_rejected_quantity: row.try_get("total_rejected_quantity")?,
        total_missing_quantity: row.try_get("total_missing_quantity")?,
        total_remaining_quantity: row.try_get("total_remaining_quantity")?,
        load_id: row
            .try_get::<Option<i64>, _>("load_id")?
            .map(InboundLoadId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        execution_status: row
            .try_get::<Option<String>, _>("execution_status")?
            .map(|status| execution_status(&status))
            .transpose()?,
        created_by: UserId::new(row.try_get("created_by_user_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created_at: row.try_get("created_at")?,
        planned_by: row
            .try_get::<Option<i64>, _>("planned_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        planned_at: row.try_get("planned_at")?,
        cancellation_id: row
            .try_get::<Option<i64>, _>("cancellation_id")?
            .map(CustomerReturnCancellationId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        cancellation_reason: row
            .try_get::<Option<String>, _>("cancellation_reason")?
            .map(|reason| {
                CustomerReturnCancellationReason::parse(&reason).ok_or_else(|| {
                    AppError::internal("stored customer return cancellation reason is invalid")
                })
            })
            .transpose()?,
        cancellation_note: row.try_get("cancellation_note")?,
        cancelled_by: row
            .try_get::<Option<i64>, _>("cancelled_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        cancelled_at: row.try_get("cancelled_at")?,
        lines: Vec::new(),
    })
}

fn map_line(row: &sqlx::postgres::PgRow) -> AppResult<CustomerReturnLineReadModel> {
    Ok(CustomerReturnLineReadModel {
        line_id: CustomerReturnLineId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        sequence: row.try_get("sequence")?,
        item_id: CatalogItemId::new(row.try_get("item_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_description: row.try_get("item_description")?,
        uom: row.try_get("uom")?,
        authorized_quantity: row.try_get("authorized_quantity")?,
        received_quantity: row.try_get("received_quantity")?,
        rejected_quantity: row.try_get("rejected_quantity")?,
        missing_quantity: row.try_get("missing_quantity")?,
        remaining_quantity: row.try_get("remaining_quantity")?,
        reason: CustomerReturnReason::parse(&row.try_get::<String, _>("reason_code")?)
            .ok_or_else(|| AppError::internal("stored customer return reason is invalid"))?,
        note: row.try_get("note")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        inspection_hold_ids: row
            .try_get::<Vec<i64>, _>("inspection_hold_ids")?
            .into_iter()
            .map(|id| {
                InventoryHoldId::new(id).map_err(|error| AppError::internal(error.to_string()))
            })
            .collect::<AppResult<Vec<_>>>()?,
    })
}

fn execution_status(value: &str) -> AppResult<CustomerReturnExecutionStatus> {
    match value {
        "planned" => Ok(CustomerReturnExecutionStatus::Planned),
        "scheduled" => Ok(CustomerReturnExecutionStatus::Scheduled),
        "arrived" => Ok(CustomerReturnExecutionStatus::Arrived),
        "receiving" => Ok(CustomerReturnExecutionStatus::Receiving),
        "received" => Ok(CustomerReturnExecutionStatus::Received),
        "rejected" => Ok(CustomerReturnExecutionStatus::Rejected),
        "closed" => Ok(CustomerReturnExecutionStatus::Closed),
        "cancelled" => Ok(CustomerReturnExecutionStatus::Cancelled),
        _ => Err(AppError::internal(
            "stored customer return load status is invalid",
        )),
    }
}
