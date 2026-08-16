use sqlx::Row;
use wareboxes_application::CommandContext;
use wareboxes_core::models::{
    InventoryRelocationWorkflow, InventoryStatus, TenantAccess, Timestamp, WorkTaskType,
};
use wareboxes_domain::TenantId;

use crate::db::{bind_tenant_context, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;
use crate::repo::inventory;
use crate::repo::inventory_journal;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use super::license_plate_tree::{lock_root_tree_tx, require_no_active_tree_movement_tx};
use super::{
    insert_task_tx, lock_current_task_scope_tx, require_replayed_task_visible_tx, task_permission,
    task_timeout_seconds, NewWorkTask, TaskDimensions,
};

const CREATE_LOOSE_OPERATION: &str = "task.create_inventory_relocation.loose.v1";
const CREATE_PLATE_OPERATION: &str = "task.create_inventory_relocation.license_plate.v1";

#[derive(Debug)]
struct LooseSource {
    inventory_owner_id: i64,
    facility_id: i64,
    location_id: i64,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    status: InventoryStatus,
    qty_on_hand: i64,
    qty_reserved: i64,
    qty_held: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PlateContent {
    pub(super) inventory_balance_id: i64,
    pub(super) license_plate_id: i64,
    pub(super) location_id: i64,
    pub(super) item_batch_id: i64,
    pub(super) item_id: i64,
    pub(super) uom: String,
    pub(super) status: InventoryStatus,
    pub(super) quantity: i64,
    pub(super) qty_reserved: i64,
    pub(super) qty_held: i64,
}

#[derive(Debug)]
pub(super) struct RelocationTarget {
    pub(super) task_id: i64,
    pub(super) workflow: InventoryRelocationWorkflow,
    pub(super) inventory_owner_id: i64,
    pub(super) facility_id: i64,
    pub(super) source_inventory_balance_id: Option<i64>,
    pub(super) license_plate_id: Option<i64>,
    pub(super) source_location_id: i64,
    pub(super) destination_location_id: i64,
    pub(super) item_batch_id: Option<i64>,
    pub(super) item_id: Option<i64>,
    pub(super) uom: Option<String>,
    pub(super) status: Option<InventoryStatus>,
    pub(super) quantity: Option<i64>,
    pub(super) planned_balance_count: Option<i64>,
}

#[allow(clippy::too_many_arguments)]
pub async fn create_loose_inventory_relocation_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    source_inventory_balance_id: i64,
    destination_location_id: i64,
    quantity: i64,
    priority: i64,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<&str>,
) -> AppResult<i64> {
    command.require_actor(access.tenant_id, access.user_id)?;
    validate_creation(
        source_inventory_balance_id,
        destination_location_id,
        quantity,
        priority,
        instructions,
    )?;
    let prepared = PreparedCommand::new_v1(
        command,
        CREATE_LOOSE_OPERATION,
        &(
            source_inventory_balance_id,
            destination_location_id,
            quantity,
            priority,
            assigned_user_id,
            scheduled_for,
            due_at,
            instructions,
        ),
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_task_scope_tx(
        &mut tx,
        access.tenant_id,
        command.actor_id.get(),
        assigned_user_id,
    )
    .await?;
    if let Some(task_id) = prepared.replayed::<i64>(&mut tx).await? {
        require_replayed_task_visible_tx(&mut tx, access.tenant_id, task_id, &scope).await?;
        tx.commit().await?;
        return Ok(task_id);
    }

    let source = lock_loose_source(&mut tx, access.tenant_id, source_inventory_balance_id).await?;
    require_dimensions(
        &scope,
        source.facility_id,
        source.inventory_owner_id,
        "relocation source inventory",
    )?;
    if source.location_id == destination_location_id {
        return Err(AppError::bad_request(
            "relocation source and destination locations must differ",
        ));
    }
    lock_relocation_destination(
        &mut tx,
        access.tenant_id,
        source.facility_id,
        destination_location_id,
    )
    .await?;
    inventory::ensure_location_accepts_batch_tx(
        &mut tx,
        access.tenant_id,
        source.inventory_owner_id,
        destination_location_id,
        source.item_batch_id,
    )
    .await?;
    let owner_facility =
        inventory_journal::owner_facility_scope(source.inventory_owner_id, source.facility_id)?;
    inventory_journal::lock_active_owner_facility_tx(&mut tx, access.tenant_id, owner_facility)
        .await?;
    let movable = movable_quantity(source.qty_on_hand, source.qty_reserved, source.qty_held)?;
    if movable < quantity {
        return Err(AppError::conflict(
            "insufficient uncommitted inventory for relocation",
        ));
    }
    require_no_active_loose_movement(&mut tx, access.tenant_id, source_inventory_balance_id)
        .await?;

    let task_id = insert_task_tx(
        &mut tx,
        access.tenant_id,
        NewWorkTask {
            facility_id: Some(source.facility_id),
            inventory_owner_id: Some(source.inventory_owner_id),
            task_type: WorkTaskType::InventoryRelocation,
            title: "Relocate inventory".to_owned(),
            instructions: instructions.map(str::to_owned),
            required_permission: task_permission(WorkTaskType::InventoryRelocation).to_owned(),
            priority,
            task_timeout_seconds: task_timeout_seconds(WorkTaskType::InventoryRelocation),
            assigned_user_id,
            created_by: Some(command.actor_id.get()),
            scheduled_for,
            due_at,
            metadata_json: None,
        },
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO inventory_relocation_tasks (
            tenant_id, task_id, inventory_owner_id, facility_id, workflow,
            source_inventory_balance_id, source_location_id,
            destination_location_id, item_batch_id, item_id, uom,
            inventory_status, planned_quantity
        )
        VALUES (
            $1, $2, $3, $4, 'loose_balance', $5, $6, $7, $8, $9, $10,
            $11, $12
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(source.inventory_owner_id)
    .bind(source.facility_id)
    .bind(source_inventory_balance_id)
    .bind(source.location_id)
    .bind(destination_location_id)
    .bind(source.item_batch_id)
    .bind(source.item_id)
    .bind(&source.uom)
    .bind(source.status.as_str())
    .bind(quantity)
    .execute(&mut *tx)
    .await?;
    Ok(prepared.commit(tx, task_id).await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_license_plate_inventory_relocation_task_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    license_plate_id: i64,
    destination_location_id: i64,
    priority: i64,
    assigned_user_id: Option<i64>,
    scheduled_for: Option<Timestamp>,
    due_at: Option<Timestamp>,
    instructions: Option<&str>,
) -> AppResult<i64> {
    command.require_actor(access.tenant_id, access.user_id)?;
    validate_creation(
        license_plate_id,
        destination_location_id,
        1,
        priority,
        instructions,
    )?;
    let prepared = PreparedCommand::new_v1(
        command,
        CREATE_PLATE_OPERATION,
        &(
            license_plate_id,
            destination_location_id,
            priority,
            assigned_user_id,
            scheduled_for,
            due_at,
            instructions,
        ),
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_task_scope_tx(
        &mut tx,
        access.tenant_id,
        command.actor_id.get(),
        assigned_user_id,
    )
    .await?;
    if let Some(task_id) = prepared.replayed::<i64>(&mut tx).await? {
        require_replayed_task_visible_tx(&mut tx, access.tenant_id, task_id, &scope).await?;
        tx.commit().await?;
        return Ok(task_id);
    }

    let plate = lock_root_tree_tx(&mut tx, access.tenant_id, license_plate_id).await?;
    require_dimensions(
        &scope,
        plate.facility_id,
        plate.inventory_owner_id,
        "license plate",
    )?;
    if plate.location_id == destination_location_id {
        return Err(AppError::bad_request(
            "relocation source and destination locations must differ",
        ));
    }
    lock_relocation_destination(
        &mut tx,
        access.tenant_id,
        plate.facility_id,
        destination_location_id,
    )
    .await?;
    let owner_facility =
        inventory_journal::owner_facility_scope(plate.inventory_owner_id, plate.facility_id)?;
    inventory_journal::lock_active_owner_facility_tx(&mut tx, access.tenant_id, owner_facility)
        .await?;
    let contents = lock_plate_contents(
        &mut tx,
        access.tenant_id,
        plate.inventory_owner_id,
        plate.facility_id,
        &plate.plate_ids,
    )
    .await?;
    let positive_contents = require_movable_plate_contents(&contents, plate.location_id)?;
    require_plate_destination_compatible(
        &mut tx,
        access.tenant_id,
        plate.inventory_owner_id,
        destination_location_id,
        &positive_contents,
    )
    .await?;
    require_no_active_tree_movement_tx(
        &mut tx,
        access.tenant_id,
        plate.inventory_owner_id,
        &plate.plate_ids,
    )
    .await?;

    let planned_balance_count = i64::try_from(positive_contents.len())
        .map_err(|_| AppError::internal("license plate content count is out of range"))?;
    let task_id = insert_task_tx(
        &mut tx,
        access.tenant_id,
        NewWorkTask {
            facility_id: Some(plate.facility_id),
            inventory_owner_id: Some(plate.inventory_owner_id),
            task_type: WorkTaskType::InventoryRelocation,
            title: "Relocate license plate".to_owned(),
            instructions: instructions.map(str::to_owned),
            required_permission: task_permission(WorkTaskType::InventoryRelocation).to_owned(),
            priority,
            task_timeout_seconds: task_timeout_seconds(WorkTaskType::InventoryRelocation),
            assigned_user_id,
            created_by: Some(command.actor_id.get()),
            scheduled_for,
            due_at,
            metadata_json: None,
        },
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO inventory_relocation_tasks (
            tenant_id, task_id, inventory_owner_id, facility_id, workflow,
            license_plate_id, source_location_id, destination_location_id,
            planned_balance_count
        )
        VALUES ($1, $2, $3, $4, 'license_plate', $5, $6, $7, $8)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(plate.inventory_owner_id)
    .bind(plate.facility_id)
    .bind(license_plate_id)
    .bind(plate.location_id)
    .bind(destination_location_id)
    .bind(planned_balance_count)
    .execute(&mut *tx)
    .await?;
    for content in &positive_contents {
        sqlx::query(
            r#"
            INSERT INTO inventory_relocation_task_contents (
                tenant_id, task_id, inventory_owner_id, facility_id,
                license_plate_id, content_license_plate_id, inventory_balance_id, item_batch_id,
                item_id, uom, inventory_status, planned_quantity
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(task_id)
        .bind(plate.inventory_owner_id)
        .bind(plate.facility_id)
        .bind(license_plate_id)
        .bind(content.license_plate_id)
        .bind(content.inventory_balance_id)
        .bind(content.item_batch_id)
        .bind(content.item_id)
        .bind(&content.uom)
        .bind(content.status.as_str())
        .bind(content.quantity)
        .execute(&mut *tx)
        .await?;
    }
    Ok(prepared.commit(tx, task_id).await?)
}

fn validate_creation(
    source_id: i64,
    destination_location_id: i64,
    quantity: i64,
    priority: i64,
    instructions: Option<&str>,
) -> AppResult<()> {
    if source_id <= 0 || destination_location_id <= 0 || quantity <= 0 {
        return Err(AppError::bad_request(
            "relocation source, destination, and quantity must be positive",
        ));
    }
    if priority < 0 {
        return Err(AppError::bad_request(
            "inventory relocation priority cannot be negative",
        ));
    }
    if let Some(instructions) = instructions {
        if instructions.trim() != instructions
            || instructions.is_empty()
            || instructions.chars().count() > 1_000
        {
            return Err(AppError::bad_request(
                "relocation instructions must be trimmed, nonempty, and at most 1000 characters",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_barcode(value: &str, label: &str) -> AppResult<()> {
    if value.trim() != value || value.is_empty() || value.chars().count() > 200 {
        return Err(AppError::bad_request(format!(
            "{label} must be trimmed, nonempty, and at most 200 characters"
        )));
    }
    Ok(())
}

pub(super) fn parse_inventory_status(value: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {value}")))
}

fn require_dimensions(
    scope: &ScopeBindings,
    facility_id: i64,
    inventory_owner_id: i64,
    label: &str,
) -> AppResult<()> {
    if (TaskDimensions {
        facility_id: Some(facility_id),
        inventory_owner_id: Some(inventory_owner_id),
    })
    .is_allowed_by(scope)
    {
        Ok(())
    } else {
        Err(AppError::not_found(label))
    }
}

pub(super) fn movable_quantity(on_hand: i64, reserved: i64, held: i64) -> AppResult<i64> {
    on_hand
        .checked_sub(reserved)
        .and_then(|quantity| quantity.checked_sub(held))
        .ok_or_else(|| AppError::internal("inventory commitments are out of range"))
}

async fn lock_loose_source(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    balance_id: i64,
) -> AppResult<LooseSource> {
    let row = sqlx::query(
        r#"
        SELECT balance.inventory_owner_id, balance.facility_id,
               balance.location_id, balance.item_batch_id, balance.item_id,
               balance.uom, balance.status, balance.qty_on_hand,
               balance.qty_reserved, balance.qty_held, balance.license_plate_id
        FROM inventory_balances balance
        INNER JOIN locations location
          ON location.tenant_id = balance.tenant_id
         AND location.facility_id = balance.facility_id
         AND location.id = balance.location_id
        INNER JOIN item_batches batch
          ON batch.tenant_id = balance.tenant_id
         AND batch.inventory_owner_id = balance.inventory_owner_id
         AND batch.id = balance.item_batch_id
         AND batch.deleted IS NULL
        INNER JOIN items item
          ON item.tenant_id = balance.tenant_id
         AND item.id = balance.item_id
         AND item.deleted IS NULL
        WHERE balance.tenant_id = $1
          AND balance.id = $2
          AND balance.deleted IS NULL
          AND location.deleted IS NULL
          AND location.active
        FOR UPDATE OF balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(balance_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("relocation source inventory"))?;
    if row.try_get::<Option<i64>, _>("license_plate_id")?.is_some() {
        return Err(AppError::conflict(
            "license-plated inventory requires a whole-license-plate relocation",
        ));
    }
    Ok(LooseSource {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        location_id: row.try_get("location_id")?,
        item_batch_id: row.try_get("item_batch_id")?,
        item_id: row.try_get("item_id")?,
        uom: row.try_get("uom")?,
        status: parse_inventory_status(&row.try_get::<String, _>("status")?)?,
        qty_on_hand: row.try_get("qty_on_hand")?,
        qty_reserved: row.try_get("qty_reserved")?,
        qty_held: row.try_get("qty_held")?,
    })
}

/// Creates the same typed loose-balance relocation work used by the manual API,
/// but inside a caller-owned transaction. Advisory planners use this only after
/// their recommendation decision row has been locked and revalidated.
pub(in crate::repo) struct AdvisoryLooseRelocation<'a> {
    pub actor_id: i64,
    pub source_inventory_balance_id: i64,
    pub destination_location_id: i64,
    pub quantity: i64,
    pub priority: i64,
    pub instructions: Option<&'a str>,
    pub metadata_json: &'a str,
}

pub(in crate::repo) async fn create_advisory_loose_relocation_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    command: AdvisoryLooseRelocation<'_>,
) -> AppResult<i64> {
    validate_creation(
        command.source_inventory_balance_id,
        command.destination_location_id,
        command.quantity,
        command.priority,
        command.instructions,
    )?;
    let source = lock_loose_source(tx, tenant_id, command.source_inventory_balance_id).await?;
    if source.location_id == command.destination_location_id {
        return Err(AppError::conflict(
            "relocation source and destination locations must differ",
        ));
    }
    lock_relocation_destination(
        tx,
        tenant_id,
        source.facility_id,
        command.destination_location_id,
    )
    .await?;
    inventory::ensure_location_accepts_batch_tx(
        tx,
        tenant_id,
        source.inventory_owner_id,
        command.destination_location_id,
        source.item_batch_id,
    )
    .await?;
    let owner_facility =
        inventory_journal::owner_facility_scope(source.inventory_owner_id, source.facility_id)?;
    inventory_journal::lock_active_owner_facility_tx(tx, tenant_id, owner_facility).await?;
    let movable = movable_quantity(source.qty_on_hand, source.qty_reserved, source.qty_held)?;
    if movable < command.quantity {
        return Err(AppError::conflict(
            "recommended source inventory is no longer available",
        ));
    }
    require_no_active_loose_movement(tx, tenant_id, command.source_inventory_balance_id).await?;

    let task_id = insert_task_tx(
        tx,
        tenant_id,
        NewWorkTask {
            facility_id: Some(source.facility_id),
            inventory_owner_id: Some(source.inventory_owner_id),
            task_type: WorkTaskType::InventoryRelocation,
            title: "Execute accepted slotting recommendation".to_owned(),
            instructions: command.instructions.map(str::to_owned),
            required_permission: task_permission(WorkTaskType::InventoryRelocation).to_owned(),
            priority: command.priority,
            task_timeout_seconds: task_timeout_seconds(WorkTaskType::InventoryRelocation),
            assigned_user_id: None,
            created_by: Some(command.actor_id),
            scheduled_for: None,
            due_at: None,
            metadata_json: Some(command.metadata_json.to_owned()),
        },
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO inventory_relocation_tasks (
            tenant_id, task_id, inventory_owner_id, facility_id, workflow,
            source_inventory_balance_id, source_location_id,
            destination_location_id, item_batch_id, item_id, uom,
            inventory_status, planned_quantity
        ) VALUES ($1,$2,$3,$4,'loose_balance',$5,$6,$7,$8,$9,$10,$11,$12)
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(source.inventory_owner_id)
    .bind(source.facility_id)
    .bind(command.source_inventory_balance_id)
    .bind(source.location_id)
    .bind(command.destination_location_id)
    .bind(source.item_batch_id)
    .bind(source.item_id)
    .bind(source.uom)
    .bind(source.status.as_str())
    .bind(command.quantity)
    .execute(&mut **tx)
    .await?;
    Ok(task_id)
}

pub(super) async fn lock_plate_contents(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    license_plate_ids: &[i64],
) -> AppResult<Vec<PlateContent>> {
    let rows = sqlx::query(
        r#"
        SELECT balance.id, balance.license_plate_id, balance.location_id, balance.item_batch_id,
               balance.item_id, balance.uom, balance.status,
               balance.qty_on_hand, balance.qty_reserved, balance.qty_held
        FROM inventory_balances balance
        INNER JOIN item_batches batch
          ON batch.tenant_id = balance.tenant_id
         AND batch.inventory_owner_id = balance.inventory_owner_id
         AND batch.id = balance.item_batch_id
         AND batch.deleted IS NULL
        INNER JOIN items item
          ON item.tenant_id = balance.tenant_id
         AND item.id = balance.item_id
         AND item.deleted IS NULL
        WHERE balance.tenant_id = $1
          AND balance.inventory_owner_id = $2
          AND balance.facility_id = $3
          AND balance.license_plate_id = ANY($4)
          AND balance.deleted IS NULL
        ORDER BY balance.id
        FOR UPDATE OF balance
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .bind(license_plate_ids)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(PlateContent {
                inventory_balance_id: row.try_get("id")?,
                license_plate_id: row.try_get("license_plate_id")?,
                location_id: row.try_get("location_id")?,
                item_batch_id: row.try_get("item_batch_id")?,
                item_id: row.try_get("item_id")?,
                uom: row.try_get("uom")?,
                status: parse_inventory_status(&row.try_get::<String, _>("status")?)?,
                quantity: row.try_get("qty_on_hand")?,
                qty_reserved: row.try_get("qty_reserved")?,
                qty_held: row.try_get("qty_held")?,
            })
        })
        .collect()
}

pub(super) fn require_movable_plate_contents(
    contents: &[PlateContent],
    source_location_id: i64,
) -> AppResult<Vec<PlateContent>> {
    let positive = contents
        .iter()
        .filter(|content| content.quantity > 0)
        .cloned()
        .collect::<Vec<_>>();
    if positive.is_empty()
        || contents
            .iter()
            .any(|content| content.location_id != source_location_id)
        || positive
            .iter()
            .any(|content| content.qty_reserved != 0 || content.qty_held != 0)
    {
        return Err(AppError::conflict(
            "license plate contents must be colocated and uncommitted for relocation",
        ));
    }
    Ok(positive)
}

pub(super) async fn lock_relocation_destination(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: i64,
    location_id: i64,
) -> AppResult<String> {
    sqlx::query_scalar(
        r#"
        SELECT barcode
        FROM locations
        WHERE tenant_id = $1
          AND facility_id = $2
          AND id = $3
          AND deleted IS NULL
          AND active
          AND barcode IS NOT NULL
          AND btrim(barcode) <> ''
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(location_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        AppError::conflict(
            "relocation destination must be active, scannable, and in the source facility",
        )
    })
}

async fn require_no_active_loose_movement(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    balance_id: i64,
) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM putaway_tasks
            WHERE tenant_id = $1
              AND source_inventory_balance_id = $2
              AND closed_at IS NULL
            UNION ALL
            SELECT 1 FROM inventory_relocation_tasks
            WHERE tenant_id = $1
              AND source_inventory_balance_id = $2
              AND closed_at IS NULL
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(balance_id)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Err(AppError::conflict(
            "source inventory already has active movement work",
        ))
    } else {
        Ok(())
    }
}

pub(super) async fn require_plate_destination_compatible(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    destination_location_id: i64,
    contents: &[PlateContent],
) -> AppResult<()> {
    for content in contents {
        inventory::ensure_location_accepts_batch_tx(
            tx,
            tenant_id,
            inventory_owner_id,
            destination_location_id,
            content.item_batch_id,
        )
        .await?;
    }
    Ok(())
}
