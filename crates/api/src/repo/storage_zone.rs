//! Versioned facility storage-zone configuration and scoped read model.

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::storage_zone::{
    ConfigureStorageZoneCommand, ConfigureStorageZoneResult, RetireStorageZoneCommand,
    RetireStorageZoneResult, StorageZoneCursor, StorageZoneLocationReadModel, StorageZonePage,
    StorageZonePageQuery, StorageZoneReadModel, CONFIGURE_STORAGE_ZONE_OPERATION,
    RETIRE_STORAGE_ZONE_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, LocationId, StorageZoneCode, StorageZoneDefinition, StorageZoneId,
    StorageZoneLocationIds, StorageZoneName, StorageZonePurpose, StorageZoneRevision,
    StorageZoneStatus, StorageZoneTravelSequence, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::next_outbox_sequence_tx;

const SUPERVISOR_PERMISSION: &str = "wms_supervisor";
const READ_PERMISSION: &str = "wms";

#[derive(Debug)]
struct ZoneHeader {
    id: StorageZoneId,
    tenant_id: TenantId,
    facility_id: FacilityId,
    facility_name: String,
    code: StorageZoneCode,
    name: StorageZoneName,
    purpose: StorageZonePurpose,
    travel_sequence: StorageZoneTravelSequence,
    revision: StorageZoneRevision,
    status: StorageZoneStatus,
    configured_by: UserId,
    configured_at: Timestamp,
    retired_by: Option<UserId>,
    retired_at: Option<Timestamp>,
}

fn parse_purpose(value: &str) -> AppResult<StorageZonePurpose> {
    StorageZonePurpose::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid storage zone purpose: {value}")))
}

fn zone_header(row: &sqlx::postgres::PgRow) -> AppResult<ZoneHeader> {
    let retired_at = row.try_get::<Option<Timestamp>, _>("effective_to")?;
    Ok(ZoneHeader {
        id: StorageZoneId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_name: row.try_get("facility_name")?,
        code: StorageZoneCode::new(row.try_get::<String, _>("code")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        name: StorageZoneName::new(row.try_get::<String, _>("name")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        purpose: parse_purpose(&row.try_get::<String, _>("purpose")?)?,
        travel_sequence: StorageZoneTravelSequence::new(
            u32::try_from(row.try_get::<i64, _>("travel_sequence")?)
                .map_err(|_| AppError::internal("invalid storage zone travel sequence"))?,
        ),
        revision: StorageZoneRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status: if retired_at.is_some() {
            StorageZoneStatus::Retired
        } else {
            StorageZoneStatus::Active
        },
        configured_by: UserId::new(row.try_get("configured_by_user_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        configured_at: row.try_get("configured_at")?,
        retired_by: row
            .try_get::<Option<i64>, _>("retired_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        retired_at,
    })
}

fn build_zone(
    header: ZoneHeader,
    locations: Vec<StorageZoneLocationReadModel>,
) -> AppResult<StorageZoneReadModel> {
    let location_ids = StorageZoneLocationIds::new(
        locations
            .iter()
            .map(|location| location.location_id)
            .collect(),
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(StorageZoneReadModel {
        storage_zone_id: header.id,
        facility_name: header.facility_name,
        definition: StorageZoneDefinition {
            tenant_id: header.tenant_id,
            facility_id: header.facility_id,
            code: header.code,
            name: header.name,
            purpose: header.purpose,
            travel_sequence: header.travel_sequence,
            location_ids,
        },
        status: header.status,
        revision: header.revision,
        locations,
        configured_by: header.configured_by,
        configured_at: header.configured_at,
        retired_by: header.retired_by,
        retired_at: header.retired_at,
    })
}

fn require_facility_scope(scope: &ScopeBindings, facility_id: i64) -> AppResult<()> {
    if scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::not_found("storage zone"))
    }
}

async fn require_stored_zone_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT zone.facility_id
        FROM command_idempotency_records command
        JOIN storage_zones zone
          ON zone.tenant_id=command.tenant_id
         AND zone.id=(command.result_json->>'storage_zone_id')::BIGINT
        WHERE command.tenant_id=$1 AND command.operation=$2 AND command.idempotency_key=$3
        "#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = row {
        require_facility_scope(scope, row.try_get("facility_id")?)?;
    }
    Ok(())
}

async fn lock_natural_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    code: &StorageZoneCode,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "storage_zone:{}:{}:{}",
            tenant_id.get(),
            facility_id.get(),
            code.as_str()
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn lock_locations_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    location_ids: &[LocationId],
) -> AppResult<()> {
    let ids = location_ids.iter().map(|id| id.get()).collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT id FROM locations
        WHERE tenant_id=$1 AND facility_id=$2 AND id=ANY($3)
        ORDER BY id FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != ids.len() {
        return Err(AppError::not_found("storage zone location"));
    }
    Ok(())
}

async fn validate_new_locations_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &StorageZoneDefinition,
) -> AppResult<()> {
    let ids = definition
        .location_ids
        .as_slice()
        .iter()
        .map(|id| id.get())
        .collect::<Vec<_>>();
    let eligible_count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*) FROM locations
        WHERE tenant_id=$1 AND facility_id=$2 AND id=ANY($3)
          AND deleted IS NULL AND active AND NULLIF(btrim(barcode), '') IS NOT NULL
          AND CASE $4
              WHEN 'receiving' THEN receivable AND NOT pickable
              WHEN 'pick' THEN pickable AND NOT receivable
              ELSE NOT pickable AND NOT receivable
          END
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.facility_id.get())
    .bind(&ids)
    .bind(definition.purpose.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if eligible_count != i64::try_from(ids.len()).unwrap_or(i64::MAX) {
        return Err(AppError::conflict(
            "one or more locations are inactive, unscannable, or incompatible with the zone purpose",
        ));
    }
    let conflict = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT member.location_id
        FROM storage_zone_locations member
        JOIN storage_zones zone
          ON zone.tenant_id=member.tenant_id AND zone.facility_id=member.facility_id
         AND zone.id=member.storage_zone_id AND zone.effective_to IS NULL
        WHERE member.tenant_id=$1 AND member.facility_id=$2
          AND member.location_id=ANY($3) AND zone.code <> $4
        LIMIT 1
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.facility_id.get())
    .bind(&ids)
    .bind(definition.code.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    if conflict.is_some() {
        return Err(AppError::conflict(
            "one or more locations already belong to another active storage zone",
        ));
    }
    Ok(())
}

async fn latest_zone_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    code: &StorageZoneCode,
) -> AppResult<Option<(StorageZoneId, StorageZoneRevision, bool)>> {
    let row = sqlx::query(
        r#"
        SELECT id, revision, effective_to IS NULL AS active
        FROM storage_zones
        WHERE tenant_id=$1 AND facility_id=$2 AND code=$3
        ORDER BY revision DESC LIMIT 1 FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(code.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok((
            StorageZoneId::new(row.try_get("id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            StorageZoneRevision::new(row.try_get("revision")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            row.try_get("active")?,
        ))
    })
    .transpose()
}

async fn member_ids_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    zone_id: StorageZoneId,
) -> AppResult<Vec<LocationId>> {
    sqlx::query_scalar::<_, i64>(
        "SELECT location_id FROM storage_zone_locations WHERE tenant_id=$1 AND storage_zone_id=$2 ORDER BY location_id",
    )
    .bind(tenant_id.get())
    .bind(zone_id.get())
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|id| LocationId::new(id).map_err(|error| AppError::internal(error.to_string())))
    .collect()
}

async fn retire_row_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    zone_id: StorageZoneId,
    actor_id: i64,
    retired_at: Timestamp,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE storage_zones SET effective_to=$3, retired_by_user_id=$4 WHERE tenant_id=$1 AND id=$2 AND effective_to IS NULL",
    )
    .bind(tenant_id.get())
    .bind(zone_id.get())
    .bind(retired_at)
    .bind(actor_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_zone_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &ConfigureStorageZoneCommand,
    revision: StorageZoneRevision,
    predecessor: Option<StorageZoneId>,
    actor_id: i64,
    configured_at: Timestamp,
) -> AppResult<StorageZoneId> {
    let definition = &command.definition;
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO storage_zones
            (tenant_id,facility_id,code,name,purpose,travel_sequence,revision,
             supersedes_storage_zone_id,location_count,effective_from,configured_by_user_id,configured_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$10)
        RETURNING id
        "#,
    )
    .bind(definition.tenant_id.get())
    .bind(definition.facility_id.get())
    .bind(definition.code.as_str())
    .bind(definition.name.as_str())
    .bind(definition.purpose.as_str())
    .bind(i64::from(definition.travel_sequence.get()))
    .bind(revision.get())
    .bind(predecessor.map(StorageZoneId::get))
    .bind(i64::try_from(definition.location_ids.as_slice().len()).map_err(|_| {
        AppError::bad_request("storage zone contains too many locations")
    })?)
    .bind(configured_at)
    .bind(actor_id)
    .fetch_one(&mut **tx)
    .await?;
    StorageZoneId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn insert_members_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition: &StorageZoneDefinition,
    zone_id: StorageZoneId,
) -> AppResult<()> {
    for (index, location_id) in definition.location_ids.as_slice().iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO storage_zone_locations
                (tenant_id,facility_id,storage_zone_id,location_id,location_sequence)
            VALUES ($1,$2,$3,$4,$5)
            "#,
        )
        .bind(definition.tenant_id.get())
        .bind(definition.facility_id.get())
        .bind(zone_id.get())
        .bind(location_id.get())
        .bind(
            i64::try_from(index + 1)
                .map_err(|_| AppError::bad_request("storage zone contains too many locations"))?,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn locations_for_zones_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    zone_ids: &[i64],
) -> AppResult<HashMap<i64, Vec<StorageZoneLocationReadModel>>> {
    if zone_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT member.storage_zone_id, location.id, location.barcode, location.name,
               location.type, location.pickable, location.receivable
        FROM storage_zone_locations member
        JOIN locations location
          ON location.tenant_id=member.tenant_id AND location.facility_id=member.facility_id
         AND location.id=member.location_id
        WHERE member.tenant_id=$1 AND member.storage_zone_id=ANY($2)
        ORDER BY member.storage_zone_id, member.location_sequence
        "#,
    )
    .bind(tenant_id.get())
    .bind(zone_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut grouped = HashMap::<i64, Vec<StorageZoneLocationReadModel>>::new();
    for row in rows {
        grouped
            .entry(row.try_get("storage_zone_id")?)
            .or_default()
            .push(StorageZoneLocationReadModel {
                location_id: LocationId::new(row.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                barcode: row.try_get("barcode")?,
                name: row.try_get("name")?,
                location_type: row.try_get("type")?,
                pickable: row.try_get("pickable")?,
                receivable: row.try_get("receivable")?,
            });
    }
    Ok(grouped)
}

async fn read_zone_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    zone_id: StorageZoneId,
) -> AppResult<StorageZoneReadModel> {
    let row = sqlx::query(
        r#"
        SELECT zone.*, facility.name AS facility_name
        FROM storage_zones zone
        JOIN facilities facility ON facility.tenant_id=zone.tenant_id AND facility.id=zone.facility_id
        WHERE zone.tenant_id=$1 AND zone.id=$2
        "#,
    )
    .bind(tenant_id.get())
    .bind(zone_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("storage zone"))?;
    let mut grouped = locations_for_zones_tx(tx, tenant_id, &[zone_id.get()]).await?;
    build_zone(
        zone_header(&row)?,
        grouped.remove(&zone_id.get()).unwrap_or_default(),
    )
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_zone_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    actor_id: i64,
    zone_id: StorageZoneId,
    transition: &str,
    occurred_at: Timestamp,
    payload: &serde_json::Value,
) -> AppResult<()> {
    let event_key = format!("storage-zone:{}:{}", zone_id.get(), transition);
    let aggregate_id = zone_id.get().to_string();
    let ordering_key = format!("storage-zone:{}", zone_id.get());
    let event_type = format!("topology.storage_zone.{transition}");
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: None,
            facility_id: Some(facility_id),
            actor_user_id: Some(actor_id),
            event_key: &event_key,
            aggregate_type: "storage_zone",
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

pub async fn configure_storage_zone(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureStorageZoneCommand,
) -> AppResult<ConfigureStorageZoneResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.definition.tenant_id != access.tenant_id {
        return Err(AppError::not_found("storage zone"));
    }
    let prepared = PreparedCommand::new_v1(context, CONFIGURE_STORAGE_ZONE_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    require_stored_zone_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    require_facility_scope(&scope, command.definition.facility_id.get())?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }

    lock_natural_key_tx(
        &mut tx,
        access.tenant_id,
        command.definition.facility_id,
        &command.definition.code,
    )
    .await?;
    let predecessor = latest_zone_tx(
        &mut tx,
        access.tenant_id,
        command.definition.facility_id,
        &command.definition.code,
    )
    .await?;
    match (predecessor, command.expected_revision) {
        (None, None) | (Some((_, _, false)), None) => {}
        (Some((_, revision, true)), Some(expected)) if revision == expected => {}
        (Some((_, _, true)), None) => {
            return Err(AppError::conflict("storage zone already exists"));
        }
        (None, Some(_)) | (Some((_, _, false)), Some(_)) => {
            return Err(AppError::conflict("storage zone has no active revision"));
        }
        (Some((_, _, true)), Some(_)) => {
            return Err(AppError::conflict(
                "storage zone revision does not match expected revision",
            ));
        }
    }

    let mut locked_location_ids = command.definition.location_ids.as_slice().to_vec();
    if let Some((predecessor_id, _, true)) = predecessor {
        locked_location_ids.extend(member_ids_tx(&mut tx, access.tenant_id, predecessor_id).await?);
    }
    locked_location_ids.sort_unstable_by_key(|id| id.get());
    locked_location_ids.dedup();
    lock_locations_tx(
        &mut tx,
        access.tenant_id,
        command.definition.facility_id,
        &locked_location_ids,
    )
    .await?;
    validate_new_locations_tx(&mut tx, &command.definition).await?;

    let configured_at = now_iso();
    if let Some((predecessor_id, _, true)) = predecessor {
        retire_row_tx(
            &mut tx,
            access.tenant_id,
            predecessor_id,
            context.actor_id.get(),
            configured_at,
        )
        .await?;
    }
    let revision = match predecessor {
        Some((_, revision, _)) => revision
            .checked_next()
            .ok_or_else(|| AppError::internal("storage zone revision overflow"))?,
        None => {
            StorageZoneRevision::new(1).map_err(|error| AppError::internal(error.to_string()))?
        }
    };
    let zone_id = insert_zone_tx(
        &mut tx,
        command,
        revision,
        predecessor.map(|(id, _, _)| id),
        context.actor_id.get(),
        configured_at,
    )
    .await?;
    insert_members_tx(&mut tx, &command.definition, zone_id).await?;
    let result = read_zone_tx(&mut tx, access.tenant_id, zone_id).await?;
    enqueue_zone_event_tx(
        &mut tx,
        access.tenant_id,
        command.definition.facility_id,
        context.actor_id.get(),
        zone_id,
        "configured",
        configured_at,
        &serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn retire_storage_zone(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RetireStorageZoneCommand,
) -> AppResult<RetireStorageZoneResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, RETIRE_STORAGE_ZONE_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    require_stored_zone_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }

    let hint =
        sqlx::query("SELECT facility_id,code FROM storage_zones WHERE tenant_id=$1 AND id=$2")
            .bind(access.tenant_id.get())
            .bind(command.storage_zone_id.get())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::not_found("storage zone"))?;
    let facility_id = FacilityId::new(hint.try_get("facility_id")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let code = StorageZoneCode::new(hint.try_get::<String, _>("code")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    require_facility_scope(&scope, facility_id.get())?;
    lock_natural_key_tx(&mut tx, access.tenant_id, facility_id, &code).await?;
    let row = sqlx::query(
        "SELECT revision,effective_to FROM storage_zones WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(access.tenant_id.get())
    .bind(command.storage_zone_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("storage zone"))?;
    if row
        .try_get::<Option<Timestamp>, _>("effective_to")?
        .is_some()
    {
        return Err(AppError::conflict("storage zone is already retired"));
    }
    let revision = StorageZoneRevision::new(row.try_get("revision")?)
        .map_err(|error| AppError::internal(error.to_string()))?;
    if revision != command.expected_revision {
        return Err(AppError::conflict(
            "storage zone revision does not match expected revision",
        ));
    }
    let location_ids = member_ids_tx(&mut tx, access.tenant_id, command.storage_zone_id).await?;
    lock_locations_tx(&mut tx, access.tenant_id, facility_id, &location_ids).await?;
    let retired_at = now_iso();
    retire_row_tx(
        &mut tx,
        access.tenant_id,
        command.storage_zone_id,
        context.actor_id.get(),
        retired_at,
    )
    .await?;
    let result = read_zone_tx(&mut tx, access.tenant_id, command.storage_zone_id).await?;
    enqueue_zone_event_tx(
        &mut tx,
        access.tenant_id,
        facility_id,
        context.actor_id.get(),
        command.storage_zone_id,
        "retired",
        retired_at,
        &serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn storage_zone_page(
    db: &Db,
    access: &TenantAccess,
    query: StorageZonePageQuery,
) -> AppResult<StorageZonePage> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        READ_PERMISSION,
    )
    .await?;
    if let Some(facility_id) = query.facility_id {
        require_facility_scope(&scope, facility_id.get())?;
    }
    let facility_ids = &scope.facility_ids;
    let cursor_sequence = query
        .cursor
        .map(|cursor| i64::from(cursor.after_travel_sequence.get()));
    let cursor_id = query
        .cursor
        .map(|cursor| cursor.after_storage_zone_id.get());
    let purpose = query.purpose.map(StorageZonePurpose::as_str);
    let status = query.status.map(|status| match status {
        StorageZoneStatus::Active => "active",
        StorageZoneStatus::Retired => "retired",
    });
    let rows = sqlx::query(
        r#"
        SELECT zone.*, facility.name AS facility_name
        FROM storage_zones zone
        JOIN facilities facility ON facility.tenant_id=zone.tenant_id AND facility.id=zone.facility_id
        WHERE zone.tenant_id=$1
          AND ($2 OR zone.facility_id=ANY($3))
          AND ($4::BIGINT IS NULL OR zone.facility_id=$4)
          AND ($5::TEXT IS NULL OR zone.purpose=$5)
          AND (
              ($6::TEXT IS NULL AND zone.effective_to IS NULL)
              OR ($6='active' AND zone.effective_to IS NULL)
              OR ($6='retired' AND zone.effective_to IS NOT NULL)
          )
          AND ($7::BIGINT IS NULL OR (zone.travel_sequence,zone.id)>($7,$8))
        ORDER BY zone.travel_sequence,zone.id
        LIMIT $9
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(facility_ids)
    .bind(query.facility_id.map(FacilityId::get))
    .bind(purpose)
    .bind(status)
    .bind(cursor_sequence)
    .bind(cursor_id)
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > usize::from(query.limit);
    let headers = rows
        .into_iter()
        .take(usize::from(query.limit))
        .map(|row| zone_header(&row))
        .collect::<AppResult<Vec<_>>>()?;
    let zone_ids = headers
        .iter()
        .map(|header| header.id.get())
        .collect::<Vec<_>>();
    let mut locations = locations_for_zones_tx(&mut tx, access.tenant_id, &zone_ids).await?;
    let mut items = Vec::with_capacity(headers.len());
    for header in headers {
        let id = header.id.get();
        items.push(build_zone(
            header,
            locations.remove(&id).unwrap_or_default(),
        )?);
    }
    let next_cursor = if has_more {
        items.last().map(|item| StorageZoneCursor {
            after_travel_sequence: item.definition.travel_sequence,
            after_storage_zone_id: item.storage_zone_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(StorageZonePage { items, next_cursor })
}
