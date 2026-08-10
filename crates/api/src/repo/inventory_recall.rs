//! Facility-scoped item-batch recall orchestration and read model.

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::inventory_recall::{
    CreateInventoryRecallCommand, CreateInventoryRecallResult, InventoryRecallCursor,
    InventoryRecallPage, InventoryRecallPageQuery, InventoryRecallReadModel,
    ReleaseInventoryRecallCommand, ReleaseInventoryRecallResult, CREATE_INVENTORY_RECALL_OPERATION,
    RELEASE_INVENTORY_RECALL_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::{InventoryHoldReason, TenantAccess, Timestamp};
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, InventoryRecallDetails, InventoryRecallId, InventoryRecallNote,
    InventoryRecallReason, InventoryRecallRevision, InventoryRecallStatus, ItemBatchId, TenantId,
    UserId,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::inventory::{
    place_composed_inventory_hold_tx, release_composed_inventory_hold_tx, PlaceInventoryHoldCommand,
};
use crate::repo::inventory_locking::lock_license_plate;

const PERMISSION: &str = "wms_supervisor";
const REFERENCE_TYPE: &str = "inventory_recall";

#[derive(Debug)]
struct BatchTarget {
    inventory_owner_id: i64,
    item_id: i64,
    uom: String,
    lot: Option<String>,
    expiration: Option<Timestamp>,
    serial: Option<String>,
}

#[derive(Debug)]
struct BalanceTarget {
    id: i64,
    license_plate_id: Option<i64>,
    qty_on_hand: i64,
    qty_reserved: i64,
    qty_held: i64,
}

fn require_scope(scope: &ScopeBindings, owner_id: i64, facility_id: i64) -> AppResult<()> {
    if scope.includes_inventory_owner(owner_id) && scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::not_found("inventory recall"))
    }
}

async fn lock_batch(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: i64,
    item_batch_id: i64,
) -> AppResult<BatchTarget> {
    let row = sqlx::query(
        r#"
        SELECT batch.inventory_owner_id, batch.item_id, batch.uom,
               batch.lot, batch.expiration, batch.serial
        FROM item_batches batch
        INNER JOIN inventory_owner_facilities owner_facility
          ON owner_facility.tenant_id=batch.tenant_id
         AND owner_facility.inventory_owner_id=batch.inventory_owner_id
         AND owner_facility.facility_id=$2 AND owner_facility.deleted IS NULL
        WHERE batch.tenant_id=$1 AND batch.id=$3 AND batch.deleted IS NULL
        FOR SHARE OF batch, owner_facility
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(item_batch_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("item batch"))?;
    Ok(BatchTarget {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        expiration: row.try_get("expiration")?,
        serial: row.try_get("serial")?,
    })
}

async fn lock_balances(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
    item_batch_id: i64,
) -> AppResult<Vec<BalanceTarget>> {
    let rows = sqlx::query(
        r#"
        SELECT id, license_plate_id, qty_on_hand, qty_reserved, qty_held
        FROM inventory_balances
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND item_batch_id=$4 AND deleted IS NULL AND qty_on_hand > 0
        ORDER BY id FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id)
    .bind(facility_id)
    .bind(item_batch_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(BalanceTarget {
                id: row.try_get("id")?,
                license_plate_id: row.try_get("license_plate_id")?,
                qty_on_hand: row.try_get("qty_on_hand")?,
                qty_reserved: row.try_get("qty_reserved")?,
                qty_held: row.try_get("qty_held")?,
            })
        })
        .collect()
}

fn hold_reason(reason: InventoryRecallReason) -> InventoryHoldReason {
    match reason {
        InventoryRecallReason::CustomerRequest => InventoryHoldReason::CustomerRequest,
        InventoryRecallReason::QualityConcern | InventoryRecallReason::SupplierNotice => {
            InventoryHoldReason::QualityInspection
        }
        InventoryRecallReason::Regulatory => InventoryHoldReason::Regulatory,
        InventoryRecallReason::Other => InventoryHoldReason::Other,
    }
}

fn parse_status(value: &str) -> AppResult<InventoryRecallStatus> {
    InventoryRecallStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid inventory recall status: {value}")))
}

fn parse_reason(value: &str) -> AppResult<InventoryRecallReason> {
    InventoryRecallReason::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid inventory recall reason: {value}")))
}

fn map_recall(row: &sqlx::postgres::PgRow) -> AppResult<InventoryRecallReadModel> {
    let reason = parse_reason(&row.try_get::<String, _>("reason_code")?)?;
    let note = row
        .try_get::<Option<String>, _>("note")?
        .map(InventoryRecallNote::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(InventoryRecallReadModel {
        recall_id: InventoryRecallId::new(row.try_get("recall_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_name: row.try_get("facility_name")?,
        item_batch_id: ItemBatchId::new(row.try_get("item_batch_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        item_id: row.try_get("item_id")?,
        primary_sku: row.try_get("primary_sku")?,
        item_description: row.try_get("item_description")?,
        uom: row.try_get("uom")?,
        lot: row.try_get("lot")?,
        expiration: row.try_get("expiration")?,
        serial: row.try_get("serial")?,
        status: parse_status(&row.try_get::<String, _>("state")?)?,
        revision: InventoryRecallRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        details: InventoryRecallDetails::new(reason, note)
            .map_err(|error| AppError::internal(error.to_string()))?,
        affected_position_count: u32::try_from(row.try_get::<i32, _>("affected_position_count")?)
            .map_err(|_| {
            AppError::internal("inventory recall position count is out of range")
        })?,
        held_quantity: row.try_get("held_qty")?,
        created_by: UserId::new(row.try_get("created_by_user_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created_at: row.try_get("created_at")?,
        released_by: row
            .try_get::<Option<i64>, _>("released_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        released_at: row.try_get("released_at")?,
    })
}

const RECALL_SELECT: &str = r#"
    SELECT recall.id AS recall_id, recall.inventory_owner_id,
           owner.name AS inventory_owner_name, recall.facility_id,
           facility.name AS facility_name, recall.item_batch_id,
           recall.item_id, sku.name AS primary_sku,
           item.description AS item_description, recall.uom, recall.lot,
           recall.expiration, recall.serial, recall.state, recall.revision,
           recall.reason_code, recall.note, recall.affected_position_count,
           recall.held_qty, recall.created_by_user_id, recall.created_at,
           recall.released_by_user_id, recall.released_at
    FROM inventory_recall_cases recall
    INNER JOIN inventory_owners owner
      ON owner.tenant_id=recall.tenant_id AND owner.id=recall.inventory_owner_id
    INNER JOIN facilities facility
      ON facility.tenant_id=recall.tenant_id AND facility.id=recall.facility_id
    INNER JOIN items item
      ON item.tenant_id=recall.tenant_id AND item.id=recall.item_id
    LEFT JOIN LATERAL (
        SELECT item_sku.name FROM skus item_sku
        WHERE item_sku.tenant_id=recall.tenant_id
          AND item_sku.item_id=recall.item_id AND item_sku.deleted IS NULL
        ORDER BY item_sku.id LIMIT 1
    ) sku ON TRUE
"#;

async fn load_recall_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    recall_id: i64,
) -> AppResult<InventoryRecallReadModel> {
    let sql = format!("{RECALL_SELECT} WHERE recall.tenant_id=$1 AND recall.id=$2");
    let row = sqlx::query(&sql)
        .bind(tenant_id.get())
        .bind(recall_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("inventory recall"))?;
    map_recall(&row)
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_recall_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
    actor_id: i64,
    recall_id: i64,
    sequence: i64,
    transition: &str,
    occurred_at: Timestamp,
    payload: &serde_json::Value,
) -> AppResult<()> {
    let event_key = format!("inventory-recall:{recall_id}:{transition}");
    let aggregate_id = recall_id.to_string();
    let ordering_key = format!("inventory-recall:{recall_id}");
    let event_type = format!("inventory.recall.{transition}");
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(
                InventoryOwnerId::new(owner_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            facility_id: Some(
                FacilityId::new(facility_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            actor_user_id: Some(actor_id),
            event_key: &event_key,
            aggregate_type: "inventory_recall",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: &event_type,
            schema_version: 1,
            payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

pub async fn create_inventory_recall(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreateInventoryRecallCommand,
) -> AppResult<CreateInventoryRecallResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CREATE_INVENTORY_RECALL_OPERATION, command)?;
    let now = now_iso();
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    let batch = lock_batch(
        &mut tx,
        access.tenant_id,
        command.facility_id.get(),
        command.item_batch_id.get(),
    )
    .await?;
    require_scope(&scope, batch.inventory_owner_id, command.facility_id.get())?;
    if let Some(result) = prepared
        .replayed::<CreateInventoryRecallResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let plate_hints = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT DISTINCT license_plate_id FROM inventory_balances
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND item_batch_id=$4 AND deleted IS NULL AND qty_on_hand > 0
          AND license_plate_id IS NOT NULL
        ORDER BY license_plate_id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(batch.inventory_owner_id)
    .bind(command.facility_id.get())
    .bind(command.item_batch_id.get())
    .fetch_all(&mut *tx)
    .await?;
    for plate_id in &plate_hints {
        lock_license_plate(&mut tx, access.tenant_id, Some(*plate_id)).await?;
    }
    let balances = lock_balances(
        &mut tx,
        access.tenant_id,
        batch.inventory_owner_id,
        command.facility_id.get(),
        command.item_batch_id.get(),
    )
    .await?;
    let mut locked_plate_ids = balances
        .iter()
        .filter_map(|balance| balance.license_plate_id)
        .collect::<Vec<_>>();
    locked_plate_ids.sort_unstable();
    locked_plate_ids.dedup();
    if locked_plate_ids != plate_hints {
        return Err(AppError::conflict(
            "inventory batch positions changed while acquiring locks",
        ));
    }
    if balances.is_empty() {
        return Err(AppError::conflict(
            "item batch has no positive inventory positions at this facility",
        ));
    }
    if balances
        .iter()
        .any(|balance| balance.qty_reserved > 0 || balance.qty_held > 0)
    {
        return Err(AppError::conflict(
            "item batch recall requires every position to be unreserved and unheld",
        ));
    }
    let position_count = i32::try_from(balances.len())
        .map_err(|_| AppError::conflict("item batch has too many inventory positions"))?;
    let held_qty = balances.iter().try_fold(0_i64, |total, balance| {
        total
            .checked_add(balance.qty_on_hand)
            .ok_or_else(|| AppError::internal("inventory recall quantity overflow"))
    })?;
    let recall_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO inventory_recall_cases (
          tenant_id, inventory_owner_id, facility_id, item_batch_id, item_id,
          uom, lot, expiration, serial, state, revision, reason_code, note,
          affected_position_count, held_qty, created_by_user_id, created_at, modified_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'active',1,$10,$11,$12,$13,$14,$15,$15)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(batch.inventory_owner_id)
    .bind(command.facility_id.get())
    .bind(command.item_batch_id.get())
    .bind(batch.item_id)
    .bind(&batch.uom)
    .bind(&batch.lot)
    .bind(batch.expiration)
    .bind(&batch.serial)
    .bind(command.details.reason().as_str())
    .bind(command.details.note().map(InventoryRecallNote::as_str))
    .bind(position_count)
    .bind(held_qty)
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;

    for balance in &balances {
        let hold_id = place_composed_inventory_hold_tx(
            &mut tx,
            access.tenant_id,
            context.actor_id.get(),
            now,
            &PlaceInventoryHoldCommand {
                inventory_balance_id: balance.id,
                qty: balance.qty_on_hand,
                reason: hold_reason(command.details.reason()),
                note: command.details.note().map(InventoryRecallNote::as_str),
                reference_type: Some(REFERENCE_TYPE),
                reference_id: Some(recall_id),
            },
        )
        .await?;
        sqlx::query(
            r#"
            INSERT INTO inventory_recall_case_holds (
              tenant_id, inventory_owner_id, facility_id, recall_case_id,
              inventory_hold_id, inventory_balance_id, held_qty, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(batch.inventory_owner_id)
        .bind(command.facility_id.get())
        .bind(recall_id)
        .bind(hold_id)
        .bind(balance.id)
        .bind(balance.qty_on_hand)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    let result = load_recall_tx(&mut tx, access.tenant_id, recall_id).await?;
    enqueue_recall_event(
        &mut tx,
        access.tenant_id,
        batch.inventory_owner_id,
        command.facility_id.get(),
        context.actor_id.get(),
        recall_id,
        1,
        "created",
        now,
        &serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn release_inventory_recall(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReleaseInventoryRecallCommand,
) -> AppResult<ReleaseInventoryRecallResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, RELEASE_INVENTORY_RECALL_OPERATION, command)?;
    let now = now_iso();
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    let hint = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id
        FROM inventory_recall_cases WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.recall_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory recall"))?;
    let owner_id: i64 = hint.try_get("inventory_owner_id")?;
    let facility_id: i64 = hint.try_get("facility_id")?;
    require_scope(&scope, owner_id, facility_id)?;
    let plate_ids = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT DISTINCT hold.license_plate_id
        FROM inventory_recall_case_holds link
        INNER JOIN inventory_holds hold
          ON hold.tenant_id=link.tenant_id AND hold.id=link.inventory_hold_id
        WHERE link.tenant_id=$1 AND link.recall_case_id=$2
          AND hold.license_plate_id IS NOT NULL
        ORDER BY hold.license_plate_id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.recall_id.get())
    .fetch_all(&mut *tx)
    .await?;
    for plate_id in plate_ids {
        lock_license_plate(&mut tx, access.tenant_id, Some(plate_id)).await?;
    }
    let case = sqlx::query(
        "SELECT state, revision FROM inventory_recall_cases WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(access.tenant_id.get())
    .bind(command.recall_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("inventory recall"))?;
    if let Some(result) = prepared
        .replayed::<ReleaseInventoryRecallResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    if case.try_get::<String, _>("state")? != InventoryRecallStatus::Active.as_str() {
        return Err(AppError::conflict("inventory recall is not active"));
    }
    if case.try_get::<i64, _>("revision")? != command.expected_revision.get() {
        return Err(AppError::conflict("inventory recall revision is stale"));
    }
    let hold_ids = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT inventory_hold_id FROM inventory_recall_case_holds
        WHERE tenant_id=$1 AND recall_case_id=$2 ORDER BY inventory_hold_id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.recall_id.get())
    .fetch_all(&mut *tx)
    .await?;
    for hold_id in hold_ids {
        release_composed_inventory_hold_tx(
            &mut tx,
            access.tenant_id,
            context.actor_id.get(),
            now,
            hold_id,
            REFERENCE_TYPE,
            command.recall_id.get(),
        )
        .await?;
    }
    let updated = sqlx::query(
        r#"
        UPDATE inventory_recall_cases
        SET state='released', revision=revision+1, modified_at=$1,
            released_by_user_id=$2, released_at=$1
        WHERE tenant_id=$3 AND id=$4 AND state='active' AND revision=$5
        "#,
    )
    .bind(now)
    .bind(context.actor_id.get())
    .bind(access.tenant_id.get())
    .bind(command.recall_id.get())
    .bind(command.expected_revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("inventory recall could not be released"));
    }
    let result = load_recall_tx(&mut tx, access.tenant_id, command.recall_id.get()).await?;
    enqueue_recall_event(
        &mut tx,
        access.tenant_id,
        owner_id,
        facility_id,
        context.actor_id.get(),
        command.recall_id.get(),
        2,
        "released",
        now,
        &serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn inventory_recall_page(
    db: &Db,
    access: &TenantAccess,
    query: &InventoryRecallPageQuery,
) -> AppResult<InventoryRecallPage> {
    if query.limit == 0 || query.limit > 1_000 {
        return Err(AppError::bad_request(
            "inventory recall page limit must be between 1 and 1000",
        ));
    }
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;
    let sql = format!(
        "{RECALL_SELECT} WHERE recall.tenant_id=$1
         AND ($2 OR recall.facility_id=ANY($3))
         AND ($4 OR recall.inventory_owner_id=ANY($5))
         AND ($6::BIGINT IS NULL OR recall.facility_id=$6)
         AND ($7::BIGINT IS NULL OR recall.inventory_owner_id=$7)
         AND ($8::TEXT IS NULL OR recall.state=$8)
         AND ($9::BIGINT IS NULL OR recall.id < $9)
         ORDER BY recall.id DESC LIMIT $10"
    );
    let rows = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .bind(query.facility_id.map(FacilityId::get))
        .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
        .bind(query.status.map(InventoryRecallStatus::as_str))
        .bind(query.cursor.map(|cursor| cursor.before_id.get()))
        .bind(i64::from(query.limit) + 1)
        .fetch_all(&mut *tx)
        .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(map_recall)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = has_more
        .then(|| {
            items.last().map(|item| InventoryRecallCursor {
                before_id: item.recall_id,
            })
        })
        .flatten();
    tx.commit().await?;
    Ok(InventoryRecallPage { items, next_cursor })
}
