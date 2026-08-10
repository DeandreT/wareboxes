use std::collections::HashMap;

use sqlx::postgres::PgRow;
use sqlx::Row;
use wareboxes_application::inventory::InventoryBalanceStatus;
use wareboxes_application::inventory_integrity::{
    InventoryAgingBucket, InventoryAgingPage, InventoryAgingQuery, InventoryAgingReadModel,
    InventoryAgingSort, InventoryIntegrityIssueKind, InventoryIntegrityIssueReadModel,
    InventoryIntegrityPage, InventoryIntegrityQuery, InventoryIntegritySort,
    InventoryJournalEntryReadModel, InventoryJournalPage, InventoryJournalQuery,
    InventoryJournalSort, InventoryJournalTransactionReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{FacilityId, InventoryOwnerId};

use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

const MAX_PAGE_SIZE: u16 = 1_000;

pub async fn journal_page(
    db: &Db,
    access: &TenantAccess,
    query: &InventoryJournalQuery,
) -> AppResult<InventoryJournalPage> {
    validate_journal_query(query)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let query_id = query
        .search
        .as_deref()
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0);
    let fetch_limit = i64::from(query.limit) + 1;
    let offset = i64::try_from(query.offset)
        .map_err(|_| AppError::bad_request("inventory journal cursor is out of range"))?;
    let rows = sqlx::query(
        r#"
        SELECT transaction.id, transaction.inventory_owner_id,
               owner.name AS inventory_owner_name, transaction.created,
               transaction.actor_user_id, transaction.transaction_type,
               transaction.reason, transaction.reference_type, transaction.reference_id,
               transaction.correlation_id, transaction.operation,
               COUNT(entry.id)::BIGINT AS entry_count,
               COALESCE(SUM(entry.quantity_delta), 0)::BIGINT AS net_quantity
        FROM inventory_transactions transaction
        INNER JOIN inventory_owners owner
          ON owner.tenant_id=transaction.tenant_id
         AND owner.id=transaction.inventory_owner_id
        INNER JOIN inventory_entries entry
          ON entry.tenant_id=transaction.tenant_id
         AND entry.inventory_owner_id=transaction.inventory_owner_id
         AND entry.transaction_id=transaction.id
        INNER JOIN facilities facility
          ON facility.tenant_id=entry.tenant_id AND facility.id=entry.facility_id
        INNER JOIN locations location
          ON location.tenant_id=entry.tenant_id AND location.id=entry.location_id
        INNER JOIN items item
          ON item.tenant_id=entry.tenant_id AND item.id=entry.item_id
        LEFT JOIN license_plates plate
          ON plate.tenant_id=entry.tenant_id
         AND plate.inventory_owner_id=entry.inventory_owner_id
         AND plate.id=entry.license_plate_id
        LEFT JOIN LATERAL (
            SELECT item_sku.name
            FROM skus item_sku
            WHERE item_sku.tenant_id=entry.tenant_id
              AND item_sku.item_id=entry.item_id
              AND item_sku.deleted IS NULL
            ORDER BY item_sku.id LIMIT 1
        ) sku ON TRUE
        WHERE transaction.tenant_id=$1
          AND ($2 OR entry.facility_id=ANY($3))
          AND ($4 OR transaction.inventory_owner_id=ANY($5))
          AND ($6::BIGINT IS NULL OR entry.facility_id=$6)
          AND ($7::BIGINT IS NULL OR transaction.inventory_owner_id=$7)
          AND ($8::BIGINT IS NULL OR entry.item_id=$8)
          AND ($9::BIGINT IS NULL OR entry.item_batch_id=$9)
          AND ($10::BIGINT IS NULL OR entry.license_plate_id=$10)
          AND ($11::BIGINT IS NULL OR transaction.id=$11)
          AND (
              $12::TEXT IS NULL
              OR STRPOS(LOWER(transaction.transaction_type), LOWER($12)) > 0
              OR STRPOS(LOWER(transaction.operation), LOWER($12)) > 0
              OR STRPOS(LOWER(COALESCE(transaction.reason,'')), LOWER($12)) > 0
              OR STRPOS(LOWER(COALESCE(transaction.reference_type,'')), LOWER($12)) > 0
              OR STRPOS(LOWER(owner.name), LOWER($12)) > 0
              OR STRPOS(LOWER(facility.name), LOWER($12)) > 0
              OR STRPOS(LOWER(COALESCE(location.name,'')), LOWER($12)) > 0
              OR STRPOS(LOWER(COALESCE(location.barcode,'')), LOWER($12)) > 0
              OR STRPOS(LOWER(COALESCE(plate.barcode,'')), LOWER($12)) > 0
              OR STRPOS(LOWER(COALESCE(sku.name,'')), LOWER($12)) > 0
              OR STRPOS(LOWER(COALESCE(item.description,'')), LOWER($12)) > 0
              OR STRPOS(LOWER(COALESCE(entry.lot,'')), LOWER($12)) > 0
              OR STRPOS(LOWER(COALESCE(entry.serial,'')), LOWER($12)) > 0
              OR ($13::BIGINT IS NOT NULL AND $13 IN (
                  transaction.id, transaction.inventory_owner_id, entry.facility_id,
                  entry.location_id, entry.item_batch_id, entry.item_id,
                  COALESCE(entry.license_plate_id, 0)
              ))
          )
        GROUP BY transaction.id, transaction.inventory_owner_id, owner.name,
                 transaction.created, transaction.actor_user_id,
                 transaction.transaction_type, transaction.reason,
                 transaction.reference_type, transaction.reference_id,
                 transaction.correlation_id, transaction.operation
        ORDER BY
          CASE WHEN $14='occurred_at' AND $15 THEN transaction.created END ASC,
          CASE WHEN $14='occurred_at' AND NOT $15 THEN transaction.created END DESC,
          CASE WHEN $14='transaction' AND $15 THEN transaction.id END ASC,
          CASE WHEN $14='transaction' AND NOT $15 THEN transaction.id END DESC,
          CASE WHEN $14='type' AND $15 THEN LOWER(transaction.transaction_type) END ASC,
          CASE WHEN $14='type' AND NOT $15 THEN LOWER(transaction.transaction_type) END DESC,
          CASE WHEN $14='client' AND $15 THEN LOWER(owner.name) END ASC,
          CASE WHEN $14='client' AND NOT $15 THEN LOWER(owner.name) END DESC,
          CASE WHEN $14='net_quantity' AND $15 THEN COALESCE(SUM(entry.quantity_delta),0) END ASC,
          CASE WHEN $14='net_quantity' AND NOT $15 THEN COALESCE(SUM(entry.quantity_delta),0) END DESC,
          transaction.id DESC
        OFFSET $16 LIMIT $17
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(query.facility_id.map(FacilityId::get))
    .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(query.item_id)
    .bind(query.item_batch_id)
    .bind(query.license_plate_id)
    .bind(query.transaction_id)
    .bind(query.search.as_deref())
    .bind(query_id)
    .bind(journal_sort_key(query.sort))
    .bind(query.direction.is_ascending())
    .bind(offset)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;

    let has_more = rows.len() > usize::from(query.limit);
    let rows = rows
        .into_iter()
        .take(usize::from(query.limit))
        .collect::<Vec<_>>();
    let transaction_ids = rows
        .iter()
        .map(|row| row.try_get::<i64, _>("id"))
        .collect::<Result<Vec<_>, _>>()?;
    let mut entries_by_transaction =
        load_entries(&mut tx, access, &scope, &transaction_ids).await?;
    let items = rows
        .iter()
        .map(|row| map_transaction(row, &mut entries_by_transaction))
        .collect::<AppResult<Vec<_>>>()?;
    let next_offset = has_more.then_some(query.offset + u64::from(query.limit));
    tx.commit().await?;
    Ok(InventoryJournalPage { items, next_offset })
}

async fn load_entries(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    scope: &ScopeBindings,
    transaction_ids: &[i64],
) -> AppResult<HashMap<i64, Vec<InventoryJournalEntryReadModel>>> {
    if transaction_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT entry.transaction_id, entry.id, entry.facility_id,
               facility.name AS facility_name, entry.location_id,
               location.name AS location_name, location.barcode AS location_barcode,
               entry.license_plate_id, plate.barcode AS license_plate_barcode,
               entry.item_batch_id, entry.item_id, sku.name AS primary_sku,
               item.description AS item_description, entry.uom, entry.lot,
               entry.expiration, entry.serial, entry.status, entry.quantity_delta
        FROM inventory_entries entry
        INNER JOIN facilities facility
          ON facility.tenant_id=entry.tenant_id AND facility.id=entry.facility_id
        INNER JOIN locations location
          ON location.tenant_id=entry.tenant_id AND location.id=entry.location_id
        INNER JOIN items item
          ON item.tenant_id=entry.tenant_id AND item.id=entry.item_id
        LEFT JOIN license_plates plate
          ON plate.tenant_id=entry.tenant_id
         AND plate.inventory_owner_id=entry.inventory_owner_id
         AND plate.id=entry.license_plate_id
        LEFT JOIN LATERAL (
            SELECT item_sku.name FROM skus item_sku
            WHERE item_sku.tenant_id=entry.tenant_id
              AND item_sku.item_id=entry.item_id
              AND item_sku.deleted IS NULL
            ORDER BY item_sku.id LIMIT 1
        ) sku ON TRUE
        WHERE entry.tenant_id=$1 AND entry.transaction_id=ANY($2)
          AND ($3 OR entry.facility_id=ANY($4))
          AND ($5 OR entry.inventory_owner_id=ANY($6))
        ORDER BY entry.transaction_id, entry.id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(transaction_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut grouped = HashMap::<i64, Vec<InventoryJournalEntryReadModel>>::new();
    for row in rows {
        let transaction_id: i64 = row.try_get("transaction_id")?;
        grouped
            .entry(transaction_id)
            .or_default()
            .push(map_entry(&row)?);
    }
    Ok(grouped)
}

pub async fn integrity_page(
    db: &Db,
    access: &TenantAccess,
    query: &InventoryIntegrityQuery,
) -> AppResult<InventoryIntegrityPage> {
    validate_integrity_query(query)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let fetch_limit = i64::from(query.limit) + 1;
    let offset = i64::try_from(query.offset)
        .map_err(|_| AppError::bad_request("inventory integrity cursor is out of range"))?;
    let rows = sqlx::query(
        r#"
        WITH issue AS (
          SELECT
            CONCAT('journal:', reconciliation.inventory_owner_id, ':',
                   reconciliation.facility_id, ':', reconciliation.location_id, ':',
                   COALESCE(reconciliation.license_plate_id, 0), ':',
                   reconciliation.item_batch_id, ':', reconciliation.status) AS issue_key,
            'journal_projection'::TEXT AS kind,
            reconciliation.inventory_owner_id, owner.name AS inventory_owner_name,
            reconciliation.facility_id, facility.name AS facility_name,
            reconciliation.location_id, location.name AS location_name,
            location.barcode AS location_barcode, reconciliation.license_plate_id,
            plate.barcode AS license_plate_barcode, reconciliation.item_batch_id,
            reconciliation.item_id, sku.name AS primary_sku,
            item.description AS item_description, batch.lot, batch.serial,
            reconciliation.uom, reconciliation.status,
            reconciliation.journal_qty AS journal_quantity,
            reconciliation.projected_qty AS projected_quantity,
            reconciliation.variance AS variance_quantity,
            NULL::BIGINT AS on_hand_quantity, NULL::BIGINT AS reserved_quantity,
            NULL::BIGINT AS allocated_quantity, NULL::BIGINT AS held_quantity,
            NULL::BIGINT AS hold_ledger_quantity, NULL::BIGINT AS overcommitted_quantity,
            ABS(reconciliation.variance)::BIGINT AS severity_quantity,
            ARRAY['journal_projection_mismatch']::TEXT[] AS issue_codes
          FROM inventory_reconciliation reconciliation
          INNER JOIN inventory_owners owner
            ON owner.tenant_id=reconciliation.tenant_id
           AND owner.id=reconciliation.inventory_owner_id
          INNER JOIN facilities facility
            ON facility.tenant_id=reconciliation.tenant_id
           AND facility.id=reconciliation.facility_id
          INNER JOIN locations location
            ON location.tenant_id=reconciliation.tenant_id
           AND location.id=reconciliation.location_id
          INNER JOIN item_batches batch
            ON batch.tenant_id=reconciliation.tenant_id
           AND batch.inventory_owner_id=reconciliation.inventory_owner_id
           AND batch.id=reconciliation.item_batch_id
          INNER JOIN items item
            ON item.tenant_id=reconciliation.tenant_id AND item.id=reconciliation.item_id
          LEFT JOIN license_plates plate
            ON plate.tenant_id=reconciliation.tenant_id
           AND plate.inventory_owner_id=reconciliation.inventory_owner_id
           AND plate.id=reconciliation.license_plate_id
          LEFT JOIN LATERAL (
            SELECT item_sku.name FROM skus item_sku
            WHERE item_sku.tenant_id=reconciliation.tenant_id
              AND item_sku.item_id=reconciliation.item_id
              AND item_sku.deleted IS NULL
            ORDER BY item_sku.id LIMIT 1
          ) sku ON TRUE
          WHERE reconciliation.tenant_id=$1
          UNION ALL
          SELECT
            CONCAT('commitment:', reconciliation.inventory_balance_id) AS issue_key,
            'commitments'::TEXT AS kind,
            reconciliation.inventory_owner_id, owner.name,
            reconciliation.facility_id, facility.name,
            reconciliation.location_id, location.name, location.barcode,
            reconciliation.license_plate_id, plate.barcode,
            reconciliation.item_batch_id, reconciliation.item_id, sku.name,
            item.description, batch.lot, batch.serial,
            reconciliation.uom, reconciliation.inventory_status,
            NULL::BIGINT, NULL::BIGINT, NULL::BIGINT,
            reconciliation.qty_on_hand, reconciliation.qty_reserved,
            reconciliation.allocated_qty, reconciliation.qty_held,
            reconciliation.held_qty, reconciliation.overcommitted_qty,
            GREATEST(
              ABS(reconciliation.qty_reserved-reconciliation.allocated_qty),
              ABS(reconciliation.qty_held-reconciliation.held_qty),
              reconciliation.overcommitted_qty
            )::BIGINT,
            reconciliation.issue_codes
          FROM inventory_hold_reconciliation reconciliation
          INNER JOIN inventory_owners owner
            ON owner.tenant_id=reconciliation.tenant_id
           AND owner.id=reconciliation.inventory_owner_id
          INNER JOIN facilities facility
            ON facility.tenant_id=reconciliation.tenant_id
           AND facility.id=reconciliation.facility_id
          INNER JOIN locations location
            ON location.tenant_id=reconciliation.tenant_id
           AND location.id=reconciliation.location_id
          INNER JOIN item_batches batch
            ON batch.tenant_id=reconciliation.tenant_id
           AND batch.inventory_owner_id=reconciliation.inventory_owner_id
           AND batch.id=reconciliation.item_batch_id
          INNER JOIN items item
            ON item.tenant_id=reconciliation.tenant_id AND item.id=reconciliation.item_id
          LEFT JOIN license_plates plate
            ON plate.tenant_id=reconciliation.tenant_id
           AND plate.inventory_owner_id=reconciliation.inventory_owner_id
           AND plate.id=reconciliation.license_plate_id
          LEFT JOIN LATERAL (
            SELECT item_sku.name FROM skus item_sku
            WHERE item_sku.tenant_id=reconciliation.tenant_id
              AND item_sku.item_id=reconciliation.item_id
              AND item_sku.deleted IS NULL
            ORDER BY item_sku.id LIMIT 1
          ) sku ON TRUE
          WHERE reconciliation.tenant_id=$1
        )
        SELECT * FROM issue
        WHERE ($2::TEXT IS NULL OR kind=$2)
          AND ($3 OR facility_id=ANY($4))
          AND ($5 OR inventory_owner_id=ANY($6))
          AND ($7::BIGINT IS NULL OR facility_id=$7)
          AND ($8::BIGINT IS NULL OR inventory_owner_id=$8)
          AND ($9::BIGINT IS NULL OR item_id=$9)
        ORDER BY
          CASE WHEN $10='severity' AND $11 THEN severity_quantity END ASC,
          CASE WHEN $10='severity' AND NOT $11 THEN severity_quantity END DESC,
          CASE WHEN $10='facility' AND $11 THEN LOWER(facility_name) END ASC,
          CASE WHEN $10='facility' AND NOT $11 THEN LOWER(facility_name) END DESC,
          CASE WHEN $10='client' AND $11 THEN LOWER(inventory_owner_name) END ASC,
          CASE WHEN $10='client' AND NOT $11 THEN LOWER(inventory_owner_name) END DESC,
          CASE WHEN $10='item' AND $11 THEN LOWER(COALESCE(primary_sku,item_description,'')) END ASC,
          CASE WHEN $10='item' AND NOT $11 THEN LOWER(COALESCE(primary_sku,item_description,'')) END DESC,
          issue_key ASC
        OFFSET $12 LIMIT $13
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(query.kind.map(issue_kind_key))
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(query.facility_id.map(FacilityId::get))
    .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(query.item_id)
    .bind(integrity_sort_key(query.sort))
    .bind(query.direction.is_ascending())
    .bind(offset)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(map_issue)
        .collect::<AppResult<Vec<_>>>()?;
    let next_offset = has_more.then_some(query.offset + u64::from(query.limit));
    tx.commit().await?;
    Ok(InventoryIntegrityPage { items, next_offset })
}

pub async fn aging_page(
    db: &Db,
    access: &TenantAccess,
    query: &InventoryAgingQuery,
) -> AppResult<InventoryAgingPage> {
    validate_aging_query(query)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let query_id = query
        .search
        .as_deref()
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0);
    let offset = i64::try_from(query.offset)
        .map_err(|_| AppError::bad_request("inventory aging cursor is out of range"))?;
    let fetch_limit = i64::from(query.limit) + 1;
    let rows = sqlx::query(
        r#"
        WITH aging AS (
          SELECT balance.id AS inventory_balance_id,
                 balance.inventory_owner_id, owner.name AS inventory_owner_name,
                 balance.facility_id, facility.name AS facility_name,
                 balance.location_id, location.name AS location_name,
                 location.barcode AS location_barcode,
                 balance.license_plate_id, plate.barcode AS license_plate_barcode,
                 balance.item_batch_id, balance.item_id, sku.name AS primary_sku,
                 item.description AS item_description, balance.uom,
                 batch.lot, batch.serial, batch.created AS received_at,
                 GREATEST((CURRENT_DATE-batch.created::DATE)::BIGINT,0) AS age_days,
                 batch.expiration,
                 CASE WHEN batch.expiration IS NULL THEN NULL
                      ELSE (batch.expiration::DATE-CURRENT_DATE)::BIGINT END AS days_to_expiration,
                 CASE WHEN batch.expiration IS NULL THEN 'no_expiration'
                      WHEN batch.expiration::DATE<CURRENT_DATE THEN 'expired'
                      WHEN batch.expiration::DATE<=CURRENT_DATE+7 THEN 'due_within_7_days'
                      WHEN batch.expiration::DATE<=CURRENT_DATE+30 THEN 'due_within_30_days'
                      WHEN batch.expiration::DATE<=CURRENT_DATE+90 THEN 'due_within_90_days'
                      ELSE 'beyond_90_days' END AS aging_bucket,
                 balance.status, balance.qty_on_hand, balance.qty_reserved,
                 balance.qty_held,
                 (balance.qty_on_hand-balance.qty_reserved-balance.qty_held)::BIGINT
                   AS available_quantity
          FROM inventory_balances balance
          INNER JOIN inventory_owners owner
            ON owner.tenant_id=balance.tenant_id AND owner.id=balance.inventory_owner_id
          INNER JOIN facilities facility
            ON facility.tenant_id=balance.tenant_id AND facility.id=balance.facility_id
          INNER JOIN locations location
            ON location.tenant_id=balance.tenant_id AND location.id=balance.location_id
          INNER JOIN item_batches batch
            ON batch.tenant_id=balance.tenant_id
           AND batch.inventory_owner_id=balance.inventory_owner_id
           AND batch.id=balance.item_batch_id
          INNER JOIN items item
            ON item.tenant_id=balance.tenant_id AND item.id=balance.item_id
          LEFT JOIN license_plates plate
            ON plate.tenant_id=balance.tenant_id
           AND plate.inventory_owner_id=balance.inventory_owner_id
           AND plate.id=balance.license_plate_id
          LEFT JOIN LATERAL (
            SELECT item_sku.name FROM skus item_sku
            WHERE item_sku.tenant_id=balance.tenant_id
              AND item_sku.item_id=balance.item_id
              AND item_sku.deleted IS NULL
            ORDER BY item_sku.id LIMIT 1
          ) sku ON TRUE
          WHERE balance.tenant_id=$1 AND balance.deleted IS NULL
            AND balance.qty_on_hand>0
            AND ($2 OR balance.facility_id=ANY($3))
            AND ($4 OR balance.inventory_owner_id=ANY($5))
            AND ($6::BIGINT IS NULL OR balance.facility_id=$6)
            AND ($7::BIGINT IS NULL OR balance.inventory_owner_id=$7)
            AND ($8::BIGINT IS NULL OR balance.item_id=$8)
        )
        SELECT * FROM aging
        WHERE ($9::TEXT IS NULL OR aging_bucket=$9)
          AND (
            $10::TEXT IS NULL
            OR STRPOS(LOWER(inventory_owner_name),LOWER($10))>0
            OR STRPOS(LOWER(facility_name),LOWER($10))>0
            OR STRPOS(LOWER(COALESCE(location_name,'')),LOWER($10))>0
            OR STRPOS(LOWER(COALESCE(location_barcode,'')),LOWER($10))>0
            OR STRPOS(LOWER(COALESCE(license_plate_barcode,'')),LOWER($10))>0
            OR STRPOS(LOWER(COALESCE(primary_sku,'')),LOWER($10))>0
            OR STRPOS(LOWER(COALESCE(item_description,'')),LOWER($10))>0
            OR STRPOS(LOWER(COALESCE(lot,'')),LOWER($10))>0
            OR STRPOS(LOWER(COALESCE(serial,'')),LOWER($10))>0
            OR ($11::BIGINT IS NOT NULL AND $11 IN (
              inventory_balance_id, inventory_owner_id, facility_id, location_id,
              item_batch_id, item_id, COALESCE(license_plate_id,0)
            ))
          )
        ORDER BY
          CASE WHEN $12='age' AND $13 THEN age_days END ASC,
          CASE WHEN $12='age' AND NOT $13 THEN age_days END DESC,
          CASE WHEN $12='expiration' AND $13 THEN expiration END ASC NULLS LAST,
          CASE WHEN $12='expiration' AND NOT $13 THEN expiration END DESC NULLS LAST,
          CASE WHEN $12='quantity' AND $13 THEN qty_on_hand END ASC,
          CASE WHEN $12='quantity' AND NOT $13 THEN qty_on_hand END DESC,
          CASE WHEN $12='facility' AND $13 THEN LOWER(facility_name) END ASC,
          CASE WHEN $12='facility' AND NOT $13 THEN LOWER(facility_name) END DESC,
          CASE WHEN $12='client' AND $13 THEN LOWER(inventory_owner_name) END ASC,
          CASE WHEN $12='client' AND NOT $13 THEN LOWER(inventory_owner_name) END DESC,
          CASE WHEN $12='item' AND $13 THEN LOWER(COALESCE(primary_sku,item_description,'')) END ASC,
          CASE WHEN $12='item' AND NOT $13 THEN LOWER(COALESCE(primary_sku,item_description,'')) END DESC,
          inventory_balance_id ASC
        OFFSET $14 LIMIT $15
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(query.facility_id.map(FacilityId::get))
    .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(query.item_id)
    .bind(query.bucket.map(aging_bucket_key))
    .bind(query.search.as_deref())
    .bind(query_id)
    .bind(aging_sort_key(query.sort))
    .bind(query.direction.is_ascending())
    .bind(offset)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(map_aging)
        .collect::<AppResult<Vec<_>>>()?;
    let next_offset = has_more.then_some(query.offset + u64::from(query.limit));
    tx.commit().await?;
    Ok(InventoryAgingPage { items, next_offset })
}

fn validate_journal_query(query: &InventoryJournalQuery) -> AppResult<()> {
    validate_limit(query.limit)?;
    for value in [
        query.facility_id.map(FacilityId::get),
        query.inventory_owner_id.map(InventoryOwnerId::get),
        query.item_id,
        query.item_batch_id,
        query.license_plate_id,
        query.transaction_id,
    ] {
        if value.is_some_and(|id| id <= 0) {
            return Err(AppError::bad_request(
                "inventory journal filter IDs must be positive",
            ));
        }
    }
    Ok(())
}

fn validate_integrity_query(query: &InventoryIntegrityQuery) -> AppResult<()> {
    validate_limit(query.limit)?;
    for value in [
        query.facility_id.map(FacilityId::get),
        query.inventory_owner_id.map(InventoryOwnerId::get),
        query.item_id,
    ] {
        if value.is_some_and(|id| id <= 0) {
            return Err(AppError::bad_request(
                "inventory integrity filter IDs must be positive",
            ));
        }
    }
    Ok(())
}

fn validate_aging_query(query: &InventoryAgingQuery) -> AppResult<()> {
    validate_limit(query.limit)?;
    for value in [
        query.facility_id.map(FacilityId::get),
        query.inventory_owner_id.map(InventoryOwnerId::get),
        query.item_id,
    ] {
        if value.is_some_and(|id| id <= 0) {
            return Err(AppError::bad_request(
                "inventory aging filter IDs must be positive",
            ));
        }
    }
    Ok(())
}

fn validate_limit(limit: u16) -> AppResult<()> {
    if !(1..=MAX_PAGE_SIZE).contains(&limit) {
        return Err(AppError::bad_request(
            "inventory integrity page limit must be 1..=1000",
        ));
    }
    Ok(())
}

fn journal_sort_key(sort: InventoryJournalSort) -> &'static str {
    match sort {
        InventoryJournalSort::OccurredAt => "occurred_at",
        InventoryJournalSort::Transaction => "transaction",
        InventoryJournalSort::Type => "type",
        InventoryJournalSort::Client => "client",
        InventoryJournalSort::NetQuantity => "net_quantity",
    }
}

fn integrity_sort_key(sort: InventoryIntegritySort) -> &'static str {
    match sort {
        InventoryIntegritySort::Severity => "severity",
        InventoryIntegritySort::Facility => "facility",
        InventoryIntegritySort::Client => "client",
        InventoryIntegritySort::Item => "item",
    }
}

fn aging_sort_key(sort: InventoryAgingSort) -> &'static str {
    match sort {
        InventoryAgingSort::Age => "age",
        InventoryAgingSort::Expiration => "expiration",
        InventoryAgingSort::Quantity => "quantity",
        InventoryAgingSort::Facility => "facility",
        InventoryAgingSort::Client => "client",
        InventoryAgingSort::Item => "item",
    }
}

fn aging_bucket_key(bucket: InventoryAgingBucket) -> &'static str {
    match bucket {
        InventoryAgingBucket::Expired => "expired",
        InventoryAgingBucket::DueWithin7Days => "due_within_7_days",
        InventoryAgingBucket::DueWithin30Days => "due_within_30_days",
        InventoryAgingBucket::DueWithin90Days => "due_within_90_days",
        InventoryAgingBucket::Beyond90Days => "beyond_90_days",
        InventoryAgingBucket::NoExpiration => "no_expiration",
    }
}

fn issue_kind_key(kind: InventoryIntegrityIssueKind) -> &'static str {
    match kind {
        InventoryIntegrityIssueKind::JournalProjection => "journal_projection",
        InventoryIntegrityIssueKind::Commitments => "commitments",
    }
}

fn parse_status(value: &str) -> AppResult<InventoryBalanceStatus> {
    InventoryBalanceStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("unknown inventory status {value:?}")))
}

fn map_transaction(
    row: &PgRow,
    entries_by_transaction: &mut HashMap<i64, Vec<InventoryJournalEntryReadModel>>,
) -> AppResult<InventoryJournalTransactionReadModel> {
    let id: i64 = row.try_get("id")?;
    let entry_count: i64 = row.try_get("entry_count")?;
    Ok(InventoryJournalTransactionReadModel {
        id,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        occurred_at: row.try_get("created")?,
        actor_user_id: row.try_get("actor_user_id")?,
        transaction_type: row.try_get("transaction_type")?,
        reason: row.try_get("reason")?,
        reference_type: row.try_get("reference_type")?,
        reference_id: row.try_get("reference_id")?,
        correlation_id: row.try_get("correlation_id")?,
        operation: row.try_get("operation")?,
        entry_count: u32::try_from(entry_count)
            .map_err(|_| AppError::internal("inventory journal entry count is out of range"))?,
        net_quantity: row.try_get("net_quantity")?,
        entries: entries_by_transaction.remove(&id).unwrap_or_default(),
    })
}

fn map_entry(row: &PgRow) -> AppResult<InventoryJournalEntryReadModel> {
    Ok(InventoryJournalEntryReadModel {
        id: row.try_get("id")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_name: row.try_get("facility_name")?,
        location_id: row.try_get("location_id")?,
        location_name: row.try_get("location_name")?,
        location_barcode: row.try_get("location_barcode")?,
        license_plate_id: row.try_get("license_plate_id")?,
        license_plate_barcode: row.try_get("license_plate_barcode")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        primary_sku: row.try_get("primary_sku")?,
        item_description: row.try_get("item_description")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        expiration: row.try_get("expiration")?,
        serial: row.try_get("serial")?,
        status: parse_status(&row.try_get::<String, _>("status")?)?,
        quantity_delta: row.try_get("quantity_delta")?,
    })
}

fn map_issue(row: &PgRow) -> AppResult<InventoryIntegrityIssueReadModel> {
    let kind = match row.try_get::<String, _>("kind")?.as_str() {
        "journal_projection" => InventoryIntegrityIssueKind::JournalProjection,
        "commitments" => InventoryIntegrityIssueKind::Commitments,
        value => {
            return Err(AppError::internal(format!(
                "unknown inventory issue kind {value:?}"
            )))
        }
    };
    Ok(InventoryIntegrityIssueReadModel {
        issue_key: row.try_get("issue_key")?,
        kind,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_name: row.try_get("facility_name")?,
        location_id: row.try_get("location_id")?,
        location_name: row.try_get("location_name")?,
        location_barcode: row.try_get("location_barcode")?,
        license_plate_id: row.try_get("license_plate_id")?,
        license_plate_barcode: row.try_get("license_plate_barcode")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        primary_sku: row.try_get("primary_sku")?,
        item_description: row.try_get("item_description")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        uom: row.try_get("uom")?,
        status: parse_status(&row.try_get::<String, _>("status")?)?,
        journal_quantity: row.try_get("journal_quantity")?,
        projected_quantity: row.try_get("projected_quantity")?,
        variance_quantity: row.try_get("variance_quantity")?,
        on_hand_quantity: row.try_get("on_hand_quantity")?,
        reserved_quantity: row.try_get("reserved_quantity")?,
        allocated_quantity: row.try_get("allocated_quantity")?,
        held_quantity: row.try_get("held_quantity")?,
        hold_ledger_quantity: row.try_get("hold_ledger_quantity")?,
        overcommitted_quantity: row.try_get("overcommitted_quantity")?,
        severity_quantity: row.try_get("severity_quantity")?,
        issue_codes: row.try_get("issue_codes")?,
    })
}

fn map_aging(row: &PgRow) -> AppResult<InventoryAgingReadModel> {
    let bucket = match row.try_get::<String, _>("aging_bucket")?.as_str() {
        "expired" => InventoryAgingBucket::Expired,
        "due_within_7_days" => InventoryAgingBucket::DueWithin7Days,
        "due_within_30_days" => InventoryAgingBucket::DueWithin30Days,
        "due_within_90_days" => InventoryAgingBucket::DueWithin90Days,
        "beyond_90_days" => InventoryAgingBucket::Beyond90Days,
        "no_expiration" => InventoryAgingBucket::NoExpiration,
        value => {
            return Err(AppError::internal(format!(
                "unknown inventory aging bucket {value:?}"
            )))
        }
    };
    Ok(InventoryAgingReadModel {
        inventory_balance_id: row.try_get("inventory_balance_id")?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_name: row.try_get("facility_name")?,
        location_id: row.try_get("location_id")?,
        location_name: row.try_get("location_name")?,
        location_barcode: row.try_get("location_barcode")?,
        license_plate_id: row.try_get("license_plate_id")?,
        license_plate_barcode: row.try_get("license_plate_barcode")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        primary_sku: row.try_get("primary_sku")?,
        item_description: row.try_get("item_description")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        serial: row.try_get("serial")?,
        received_at: row.try_get("received_at")?,
        age_days: row.try_get("age_days")?,
        expiration: row.try_get("expiration")?,
        days_to_expiration: row.try_get("days_to_expiration")?,
        bucket,
        status: parse_status(&row.try_get::<String, _>("status")?)?,
        on_hand_quantity: row.try_get("qty_on_hand")?,
        reserved_quantity: row.try_get("qty_reserved")?,
        held_quantity: row.try_get("qty_held")?,
        available_quantity: row.try_get("available_quantity")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_queries_reject_invalid_limits_and_ids() {
        let query = InventoryJournalQuery {
            search: None,
            facility_id: None,
            inventory_owner_id: None,
            item_id: Some(0),
            item_batch_id: None,
            license_plate_id: None,
            transaction_id: None,
            sort: InventoryJournalSort::OccurredAt,
            direction:
                wareboxes_application::inventory_integrity::InventorySortDirection::Descending,
            offset: 0,
            limit: 100,
        };
        assert!(validate_journal_query(&query).is_err());
        assert!(validate_limit(0).is_err());
        assert!(validate_limit(1_001).is_err());
    }
}
