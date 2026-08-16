use serde::{Deserialize, Serialize};
use sqlx::Row;
use wareboxes_application::{
    putaway::PutawayTaskCreation,
    putaway_policy::{PutawayPolicyExpectation, PutawayPolicyReadModel},
    CommandContext,
};
use wareboxes_core::models::{
    InventoryStatus, InventoryTransactionType, LicensePlatePutawayConfirmation, TenantAccess,
    Timestamp, WorkTaskType,
};
use wareboxes_domain::{FacilityId, InventoryOwnerId};

use crate::db::{bind_tenant_context, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, ScopeBindings};
use crate::repo::inventory_journal::{self, JournalCommand, JournalEntry};
use crate::repo::putaway_policy::{self, PutawayContent};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use super::license_plate_tree::{lock_root_tree_tx, require_no_active_tree_movement_tx};
use super::{
    insert_progress_tx, insert_task_tx, lock_current_task_scope_tx,
    require_replayed_task_visible_tx, task_permission, task_timeout_seconds, NewWorkTask,
    TaskDimensions,
};

const CREATE_OPERATION: &str = "task.create_license_plate_putaway.v2";
const CONFIRM_OPERATION: &str = "task.confirm_license_plate_putaway.v2";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmLicensePlatePutawayOutcome {
    pub confirmation: LicensePlatePutawayConfirmation,
    pub putaway_policy: PutawayPolicyReadModel,
}

struct PutawayTarget {
    inventory_owner_id: i64,
    facility_id: i64,
    license_plate_id: i64,
    source_location_id: i64,
    destination_location_id: i64,
    planned_balance_count: i64,
    putaway_policy: PutawayPolicyReadModel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ContentLine {
    inventory_balance_id: i64,
    license_plate_id: i64,
    location_id: i64,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    status: InventoryStatus,
    quantity: i64,
    qty_reserved: i64,
    qty_held: i64,
    lot: Option<String>,
    expiration: Option<Timestamp>,
}

#[derive(Debug, PartialEq, Eq)]
struct PlannedContentLine {
    inventory_balance_id: i64,
    license_plate_id: i64,
    item_batch_id: i64,
    item_id: i64,
    uom: String,
    status: InventoryStatus,
    quantity: i64,
}

fn parse_inventory_status(value: &str) -> AppResult<InventoryStatus> {
    InventoryStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid inventory status in database: {value}")))
}

fn validate_creation(
    license_plate_id: i64,
    destination_location_id: i64,
    priority: i64,
    instructions: Option<&str>,
) -> AppResult<()> {
    if license_plate_id <= 0 {
        return Err(AppError::bad_request("license plate ID must be positive"));
    }
    if destination_location_id <= 0 {
        return Err(AppError::bad_request(
            "destination location ID must be positive",
        ));
    }
    if priority < 0 {
        return Err(AppError::bad_request(
            "license plate putaway priority cannot be negative",
        ));
    }
    if let Some(instructions) = instructions {
        if instructions.trim() != instructions || instructions.is_empty() {
            return Err(AppError::bad_request(
                "license plate putaway instructions must be trimmed and nonempty",
            ));
        }
        if instructions.chars().count() > 1_000 {
            return Err(AppError::bad_request(
                "license plate putaway instructions cannot exceed 1000 characters",
            ));
        }
    }
    Ok(())
}

fn validate_scanned_barcode(value: &str, label: &str) -> AppResult<()> {
    if value.trim() != value || value.is_empty() {
        return Err(AppError::bad_request(format!(
            "{label} must be trimmed and nonempty"
        )));
    }
    if value.chars().count() > 200 {
        return Err(AppError::bad_request(format!(
            "{label} cannot exceed 200 characters"
        )));
    }
    Ok(())
}

async fn lock_locations(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    facility_id: i64,
    source_location_id: i64,
    destination_location_id: i64,
) -> AppResult<String> {
    if source_location_id == destination_location_id {
        return Err(AppError::bad_request(
            "license plate putaway source and destination locations must differ",
        ));
    }
    let rows = sqlx::query(
        r#"
        SELECT id, barcode, receivable
        FROM locations
        WHERE tenant_id = $1
          AND facility_id = $2
          AND id = ANY($3)
          AND deleted IS NULL
          AND active
        ORDER BY id
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(vec![source_location_id, destination_location_id])
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != 2 {
        return Err(AppError::conflict(
            "license plate putaway locations must be active in the source facility",
        ));
    }

    let source = rows
        .iter()
        .find(|row| row.get::<i64, _>("id") == source_location_id)
        .ok_or_else(|| AppError::conflict("license plate source location is not active"))?;
    if !source.try_get::<bool, _>("receivable")? {
        return Err(AppError::conflict(
            "license plate putaway must start in a receiving location",
        ));
    }
    let destination = rows
        .iter()
        .find(|row| row.get::<i64, _>("id") == destination_location_id)
        .ok_or_else(|| AppError::conflict("license plate destination location is not active"))?;
    if destination.try_get::<bool, _>("receivable")? {
        return Err(AppError::conflict(
            "license plate putaway destination must be a storage location",
        ));
    }
    destination
        .try_get::<Option<String>, _>("barcode")?
        .filter(|barcode| !barcode.trim().is_empty())
        .ok_or_else(|| {
            AppError::conflict("license plate putaway destination must have a scannable barcode")
        })
}

async fn lock_contents(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    license_plate_ids: &[i64],
) -> AppResult<Vec<ContentLine>> {
    let rows = sqlx::query(
        r#"
        SELECT balance.id,
               balance.license_plate_id,
               balance.location_id,
               balance.item_batch_id,
               balance.item_id,
               balance.uom,
               balance.status,
               balance.qty_on_hand,
               balance.qty_reserved,
               balance.qty_held,
               batch.lot,
               batch.expiration
        FROM inventory_balances balance
        INNER JOIN item_batches batch
          ON batch.tenant_id = balance.tenant_id
         AND batch.inventory_owner_id = balance.inventory_owner_id
         AND batch.id = balance.item_batch_id
        WHERE balance.tenant_id = $1
          AND balance.inventory_owner_id = $2
          AND balance.facility_id = $3
          AND balance.license_plate_id = ANY($4)
          AND balance.deleted IS NULL
          AND batch.deleted IS NULL
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
            Ok(ContentLine {
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
                lot: row.try_get("lot")?,
                expiration: row.try_get("expiration")?,
            })
        })
        .collect()
}

fn positive_contents_at_source(
    contents: &[ContentLine],
    source_location_id: i64,
    allow_mixed_lots: bool,
) -> AppResult<Vec<ContentLine>> {
    if contents
        .iter()
        .any(|content| content.location_id != source_location_id)
    {
        return Err(AppError::conflict(
            "license plate inventory is split across locations",
        ));
    }
    let positive = contents
        .iter()
        .filter(|content| content.quantity > 0)
        .cloned()
        .collect::<Vec<_>>();
    if positive.is_empty() {
        return Err(AppError::conflict(
            "license plate does not contain inventory",
        ));
    }
    for content in &positive {
        if content.status != InventoryStatus::Available {
            return Err(AppError::conflict(
                "license plate putaway requires available inventory",
            ));
        }
        if content.qty_reserved > 0 || content.qty_held > 0 {
            return Err(AppError::conflict(
                "license plate contains reserved or held inventory",
            ));
        }
    }

    if !allow_mixed_lots {
        let mut item_lot_expiration = std::collections::BTreeMap::new();
        for content in &positive {
            let dimensions = (content.lot.clone(), content.expiration);
            if item_lot_expiration
                .insert(content.item_id, dimensions.clone())
                .is_some_and(|existing| existing != dimensions)
            {
                return Err(AppError::conflict(
                    "putaway policy prohibits the same item with multiple lots or expirations",
                ));
            }
        }
    }
    Ok(positive)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_license_plate_putaway_task_with_policy_in_scope(
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
    expected_policy: &PutawayPolicyExpectation,
) -> AppResult<PutawayTaskCreation> {
    command.require_actor(access.tenant_id, access.user_id)?;
    validate_creation(
        license_plate_id,
        destination_location_id,
        priority,
        instructions,
    )?;
    let prepared = PreparedCommand::new_v1(
        command,
        CREATE_OPERATION,
        &(
            license_plate_id,
            destination_location_id,
            priority,
            assigned_user_id,
            scheduled_for,
            due_at,
            instructions,
            expected_policy,
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

    if let Some(result) = prepared.replayed::<PutawayTaskCreation>(&mut tx).await? {
        require_replayed_task_visible_tx(&mut tx, access.tenant_id, result.task_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let plate = lock_root_tree_tx(&mut tx, access.tenant_id, license_plate_id).await?;
    let dimensions = TaskDimensions {
        facility_id: Some(plate.facility_id),
        inventory_owner_id: Some(plate.inventory_owner_id),
    };
    if !dimensions.is_allowed_by(&scope) {
        return Err(AppError::not_found("license plate"));
    }
    lock_locations(
        &mut tx,
        access.tenant_id,
        plate.facility_id,
        plate.location_id,
        destination_location_id,
    )
    .await?;
    let owner_facility =
        inventory_journal::owner_facility_scope(plate.inventory_owner_id, plate.facility_id)?;
    inventory_journal::lock_active_owner_facility_tx(&mut tx, access.tenant_id, owner_facility)
        .await?;
    let owner_id = InventoryOwnerId::new(plate.inventory_owner_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let facility_id = FacilityId::new(plate.facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let putaway_policy = putaway_policy::resolve_putaway_policy_tx(
        &mut tx,
        access.tenant_id,
        owner_id,
        facility_id,
        now_iso(),
        true,
    )
    .await?;
    putaway_policy::require_expected_policy(&putaway_policy, expected_policy)?;
    let contents = lock_contents(
        &mut tx,
        access.tenant_id,
        plate.inventory_owner_id,
        plate.facility_id,
        &plate.plate_ids,
    )
    .await?;
    require_no_active_tree_movement_tx(
        &mut tx,
        access.tenant_id,
        plate.inventory_owner_id,
        &plate.plate_ids,
    )
    .await?;
    let positive_contents = positive_contents_at_source(
        &contents,
        plate.location_id,
        putaway_policy.allow_mixed_lots,
    )?;
    putaway_policy::validate_destination_tx(
        &mut tx,
        access.tenant_id,
        owner_id,
        facility_id,
        destination_location_id,
        &positive_contents
            .iter()
            .map(|content| PutawayContent {
                item_id: content.item_id,
                item_batch_id: content.item_batch_id,
                uom: content.uom.clone(),
                quantity: content.quantity,
            })
            .collect::<Vec<_>>(),
        &putaway_policy,
    )
    .await?;

    let existing_task: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT task_id
        FROM license_plate_putaway_tasks
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND license_plate_id = $3
          AND closed_at IS NULL
        LIMIT 1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(plate.inventory_owner_id)
    .bind(license_plate_id)
    .fetch_optional(&mut *tx)
    .await?;
    if existing_task.is_some() {
        return Err(AppError::conflict(
            "license plate already has active putaway work",
        ));
    }

    let planned_balance_count = i64::try_from(positive_contents.len())
        .map_err(|_| AppError::internal("license plate content count is out of range"))?;
    let task_id = insert_task_tx(
        &mut tx,
        access.tenant_id,
        NewWorkTask {
            facility_id: Some(plate.facility_id),
            inventory_owner_id: Some(plate.inventory_owner_id),
            task_type: WorkTaskType::LicensePlatePutaway,
            title: "Put away received license plate".to_owned(),
            instructions: instructions.map(str::to_owned),
            required_permission: task_permission(WorkTaskType::LicensePlatePutaway).to_owned(),
            priority,
            task_timeout_seconds: task_timeout_seconds(WorkTaskType::LicensePlatePutaway),
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
        INSERT INTO license_plate_putaway_tasks (
            tenant_id,
            task_id,
            inventory_owner_id,
            facility_id,
            license_plate_id,
            source_location_id,
            destination_location_id,
            planned_balance_count,
            putaway_policy_source,
            putaway_policy_configuration_id,
            putaway_policy_configuration_revision,
            putaway_policy_scope_level,
            putaway_policy_scope_owner_id,
            putaway_policy_scope_facility_id,
            putaway_policy_definition,
            putaway_policy_hash
        )
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
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
    .bind(putaway_policy::source_text(putaway_policy.source))
    .bind(putaway_policy.configuration_id.map(|id| id.get()))
    .bind(putaway_policy.configuration_revision)
    .bind(putaway_policy::scope_values(putaway_policy.configuration_scope).0)
    .bind(putaway_policy::scope_values(putaway_policy.configuration_scope).1)
    .bind(putaway_policy::scope_values(putaway_policy.configuration_scope).2)
    .bind(putaway_policy::definition_json(&putaway_policy))
    .bind(&putaway_policy.policy_hash)
    .execute(&mut *tx)
    .await?;
    for content in &positive_contents {
        sqlx::query(
            r#"
            INSERT INTO license_plate_putaway_task_contents (
                tenant_id,
                task_id,
                inventory_owner_id,
                facility_id,
                license_plate_id,
                content_license_plate_id,
                inventory_balance_id,
                item_batch_id,
                item_id,
                uom,
                inventory_status,
                planned_quantity
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

    let result = PutawayTaskCreation {
        task_id,
        putaway_policy,
    };
    Ok(prepared.commit(tx, result).await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_license_plate_putaway_task_in_scope(
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
    let expected = PutawayPolicyReadModel::product_default().expectation();
    Ok(create_license_plate_putaway_task_with_policy_in_scope(
        db,
        access,
        command,
        license_plate_id,
        destination_location_id,
        priority,
        assigned_user_id,
        scheduled_for,
        due_at,
        instructions,
        &expected,
    )
    .await?
    .task_id)
}

async fn lock_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    task_id: i64,
    actor_user_id: i64,
    scope: &ScopeBindings,
) -> AppResult<PutawayTarget> {
    let row = sqlx::query(
        r#"
        SELECT task.status,
               task.assigned_user_id,
               task.lease_expires_at > statement_timestamp() AS lease_is_current,
               detail.inventory_owner_id,
               detail.facility_id,
               detail.license_plate_id,
               detail.source_location_id,
               detail.destination_location_id,
               detail.planned_balance_count,
               detail.putaway_policy_source,
               detail.putaway_policy_configuration_id,
               detail.putaway_policy_configuration_revision,
               detail.putaway_policy_scope_level,
               detail.putaway_policy_scope_owner_id,
               detail.putaway_policy_scope_facility_id,
               detail.putaway_policy_definition,
               detail.putaway_policy_hash,
               detail.closed_at
        FROM work_tasks task
        INNER JOIN license_plate_putaway_tasks detail
          ON detail.tenant_id = task.tenant_id
         AND detail.task_id = task.id
        WHERE task.tenant_id = $1
          AND task.id = $2
          AND task.deleted IS NULL
          AND task.task_type = 'license_plate_putaway'
        FOR UPDATE OF task
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("license plate putaway task"))?;
    let putaway_policy = putaway_policy::frozen_policy(&row)?;
    let target = PutawayTarget {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        license_plate_id: row.try_get("license_plate_id")?,
        source_location_id: row.try_get("source_location_id")?,
        destination_location_id: row.try_get("destination_location_id")?,
        planned_balance_count: row.try_get("planned_balance_count")?,
        putaway_policy,
    };
    let dimensions = TaskDimensions {
        facility_id: Some(target.facility_id),
        inventory_owner_id: Some(target.inventory_owner_id),
    };
    if !dimensions.is_allowed_by(scope) {
        return Err(AppError::not_found("license plate putaway task"));
    }
    let status: String = row.try_get("status")?;
    let assigned_user_id: Option<i64> = row.try_get("assigned_user_id")?;
    let lease_is_current: Option<bool> = row.try_get("lease_is_current")?;
    let closed_at: Option<Timestamp> = row.try_get("closed_at")?;
    if status != "in_progress"
        || assigned_user_id != Some(actor_user_id)
        || lease_is_current != Some(true)
        || closed_at.is_some()
    {
        return Err(AppError::conflict(
            "license plate putaway task does not have an active claim for this operator",
        ));
    }
    Ok(target)
}

async fn planned_contents(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: wareboxes_domain::TenantId,
    task_id: i64,
) -> AppResult<Vec<PlannedContentLine>> {
    let rows = sqlx::query(
        r#"
        SELECT inventory_balance_id,
               content_license_plate_id,
               item_batch_id,
               item_id,
               uom,
               inventory_status,
               planned_quantity
        FROM license_plate_putaway_task_contents
        WHERE tenant_id = $1
          AND task_id = $2
        ORDER BY inventory_balance_id
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(PlannedContentLine {
                inventory_balance_id: row.try_get("inventory_balance_id")?,
                license_plate_id: row.try_get("content_license_plate_id")?,
                item_batch_id: row.try_get("item_batch_id")?,
                item_id: row.try_get("item_id")?,
                uom: row.try_get("uom")?,
                status: parse_inventory_status(&row.try_get::<String, _>("inventory_status")?)?,
                quantity: row.try_get("planned_quantity")?,
            })
        })
        .collect()
}

fn require_exact_snapshot(
    target: &PutawayTarget,
    current: &[ContentLine],
    planned: &[PlannedContentLine],
) -> AppResult<()> {
    if i64::try_from(planned.len()).ok() != Some(target.planned_balance_count)
        || current.len() != planned.len()
    {
        return Err(AppError::conflict(
            "license plate contents changed after putaway planning",
        ));
    }
    for (current, planned) in current.iter().zip(planned) {
        if current.inventory_balance_id != planned.inventory_balance_id
            || current.license_plate_id != planned.license_plate_id
            || current.item_batch_id != planned.item_batch_id
            || current.item_id != planned.item_id
            || current.uom != planned.uom
            || current.status != planned.status
            || current.quantity != planned.quantity
        {
            return Err(AppError::conflict(
                "license plate contents changed after putaway planning",
            ));
        }
    }
    Ok(())
}

pub async fn confirm_license_plate_putaway_with_policy_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    scanned_license_plate_barcode: &str,
    scanned_destination_location_barcode: &str,
    expected_policy: &PutawayPolicyExpectation,
) -> AppResult<ConfirmLicensePlatePutawayOutcome> {
    command.require_actor(access.tenant_id, access.user_id)?;
    if task_id <= 0 {
        return Err(AppError::bad_request(
            "license plate putaway task ID must be positive",
        ));
    }
    validate_scanned_barcode(scanned_license_plate_barcode, "license plate barcode")?;
    validate_scanned_barcode(
        scanned_destination_location_barcode,
        "destination location barcode",
    )?;
    let prepared = PreparedCommand::new_v1(
        command,
        CONFIRM_OPERATION,
        &(
            task_id,
            scanned_license_plate_barcode,
            scanned_destination_location_barcode,
            expected_policy,
        ),
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, command.actor_id.get()).await?;

    if let Some(result) = prepared
        .replayed::<ConfirmLicensePlatePutawayOutcome>(&mut tx)
        .await?
    {
        require_replayed_task_visible_tx(&mut tx, access.tenant_id, task_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }

    let target = lock_target(&mut tx, access, task_id, command.actor_id.get(), &scope).await?;
    putaway_policy::require_expected_policy(&target.putaway_policy, expected_policy)?;
    let plate = lock_root_tree_tx(&mut tx, access.tenant_id, target.license_plate_id).await?;
    if plate.inventory_owner_id != target.inventory_owner_id
        || plate.facility_id != target.facility_id
        || plate.location_id != target.source_location_id
    {
        return Err(AppError::conflict(
            "license plate no longer matches the putaway task",
        ));
    }
    if plate.barcode != scanned_license_plate_barcode {
        return Err(AppError::conflict(
            "scanned license plate does not match the putaway task",
        ));
    }
    let destination_barcode = lock_locations(
        &mut tx,
        access.tenant_id,
        target.facility_id,
        target.source_location_id,
        target.destination_location_id,
    )
    .await?;
    if destination_barcode != scanned_destination_location_barcode {
        return Err(AppError::conflict(
            "scanned destination does not match the directed putaway location",
        ));
    }
    let contents = lock_contents(
        &mut tx,
        access.tenant_id,
        target.inventory_owner_id,
        target.facility_id,
        &plate.plate_ids,
    )
    .await?;
    let positive_contents = positive_contents_at_source(
        &contents,
        target.source_location_id,
        target.putaway_policy.allow_mixed_lots,
    )?;
    let planned = planned_contents(&mut tx, access.tenant_id, task_id).await?;
    require_exact_snapshot(&target, &positive_contents, &planned)?;
    putaway_policy::validate_destination_tx(
        &mut tx,
        access.tenant_id,
        InventoryOwnerId::new(target.inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        FacilityId::new(target.facility_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        target.destination_location_id,
        &positive_contents
            .iter()
            .map(|content| PutawayContent {
                item_id: content.item_id,
                item_batch_id: content.item_batch_id,
                uom: content.uom.clone(),
                quantity: content.quantity,
            })
            .collect::<Vec<_>>(),
        &target.putaway_policy,
    )
    .await?;

    let owner_facility =
        inventory_journal::owner_facility_scope(target.inventory_owner_id, target.facility_id)?;
    let transaction_id = inventory_journal::begin_transaction(
        &mut tx,
        &JournalCommand {
            tenant_id: access.tenant_id,
            owner_facility,
            actor_user_id: command.actor_id.get(),
            transaction_type: InventoryTransactionType::Move,
            reason: Some("directed license plate putaway confirmation"),
            reference_type: Some("license_plate_putaway_task"),
            reference_id: Some(task_id),
            correlation_id: Some(&command.request_id),
            operation: CONFIRM_OPERATION,
            idempotency_key: Some(prepared.idempotency_key()),
            request_hash: prepared.request_hash(),
        },
    )
    .await?;

    let moved_at = now_iso();
    let balance_ids = contents
        .iter()
        .map(|content| content.inventory_balance_id)
        .collect::<Vec<_>>();
    let updated_balances = sqlx::query(
        r#"
        UPDATE inventory_balances
        SET location_id = $1,
            modified = $2
        WHERE tenant_id = $3
          AND inventory_owner_id = $4
          AND facility_id = $5
          AND license_plate_id = ANY($6)
          AND id = ANY($7)
          AND deleted IS NULL
        "#,
    )
    .bind(target.destination_location_id)
    .bind(moved_at)
    .bind(access.tenant_id.get())
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(&plate.plate_ids)
    .bind(&balance_ids)
    .execute(&mut *tx)
    .await?;
    if usize::try_from(updated_balances.rows_affected()).ok() != Some(balance_ids.len()) {
        return Err(AppError::conflict(
            "license plate inventory changed during putaway confirmation",
        ));
    }
    let updated_plate = sqlx::query(
        r#"
        UPDATE license_plates
        SET location_id = $1
        WHERE tenant_id = $2
          AND inventory_owner_id = $3
          AND facility_id = $4
          AND id = ANY($5)
          AND location_id = $6
          AND deleted IS NULL
        "#,
    )
    .bind(target.destination_location_id)
    .bind(access.tenant_id.get())
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(&plate.plate_ids)
    .bind(target.source_location_id)
    .execute(&mut *tx)
    .await?;
    if usize::try_from(updated_plate.rows_affected()).ok() != Some(plate.plate_ids.len()) {
        return Err(AppError::conflict(
            "license plate location changed during putaway confirmation",
        ));
    }

    for content in &positive_contents {
        for (location_id, quantity_delta) in [
            (target.source_location_id, -content.quantity),
            (target.destination_location_id, content.quantity),
        ] {
            inventory_journal::append_entry(
                &mut tx,
                access.tenant_id,
                owner_facility,
                transaction_id,
                &JournalEntry {
                    location_id,
                    license_plate_id: Some(content.license_plate_id),
                    item_batch_id: content.item_batch_id,
                    status: content.status,
                    quantity_delta,
                },
            )
            .await?;
        }
    }

    let confirmed_at = now_iso();
    let result = LicensePlatePutawayConfirmation {
        tenant_id: access.tenant_id,
        task_id,
        inventory_owner_id: InventoryOwnerId::new(target.inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: target.facility_id,
        license_plate_id: target.license_plate_id,
        license_plate_barcode: plate.barcode,
        source_location_id: target.source_location_id,
        destination_location_id: target.destination_location_id,
        destination_location_barcode: destination_barcode,
        inventory_transaction_id: transaction_id,
        moved_balance_count: target.planned_balance_count,
        confirmed_by: command.actor_id.get(),
        confirmed_at,
    };
    sqlx::query(
        r#"
        INSERT INTO license_plate_putaway_results (
            tenant_id,
            task_id,
            inventory_owner_id,
            facility_id,
            license_plate_id,
            license_plate_barcode,
            source_location_id,
            destination_location_id,
            destination_location_barcode,
            inventory_transaction_id,
            moved_balance_count,
            confirmed_by,
            confirmed_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(task_id)
    .bind(target.inventory_owner_id)
    .bind(target.facility_id)
    .bind(target.license_plate_id)
    .bind(&result.license_plate_barcode)
    .bind(target.source_location_id)
    .bind(target.destination_location_id)
    .bind(&result.destination_location_barcode)
    .bind(transaction_id)
    .bind(target.planned_balance_count)
    .bind(command.actor_id.get())
    .bind(confirmed_at)
    .execute(&mut *tx)
    .await?;

    let completed = sqlx::query(
        r#"
        UPDATE work_tasks
        SET status = 'completed',
            completed_by = $1,
            completed_at = $2,
            lease_expires_at = NULL,
            modified = $2
        WHERE tenant_id = $3
          AND id = $4
          AND deleted IS NULL
          AND status = 'in_progress'
          AND assigned_user_id = $1
          AND lease_expires_at > statement_timestamp()
        "#,
    )
    .bind(command.actor_id.get())
    .bind(confirmed_at)
    .bind(access.tenant_id.get())
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    if completed.rows_affected() != 1 {
        return Err(AppError::conflict(
            "license plate putaway task claim expired during confirmation",
        ));
    }
    insert_progress_tx(
        &mut tx,
        access.tenant_id,
        task_id,
        None,
        Some(command.actor_id.get()),
        "license_plate_putaway_confirmed",
        None,
        Some(target.source_location_id),
        Some(target.destination_location_id),
        None,
        None,
    )
    .await?;

    let inventory_owner_id = result.inventory_owner_id;
    let facility_id = FacilityId::new(target.facility_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let event_key = format!("license-plate-putaway-confirmation:{task_id}");
    let aggregate_id = task_id.to_string();
    let payload = serde_json::json!({
        "task_id": task_id,
        "inventory_transaction_id": transaction_id,
        "inventory_owner_id": target.inventory_owner_id,
        "facility_id": target.facility_id,
        "license_plate_id": target.license_plate_id,
        "source_location_id": target.source_location_id,
        "destination_location_id": target.destination_location_id,
        "moved_balance_count": target.planned_balance_count,
        "putaway_policy": &target.putaway_policy,
    });
    outbox::enqueue(
        &mut tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(command.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "license_plate_putaway_confirmation",
            aggregate_id: &aggregate_id,
            ordering_key: &event_key,
            aggregate_sequence: 1,
            event_type: "inventory.license_plate_putaway.confirmed",
            schema_version: 2,
            payload: &payload,
            occurred_at: confirmed_at,
        },
    )
    .await?;

    let outcome = ConfirmLicensePlatePutawayOutcome {
        confirmation: result,
        putaway_policy: target.putaway_policy,
    };
    Ok(prepared
        .commit_with_inventory_transaction(tx, outcome, Some(transaction_id))
        .await?)
}

pub async fn confirm_license_plate_putaway_in_scope(
    db: &Db,
    access: &TenantAccess,
    command: &CommandContext,
    task_id: i64,
    scanned_license_plate_barcode: &str,
    scanned_destination_location_barcode: &str,
) -> AppResult<LicensePlatePutawayConfirmation> {
    let expected = PutawayPolicyReadModel::product_default().expectation();
    Ok(confirm_license_plate_putaway_with_policy_in_scope(
        db,
        access,
        command,
        task_id,
        scanned_license_plate_barcode,
        scanned_destination_location_barcode,
        &expected,
    )
    .await?
    .confirmation)
}
