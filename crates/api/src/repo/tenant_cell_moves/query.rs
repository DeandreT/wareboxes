use sqlx::postgres::PgRow;
use sqlx::Row;
use wareboxes_application::tenant_cell_move::{
    TenantCellMoveAction, TenantCellMoveActionEligibility, TenantCellMoveBlocker,
    TenantCellMoveCheckpointReadModel, TenantCellMoveCursor,
    TenantCellMoveCutoverVerificationReadModel, TenantCellMoveDataCellSummary,
    TenantCellMoveEventAction, TenantCellMoveEventCursor, TenantCellMoveEventPage,
    TenantCellMoveEventPageQuery, TenantCellMoveEventReadModel, TenantCellMovePage,
    TenantCellMovePageQuery, TenantCellMoveReadModel, TenantCellMoveRollbackVerificationReadModel,
    TenantCellMoveTenantSummary, TenantCellMoveValidationReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    DataCellId, DataCellMode, DataCellPlacementRevision, DataCellRevision, DataCellStatus,
    PostgresLsn, Sha256Checksum, TenantCellMoveCheckpoint, TenantCellMoveCheckpointInput,
    TenantCellMoveCopyReference, TenantCellMoveCutoverVerification,
    TenantCellMoveCutoverVerificationInput, TenantCellMoveId, TenantCellMoveRevision,
    TenantCellMoveRollbackVerification, TenantCellMoveRollbackVerificationInput,
    TenantCellMoveRoutingReference, TenantCellMoveStatus, TenantCellMoveToolVersion,
    TenantCellMoveValidation, TenantCellMoveValidationInput, TenantId, TenantRevision,
    TenantStatus, UserId,
};

use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};

const MOVE_SELECT: &str = r#"
SELECT move.id AS tenant_cell_move_id,move.tenant_id,move.status,move.revision,
  move.source_placement_revision,move.cutover_placement_revision,
  move.rollback_placement_revision,move.residency_requirement,move.reason,
  move.copy_reference,move.requested_at,move.requested_by_user_id,
  move.copy_started_at,move.copy_started_by_user_id,move.frozen_at,
  move.frozen_by_user_id,move.validated_at,move.validated_by_user_id,
  move.cutover_at,move.cutover_by_user_id,move.post_cutover_verified_at,
  move.post_cutover_verified_by_user_id,move.completed_at,move.completed_by_user_id,
  move.rolled_back_at,move.rolled_back_by_user_id,move.cancelled_at,
  move.cancelled_by_user_id,move.change_reason,
  tenant.slug AS tenant_slug,tenant.name AS tenant_name,
  tenant.status AS tenant_status,tenant.revision AS tenant_revision,
  source.id AS source_data_cell_id,source.cell_key AS source_cell_key,
  source.name AS source_cell_name,source.region AS source_region,
  source.residency_code AS source_residency,source.mode AS source_mode,
  source.status AS source_status,source.revision AS source_revision,
  source.max_tenants AS source_max_tenants,
  (SELECT COUNT(*) FROM tenant_cell_placements placement
    WHERE placement.data_cell_id=source.id) AS source_placement_count,
  (SELECT COUNT(*) FROM tenant_cell_moves reservation
    WHERE reservation.target_data_cell_id=source.id
      AND reservation.status IN ('planned','copying','frozen','validated'))
    AS source_reserved_inbound_move_count,
  (SELECT COUNT(*) FROM tenant_cell_moves reservation
    WHERE reservation.source_data_cell_id=source.id
      AND reservation.status='cut_over') AS source_reserved_rollback_move_count,
  target.id AS target_data_cell_id,target.cell_key AS target_cell_key,
  target.name AS target_cell_name,target.region AS target_region,
  target.residency_code AS target_residency,target.mode AS target_mode,
  target.status AS target_status,target.revision AS target_revision,
  target.max_tenants AS target_max_tenants,
  (SELECT COUNT(*) FROM tenant_cell_placements placement
    WHERE placement.data_cell_id=target.id) AS target_placement_count,
  (SELECT COUNT(*) FROM tenant_cell_moves reservation
    WHERE reservation.target_data_cell_id=target.id
      AND reservation.status IN ('planned','copying','frozen','validated'))
    AS target_reserved_inbound_move_count,
  (SELECT COUNT(*) FROM tenant_cell_moves reservation
    WHERE reservation.source_data_cell_id=target.id
      AND reservation.status='cut_over') AS target_reserved_rollback_move_count,
  current_placement.data_cell_id AS current_data_cell_id,
  current_placement.revision AS current_placement_revision,
  checkpoint_event.move_revision AS checkpoint_move_revision,
  move.latest_source_wal_lsn::TEXT AS checkpoint_source_lsn,
  move.latest_target_replay_lsn::TEXT AS checkpoint_target_replay_lsn,
  move.copied_row_count AS checkpoint_copied_row_count,
  move.copied_bytes AS checkpoint_copied_bytes,
  move.checkpointed_at,move.checkpointed_by_user_id,
  validation.move_revision AS validation_move_revision,
  validation.tool_version AS validation_tool_version,
  validation.source_wal_lsn::TEXT AS validation_source_lsn,
  validation.target_replay_lsn::TEXT AS validation_target_replay_lsn,
  validation.source_row_count,validation.target_row_count,
  validation.source_data_checksum,validation.target_data_checksum,
  validation.source_schema_fingerprint,validation.target_schema_fingerprint,
  validation.source_object_manifest_checksum,validation.target_object_manifest_checksum,
  validation.inventory_reconciled AS validation_inventory_reconciled,
  validation.idempotency_verified AS validation_idempotency_verified,
  validation.outbox_verified AS validation_outbox_verified,
  validation.validated_at AS validation_evidence_at,
  validation.validated_by_user_id AS validation_evidence_by,
  cutover_verification.move_revision AS cutover_verification_move_revision,
  cutover_verification.tool_version AS cutover_verification_tool_version,
  cutover_verification.routing_reference AS cutover_verification_routing_reference,
  cutover_verification.observed_data_cell_id,
  cutover_verification.observed_placement_revision,
  cutover_verification.routing_verified,
  cutover_verification.target_read_verified,
  cutover_verification.write_fence_verified,
  cutover_verification.inventory_reconciled AS cutover_inventory_reconciled,
  cutover_verification.idempotency_verified AS cutover_idempotency_verified,
  cutover_verification.outbox_verified AS cutover_outbox_verified,
  cutover_verification.verified_at AS cutover_verification_at,
  cutover_verification.verified_by_user_id AS cutover_verification_by,
  rollback_verification.move_revision AS rollback_verification_move_revision,
  rollback_verification.tool_version AS rollback_verification_tool_version,
  rollback_verification.routing_reference AS rollback_verification_routing_reference,
  rollback_verification.observed_data_cell_id AS rollback_observed_data_cell_id,
  rollback_verification.expected_rollback_placement_revision,
  rollback_verification.routing_verified AS rollback_routing_verified,
  rollback_verification.source_read_verified,
  rollback_verification.write_fence_verified AS rollback_write_fence_verified,
  rollback_verification.inventory_reconciled AS rollback_inventory_reconciled,
  rollback_verification.idempotency_verified AS rollback_idempotency_verified,
  rollback_verification.outbox_verified AS rollback_outbox_verified,
  rollback_verification.verified_at AS rollback_verification_at,
  rollback_verification.verified_by_user_id AS rollback_verification_by,
  fence.fence_epoch AS write_fence_epoch,
  (fence.tenant_id IS NOT NULL) AS write_frozen
FROM tenant_cell_moves move
JOIN tenants tenant ON tenant.id=move.tenant_id
JOIN data_cells source ON source.id=move.source_data_cell_id
JOIN data_cells target ON target.id=move.target_data_cell_id
LEFT JOIN tenant_cell_placements current_placement
  ON current_placement.tenant_id=move.tenant_id
LEFT JOIN tenant_cell_move_validations validation
  ON validation.tenant_id=move.tenant_id
 AND validation.tenant_cell_move_id=move.id
LEFT JOIN tenant_cell_move_cutover_verifications cutover_verification
  ON cutover_verification.tenant_id=move.tenant_id
 AND cutover_verification.tenant_cell_move_id=move.id
LEFT JOIN tenant_cell_move_rollback_verifications rollback_verification
  ON rollback_verification.tenant_id=move.tenant_id
 AND rollback_verification.tenant_cell_move_id=move.id
LEFT JOIN tenant_write_fences fence
  ON fence.tenant_id=move.tenant_id AND fence.tenant_cell_move_id=move.id
LEFT JOIN LATERAL (
  SELECT event.move_revision FROM tenant_cell_move_events event
  WHERE event.tenant_cell_move_id=move.id AND event.action='checkpoint_recorded'
  ORDER BY event.move_revision DESC LIMIT 1
) checkpoint_event ON TRUE
"#;

#[derive(Clone, Copy)]
struct DataCellColumns {
    id: &'static str,
    key: &'static str,
    name: &'static str,
    region: &'static str,
    residency: &'static str,
    mode: &'static str,
    status: &'static str,
    revision: &'static str,
    max_tenants: &'static str,
    placement_count: &'static str,
    reserved_inbound_move_count: &'static str,
    reserved_rollback_move_count: &'static str,
}

const SOURCE_CELL_COLUMNS: DataCellColumns = DataCellColumns {
    id: "source_data_cell_id",
    key: "source_cell_key",
    name: "source_cell_name",
    region: "source_region",
    residency: "source_residency",
    mode: "source_mode",
    status: "source_status",
    revision: "source_revision",
    max_tenants: "source_max_tenants",
    placement_count: "source_placement_count",
    reserved_inbound_move_count: "source_reserved_inbound_move_count",
    reserved_rollback_move_count: "source_reserved_rollback_move_count",
};

const TARGET_CELL_COLUMNS: DataCellColumns = DataCellColumns {
    id: "target_data_cell_id",
    key: "target_cell_key",
    name: "target_cell_name",
    region: "target_region",
    residency: "target_residency",
    mode: "target_mode",
    status: "target_status",
    revision: "target_revision",
    max_tenants: "target_max_tenants",
    placement_count: "target_placement_count",
    reserved_inbound_move_count: "target_reserved_inbound_move_count",
    reserved_rollback_move_count: "target_reserved_rollback_move_count",
};

pub(super) async fn read_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_tenant_id: TenantId,
    tenant_cell_move_id: TenantCellMoveId,
) -> AppResult<TenantCellMoveReadModel> {
    let statement = format!("{MOVE_SELECT} WHERE move.id=$1");
    let row = sqlx::query(&statement)
        .bind(tenant_cell_move_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("tenant cell move"))?;
    map_move_row(&row, actor_tenant_id)
}

pub async fn by_id(
    db: &Db,
    actor_access: &TenantAccess,
    tenant_cell_move_id: TenantCellMoveId,
) -> AppResult<TenantCellMoveReadModel> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    crate::repo::tenant_lifecycle::authorize_tx(&mut tx, actor_access, actor_access.user_id)
        .await?;
    let result = read_tx(&mut tx, actor_access.tenant_id, tenant_cell_move_id).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn page(
    db: &Db,
    actor_access: &TenantAccess,
    query: &TenantCellMovePageQuery,
) -> AppResult<TenantCellMovePage> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    crate::repo::tenant_lifecycle::authorize_tx(&mut tx, actor_access, actor_access.user_id)
        .await?;
    let statement = format!(
        r#"{MOVE_SELECT}
        WHERE ($1::BIGINT IS NULL OR move.tenant_id=$1)
          AND ($2::BIGINT IS NULL
            OR move.source_data_cell_id=$2 OR move.target_data_cell_id=$2)
          AND ($3::TEXT IS NULL OR move.status=$3)
          AND ($4::TIMESTAMPTZ IS NULL OR (move.requested_at,move.id)<($4,$5))
        ORDER BY move.requested_at DESC,move.id DESC LIMIT $6"#
    );
    let rows = sqlx::query(&statement)
        .bind(query.tenant_id.map(TenantId::get))
        .bind(query.data_cell_id.map(DataCellId::get))
        .bind(query.status.map(TenantCellMoveStatus::as_str))
        .bind(query.cursor.map(|cursor| cursor.after_requested_at))
        .bind(
            query
                .cursor
                .map(|cursor| cursor.after_tenant_cell_move_id.get()),
        )
        .bind(i64::from(query.limit) + 1)
        .fetch_all(&mut *tx)
        .await?;
    let mut items = rows
        .iter()
        .map(|row| map_move_row(row, actor_access.tenant_id))
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if items.len() > usize::from(query.limit) {
        items.pop();
        items.last().map(|item| TenantCellMoveCursor {
            after_requested_at: item.requested_at,
            after_tenant_cell_move_id: item.tenant_cell_move_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(TenantCellMovePage { items, next_cursor })
}

pub async fn event_page(
    db: &Db,
    actor_access: &TenantAccess,
    query: &TenantCellMoveEventPageQuery,
) -> AppResult<TenantCellMoveEventPage> {
    let mut tx = begin_tenant_transaction(db, actor_access.tenant_id).await?;
    crate::repo::tenant_lifecycle::authorize_tx(&mut tx, actor_access, actor_access.user_id)
        .await?;
    read_tx(&mut tx, actor_access.tenant_id, query.tenant_cell_move_id).await?;
    let rows = sqlx::query(
        r#"SELECT event.id AS event_id,event.tenant_cell_move_id,event.tenant_id,
        event.action,event.move_revision,event.previous_status,event.resulting_status,
        move.source_placement_revision,move.cutover_placement_revision,
        move.rollback_placement_revision,event.actor_user_id,event.occurred_at,
        event.reason,event.request_id,event.evidence
        FROM tenant_cell_move_events event
        JOIN tenant_cell_moves move ON move.id=event.tenant_cell_move_id
        WHERE event.tenant_cell_move_id=$1
          AND ($2::TIMESTAMPTZ IS NULL OR (event.occurred_at,event.id)<($2,$3))
        ORDER BY event.occurred_at DESC,event.id DESC LIMIT $4"#,
    )
    .bind(query.tenant_cell_move_id.get())
    .bind(query.cursor.map(|cursor| cursor.after_occurred_at))
    .bind(query.cursor.map(|cursor| cursor.after_event_id))
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = rows
        .iter()
        .map(map_event_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if items.len() > usize::from(query.limit) {
        items.pop();
        items.last().map(|event| TenantCellMoveEventCursor {
            after_occurred_at: event.occurred_at,
            after_event_id: event.event_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(TenantCellMoveEventPage { items, next_cursor })
}

fn map_move_row(row: &PgRow, actor_tenant_id: TenantId) -> AppResult<TenantCellMoveReadModel> {
    let status = parse_move_status(row.try_get("status")?)?;
    let tenant_cell_move_id = move_id(row.try_get("tenant_cell_move_id")?)?;
    let tenant_id = tenant_id(row.try_get("tenant_id")?)?;
    let revision = move_revision(row.try_get("revision")?)?;
    let source_placement_revision = placement_revision(row.try_get("source_placement_revision")?)?;
    let cutover_placement_revision = row
        .try_get::<Option<i64>, _>("cutover_placement_revision")?
        .map(placement_revision)
        .transpose()?;
    let rollback_placement_revision = row
        .try_get::<Option<i64>, _>("rollback_placement_revision")?
        .map(placement_revision)
        .transpose()?;
    let change_reason: Option<String> = row.try_get("change_reason")?;
    let current_data_cell_id = row.try_get::<Option<i64>, _>("current_data_cell_id")?;
    let current_placement_revision = row.try_get("current_placement_revision")?;
    let write_fence_epoch = row.try_get("write_fence_epoch")?;

    let mut result = TenantCellMoveReadModel {
        tenant_cell_move_id,
        tenant: TenantCellMoveTenantSummary {
            tenant_id,
            slug: row.try_get("tenant_slug")?,
            name: row.try_get("tenant_name")?,
            status: parse_tenant_status(row.try_get("tenant_status")?)?,
            revision: TenantRevision::new(row.try_get("tenant_revision")?).map_err(stored_error)?,
        },
        source_cell: map_data_cell(row, SOURCE_CELL_COLUMNS)?,
        target_cell: map_data_cell(row, TARGET_CELL_COLUMNS)?,
        status,
        revision,
        source_placement_revision,
        cutover_placement_revision,
        rollback_placement_revision,
        residency_requirement: row.try_get("residency_requirement")?,
        reason: row.try_get("reason")?,
        copy_reference: row
            .try_get::<Option<String>, _>("copy_reference")?
            .map(TenantCellMoveCopyReference::new)
            .transpose()
            .map_err(stored_error)?,
        requested_at: row.try_get("requested_at")?,
        requested_by: user_id(row.try_get("requested_by_user_id")?)?,
        copy_started_at: row.try_get("copy_started_at")?,
        copy_started_by: optional_user(row, "copy_started_by_user_id")?,
        frozen_at: row.try_get("frozen_at")?,
        frozen_by: optional_user(row, "frozen_by_user_id")?,
        validated_at: row.try_get("validated_at")?,
        validated_by: optional_user(row, "validated_by_user_id")?,
        cutover_at: row.try_get("cutover_at")?,
        cutover_by: optional_user(row, "cutover_by_user_id")?,
        post_cutover_verified_at: row.try_get("post_cutover_verified_at")?,
        post_cutover_verified_by: optional_user(row, "post_cutover_verified_by_user_id")?,
        completed_at: row.try_get("completed_at")?,
        completed_by: optional_user(row, "completed_by_user_id")?,
        completion_reason: (status == TenantCellMoveStatus::Completed)
            .then(|| change_reason.clone())
            .flatten(),
        rolled_back_at: row.try_get("rolled_back_at")?,
        rolled_back_by: optional_user(row, "rolled_back_by_user_id")?,
        rollback_reason: (status == TenantCellMoveStatus::RolledBack)
            .then(|| change_reason.clone())
            .flatten(),
        cancelled_at: row.try_get("cancelled_at")?,
        cancelled_by: optional_user(row, "cancelled_by_user_id")?,
        cancellation_reason: (status == TenantCellMoveStatus::Cancelled)
            .then_some(change_reason)
            .flatten(),
        latest_checkpoint: map_checkpoint(row)?,
        validation: map_validation(row)?,
        cutover_verification: map_cutover_verification(row)?,
        rollback_verification: map_rollback_verification(row)?,
        write_frozen: row.try_get("write_frozen")?,
        action_eligibility: Vec::new(),
    };
    result.action_eligibility = action_eligibility(
        &result,
        actor_tenant_id,
        current_data_cell_id,
        current_placement_revision,
        write_fence_epoch,
    );
    Ok(result)
}

fn map_data_cell(
    row: &PgRow,
    columns: DataCellColumns,
) -> AppResult<TenantCellMoveDataCellSummary> {
    let max_tenants: i64 = row.try_get(columns.max_tenants)?;
    Ok(TenantCellMoveDataCellSummary {
        data_cell_id: data_cell_id(row.try_get(columns.id)?)?,
        key: row.try_get(columns.key)?,
        name: row.try_get(columns.name)?,
        region: row.try_get(columns.region)?,
        residency: row.try_get(columns.residency)?,
        mode: parse_cell_mode(row.try_get(columns.mode)?)?,
        status: parse_cell_status(row.try_get(columns.status)?)?,
        revision: DataCellRevision::new(row.try_get(columns.revision)?).map_err(stored_error)?,
        max_tenants: u32::try_from(max_tenants)
            .map_err(|_| AppError::internal("stored data-cell capacity is invalid"))?,
        placement_count: row.try_get(columns.placement_count)?,
        reserved_inbound_move_count: row.try_get(columns.reserved_inbound_move_count)?,
        reserved_rollback_move_count: row.try_get(columns.reserved_rollback_move_count)?,
    })
}

fn map_checkpoint(row: &PgRow) -> AppResult<Option<TenantCellMoveCheckpointReadModel>> {
    let Some(source_lsn) = row.try_get::<Option<String>, _>("checkpoint_source_lsn")? else {
        return Ok(None);
    };
    Ok(Some(TenantCellMoveCheckpointReadModel {
        move_revision: move_revision(required(
            row.try_get("checkpoint_move_revision")?,
            "checkpoint revision",
        )?)?,
        checkpoint: TenantCellMoveCheckpoint::new(TenantCellMoveCheckpointInput {
            source_lsn: parse_lsn(source_lsn)?,
            target_replay_lsn: parse_lsn(required(
                row.try_get("checkpoint_target_replay_lsn")?,
                "checkpoint target replay LSN",
            )?)?,
            copied_row_count: required(
                row.try_get("checkpoint_copied_row_count")?,
                "checkpoint copied row count",
            )?,
            copied_bytes: required(
                row.try_get("checkpoint_copied_bytes")?,
                "checkpoint copied bytes",
            )?,
        })
        .map_err(stored_error)?,
        recorded_at: required(row.try_get("checkpointed_at")?, "checkpoint timestamp")?,
        recorded_by: user_id(required(
            row.try_get("checkpointed_by_user_id")?,
            "checkpoint actor",
        )?)?,
    }))
}

fn map_validation(row: &PgRow) -> AppResult<Option<TenantCellMoveValidationReadModel>> {
    let Some(revision) = row.try_get::<Option<i64>, _>("validation_move_revision")? else {
        return Ok(None);
    };
    let text = |column: &'static str, label: &'static str| -> AppResult<String> {
        required(row.try_get(column)?, label)
    };
    Ok(Some(TenantCellMoveValidationReadModel {
        move_revision: move_revision(revision)?,
        validation: TenantCellMoveValidation::new(TenantCellMoveValidationInput {
            tool_version: TenantCellMoveToolVersion::new(text(
                "validation_tool_version",
                "validation tool version",
            )?)
            .map_err(stored_error)?,
            source_lsn: parse_lsn(text("validation_source_lsn", "validation source LSN")?)?,
            target_replay_lsn: parse_lsn(text(
                "validation_target_replay_lsn",
                "validation target replay LSN",
            )?)?,
            source_row_count: required(
                row.try_get("source_row_count")?,
                "validation source row count",
            )?,
            target_row_count: required(
                row.try_get("target_row_count")?,
                "validation target row count",
            )?,
            source_data_checksum: checksum(text(
                "source_data_checksum",
                "validation source data checksum",
            )?)?,
            target_data_checksum: checksum(text(
                "target_data_checksum",
                "validation target data checksum",
            )?)?,
            source_schema_checksum: checksum(text(
                "source_schema_fingerprint",
                "validation source schema fingerprint",
            )?)?,
            target_schema_checksum: checksum(text(
                "target_schema_fingerprint",
                "validation target schema fingerprint",
            )?)?,
            source_object_manifest_checksum: checksum(text(
                "source_object_manifest_checksum",
                "validation source object manifest checksum",
            )?)?,
            target_object_manifest_checksum: checksum(text(
                "target_object_manifest_checksum",
                "validation target object manifest checksum",
            )?)?,
            inventory_reconciled: required(
                row.try_get("validation_inventory_reconciled")?,
                "validation inventory control",
            )?,
            idempotency_verified: required(
                row.try_get("validation_idempotency_verified")?,
                "validation idempotency control",
            )?,
            outbox_verified: required(
                row.try_get("validation_outbox_verified")?,
                "validation outbox control",
            )?,
        })
        .map_err(stored_error)?,
        validated_at: required(
            row.try_get("validation_evidence_at")?,
            "validation timestamp",
        )?,
        validated_by: user_id(required(
            row.try_get("validation_evidence_by")?,
            "validation actor",
        )?)?,
    }))
}

fn map_cutover_verification(
    row: &PgRow,
) -> AppResult<Option<TenantCellMoveCutoverVerificationReadModel>> {
    let Some(revision) = row.try_get::<Option<i64>, _>("cutover_verification_move_revision")?
    else {
        return Ok(None);
    };
    let text = |column: &'static str, label: &'static str| -> AppResult<String> {
        required(row.try_get(column)?, label)
    };
    Ok(Some(TenantCellMoveCutoverVerificationReadModel {
        move_revision: move_revision(revision)?,
        verification: TenantCellMoveCutoverVerification::new(
            TenantCellMoveCutoverVerificationInput {
                tool_version: TenantCellMoveToolVersion::new(text(
                    "cutover_verification_tool_version",
                    "cutover verification tool version",
                )?)
                .map_err(stored_error)?,
                routing_reference: TenantCellMoveRoutingReference::new(text(
                    "cutover_verification_routing_reference",
                    "cutover verification routing reference",
                )?)
                .map_err(stored_error)?,
                observed_data_cell_id: data_cell_id(required(
                    row.try_get("observed_data_cell_id")?,
                    "cutover verification observed data cell",
                )?)?,
                observed_placement_revision: placement_revision(required(
                    row.try_get("observed_placement_revision")?,
                    "cutover verification observed placement revision",
                )?)?,
                routing_verified: required(
                    row.try_get("routing_verified")?,
                    "cutover routing control",
                )?,
                target_read_verified: required(
                    row.try_get("target_read_verified")?,
                    "cutover target-read control",
                )?,
                write_fence_verified: required(
                    row.try_get("write_fence_verified")?,
                    "cutover write-fence control",
                )?,
                inventory_reconciled: required(
                    row.try_get("cutover_inventory_reconciled")?,
                    "cutover inventory control",
                )?,
                idempotency_verified: required(
                    row.try_get("cutover_idempotency_verified")?,
                    "cutover idempotency control",
                )?,
                outbox_verified: required(
                    row.try_get("cutover_outbox_verified")?,
                    "cutover outbox control",
                )?,
            },
        )
        .map_err(stored_error)?,
        verified_at: required(
            row.try_get("cutover_verification_at")?,
            "cutover verification timestamp",
        )?,
        verified_by: user_id(required(
            row.try_get("cutover_verification_by")?,
            "cutover verification actor",
        )?)?,
    }))
}

fn map_rollback_verification(
    row: &PgRow,
) -> AppResult<Option<TenantCellMoveRollbackVerificationReadModel>> {
    let Some(revision) = row.try_get::<Option<i64>, _>("rollback_verification_move_revision")?
    else {
        return Ok(None);
    };
    let text = |column: &'static str, label: &'static str| -> AppResult<String> {
        required(row.try_get(column)?, label)
    };
    Ok(Some(TenantCellMoveRollbackVerificationReadModel {
        move_revision: move_revision(revision)?,
        verification: TenantCellMoveRollbackVerification::new(
            TenantCellMoveRollbackVerificationInput {
                tool_version: TenantCellMoveToolVersion::new(text(
                    "rollback_verification_tool_version",
                    "rollback verification tool version",
                )?)
                .map_err(stored_error)?,
                routing_reference: TenantCellMoveRoutingReference::new(text(
                    "rollback_verification_routing_reference",
                    "rollback verification routing reference",
                )?)
                .map_err(stored_error)?,
                observed_data_cell_id: data_cell_id(required(
                    row.try_get("rollback_observed_data_cell_id")?,
                    "rollback verification observed data cell",
                )?)?,
                expected_rollback_placement_revision: placement_revision(required(
                    row.try_get("expected_rollback_placement_revision")?,
                    "rollback verification expected placement revision",
                )?)?,
                routing_verified: required(
                    row.try_get("rollback_routing_verified")?,
                    "rollback routing control",
                )?,
                source_read_verified: required(
                    row.try_get("source_read_verified")?,
                    "rollback source-read control",
                )?,
                write_fence_verified: required(
                    row.try_get("rollback_write_fence_verified")?,
                    "rollback write-fence control",
                )?,
                inventory_reconciled: required(
                    row.try_get("rollback_inventory_reconciled")?,
                    "rollback inventory control",
                )?,
                idempotency_verified: required(
                    row.try_get("rollback_idempotency_verified")?,
                    "rollback idempotency control",
                )?,
                outbox_verified: required(
                    row.try_get("rollback_outbox_verified")?,
                    "rollback outbox control",
                )?,
            },
        )
        .map_err(stored_error)?,
        verified_at: required(
            row.try_get("rollback_verification_at")?,
            "rollback verification timestamp",
        )?,
        verified_by: user_id(required(
            row.try_get("rollback_verification_by")?,
            "rollback verification actor",
        )?)?,
    }))
}

fn action_eligibility(
    move_read: &TenantCellMoveReadModel,
    actor_tenant_id: TenantId,
    current_data_cell_id: Option<i64>,
    current_placement_revision: Option<i64>,
    write_fence_epoch: Option<i64>,
) -> Vec<TenantCellMoveActionEligibility> {
    const ACTIONS: [TenantCellMoveAction; 9] = [
        TenantCellMoveAction::StartCopy,
        TenantCellMoveAction::Checkpoint,
        TenantCellMoveAction::Freeze,
        TenantCellMoveAction::Validate,
        TenantCellMoveAction::Cutover,
        TenantCellMoveAction::VerifyCutover,
        TenantCellMoveAction::Complete,
        TenantCellMoveAction::Rollback,
        TenantCellMoveAction::Cancel,
    ];
    ACTIONS
        .into_iter()
        .map(|action| {
            let mut blockers = Vec::new();
            if !action_available(move_read, action) {
                blockers.push(TenantCellMoveBlocker::ActionNotAvailableInStatus);
            } else {
                append_action_blockers(
                    move_read,
                    action,
                    actor_tenant_id,
                    current_data_cell_id,
                    current_placement_revision,
                    write_fence_epoch,
                    &mut blockers,
                );
            }
            TenantCellMoveActionEligibility {
                action,
                eligible: blockers.is_empty(),
                blockers,
            }
        })
        .collect()
}

fn action_available(move_read: &TenantCellMoveReadModel, action: TenantCellMoveAction) -> bool {
    match action {
        TenantCellMoveAction::StartCopy => move_read.status == TenantCellMoveStatus::Planned,
        TenantCellMoveAction::Checkpoint => matches!(
            move_read.status,
            TenantCellMoveStatus::Copying | TenantCellMoveStatus::Frozen
        ),
        TenantCellMoveAction::Freeze => move_read.status == TenantCellMoveStatus::Copying,
        TenantCellMoveAction::Validate => move_read.status == TenantCellMoveStatus::Frozen,
        TenantCellMoveAction::Cutover => move_read.status == TenantCellMoveStatus::Validated,
        TenantCellMoveAction::VerifyCutover => {
            move_read.status == TenantCellMoveStatus::CutOver
                && move_read.cutover_verification.is_none()
        }
        TenantCellMoveAction::Complete | TenantCellMoveAction::Rollback => {
            move_read.status == TenantCellMoveStatus::CutOver
        }
        TenantCellMoveAction::Cancel => matches!(
            move_read.status,
            TenantCellMoveStatus::Planned
                | TenantCellMoveStatus::Copying
                | TenantCellMoveStatus::Frozen
                | TenantCellMoveStatus::Validated
        ),
    }
}

fn append_action_blockers(
    move_read: &TenantCellMoveReadModel,
    action: TenantCellMoveAction,
    actor_tenant_id: TenantId,
    current_data_cell_id: Option<i64>,
    current_placement_revision: Option<i64>,
    write_fence_epoch: Option<i64>,
    blockers: &mut Vec<TenantCellMoveBlocker>,
) {
    if actor_tenant_id == move_read.tenant.tenant_id {
        blockers.push(TenantCellMoveBlocker::ActorTenantMustBeSwitched);
    }

    let pre_cutover_action = matches!(
        action,
        TenantCellMoveAction::StartCopy
            | TenantCellMoveAction::Checkpoint
            | TenantCellMoveAction::Freeze
            | TenantCellMoveAction::Validate
            | TenantCellMoveAction::Cutover
            | TenantCellMoveAction::Cancel
    );
    let post_cutover_action = matches!(
        action,
        TenantCellMoveAction::VerifyCutover
            | TenantCellMoveAction::Complete
            | TenantCellMoveAction::Rollback
    );
    let placement_is_current = if pre_cutover_action {
        current_data_cell_id == Some(move_read.source_cell.data_cell_id.get())
            && current_placement_revision == Some(move_read.source_placement_revision.get())
    } else if post_cutover_action {
        current_data_cell_id == Some(move_read.target_cell.data_cell_id.get())
            && current_placement_revision
                == move_read
                    .cutover_placement_revision
                    .map(DataCellPlacementRevision::get)
    } else {
        true
    };
    if !placement_is_current {
        blockers.push(TenantCellMoveBlocker::SourcePlacementChanged);
    }

    if matches!(
        action,
        TenantCellMoveAction::StartCopy
            | TenantCellMoveAction::Freeze
            | TenantCellMoveAction::Validate
            | TenantCellMoveAction::Cutover
    ) {
        if move_read.target_cell.status != DataCellStatus::Active {
            blockers.push(TenantCellMoveBlocker::TargetNotActive);
        }
        if !cell_has_reserved_capacity(&move_read.target_cell) {
            blockers.push(TenantCellMoveBlocker::TargetCapacityUnavailable);
        }
        if move_read.residency_requirement != "GLOBAL"
            && move_read.residency_requirement != move_read.target_cell.residency
        {
            blockers.push(TenantCellMoveBlocker::ResidencyMismatch);
        }
    }

    if matches!(
        action,
        TenantCellMoveAction::Checkpoint
            | TenantCellMoveAction::Freeze
            | TenantCellMoveAction::Validate
            | TenantCellMoveAction::Cutover
    ) && move_read.copy_reference.is_none()
    {
        blockers.push(TenantCellMoveBlocker::CopyReferenceMissing);
    }

    if matches!(
        action,
        TenantCellMoveAction::Freeze | TenantCellMoveAction::Validate
    ) && move_read.latest_checkpoint.is_none()
    {
        blockers.push(TenantCellMoveBlocker::CheckpointMissing);
    }
    if action == TenantCellMoveAction::Validate
        && move_read
            .latest_checkpoint
            .as_ref()
            .zip(write_fence_epoch)
            .is_some_and(|(checkpoint, freeze_revision)| {
                checkpoint.move_revision.get() <= freeze_revision
                    || checkpoint.move_revision != move_read.revision
            })
    {
        blockers.push(TenantCellMoveBlocker::ValidationStale);
    }

    if matches!(
        action,
        TenantCellMoveAction::Validate
            | TenantCellMoveAction::Cutover
            | TenantCellMoveAction::VerifyCutover
            | TenantCellMoveAction::Complete
            | TenantCellMoveAction::Rollback
    ) && !move_read.write_frozen
    {
        blockers.push(TenantCellMoveBlocker::WriteFenceMissing);
    }

    if matches!(
        action,
        TenantCellMoveAction::Cutover
            | TenantCellMoveAction::VerifyCutover
            | TenantCellMoveAction::Complete
            | TenantCellMoveAction::Rollback
    ) {
        match move_read.validation.as_ref() {
            None => blockers.push(TenantCellMoveBlocker::ValidationMissing),
            Some(validation)
                if action == TenantCellMoveAction::Cutover
                    && validation.move_revision != move_read.revision =>
            {
                blockers.push(TenantCellMoveBlocker::ValidationStale);
            }
            Some(_) => {}
        }
    }

    if action == TenantCellMoveAction::Complete {
        let verified = move_read
            .cutover_verification
            .as_ref()
            .is_some_and(|evidence| {
                evidence.move_revision == move_read.revision
                    && evidence.verification.observed_data_cell_id()
                        == move_read.target_cell.data_cell_id
                    && Some(evidence.verification.observed_placement_revision())
                        == move_read.cutover_placement_revision
            });
        if !verified {
            blockers.push(TenantCellMoveBlocker::PostCutoverVerificationMissing);
        }
    }
}

fn cell_has_reserved_capacity(cell: &TenantCellMoveDataCellSummary) -> bool {
    let Some(occupied) = cell
        .placement_count
        .checked_add(cell.reserved_inbound_move_count)
        .and_then(|value| value.checked_add(cell.reserved_rollback_move_count))
    else {
        return false;
    };
    occupied >= 0
        && occupied <= i64::from(cell.max_tenants)
        && (cell.mode != DataCellMode::Dedicated || occupied <= 1)
}

fn map_event_row(row: &PgRow) -> AppResult<TenantCellMoveEventReadModel> {
    let action = parse_event_action(row.try_get("action")?)?;
    let resulting_placement_revision = match action {
        TenantCellMoveEventAction::CutOver => row
            .try_get::<Option<i64>, _>("cutover_placement_revision")?
            .map(placement_revision)
            .transpose()?,
        TenantCellMoveEventAction::RolledBack => row
            .try_get::<Option<i64>, _>("rollback_placement_revision")?
            .map(placement_revision)
            .transpose()?,
        _ => None,
    };
    Ok(TenantCellMoveEventReadModel {
        event_id: row.try_get("event_id")?,
        tenant_cell_move_id: move_id(row.try_get("tenant_cell_move_id")?)?,
        tenant_id: tenant_id(row.try_get("tenant_id")?)?,
        action,
        move_revision: move_revision(row.try_get("move_revision")?)?,
        previous_status: row
            .try_get::<Option<String>, _>("previous_status")?
            .map(parse_move_status)
            .transpose()?,
        resulting_status: parse_move_status(row.try_get("resulting_status")?)?,
        source_placement_revision: placement_revision(row.try_get("source_placement_revision")?)?,
        resulting_placement_revision,
        actor_id: user_id(row.try_get("actor_user_id")?)?,
        occurred_at: row.try_get("occurred_at")?,
        reason: row.try_get("reason")?,
        request_id: row.try_get("request_id")?,
        evidence: row.try_get("evidence")?,
    })
}

fn parse_move_status(value: String) -> AppResult<TenantCellMoveStatus> {
    TenantCellMoveStatus::parse(&value).ok_or_else(|| {
        AppError::internal(format!(
            "stored tenant-cell-move status is invalid: {value}"
        ))
    })
}

fn parse_event_action(value: String) -> AppResult<TenantCellMoveEventAction> {
    match value.as_str() {
        "planned" => Ok(TenantCellMoveEventAction::Planned),
        "copy_started" => Ok(TenantCellMoveEventAction::CopyStarted),
        "checkpoint_recorded" => Ok(TenantCellMoveEventAction::CheckpointRecorded),
        "writes_frozen" => Ok(TenantCellMoveEventAction::WritesFrozen),
        "validated" => Ok(TenantCellMoveEventAction::Validated),
        "cut_over" => Ok(TenantCellMoveEventAction::CutOver),
        "post_cutover_verified" => Ok(TenantCellMoveEventAction::PostCutoverVerified),
        "completed" => Ok(TenantCellMoveEventAction::Completed),
        "rolled_back" => Ok(TenantCellMoveEventAction::RolledBack),
        "cancelled" => Ok(TenantCellMoveEventAction::Cancelled),
        _ => Err(AppError::internal(format!(
            "stored tenant-cell-move event action is invalid: {value}"
        ))),
    }
}

fn parse_tenant_status(value: String) -> AppResult<TenantStatus> {
    TenantStatus::parse(&value)
        .ok_or_else(|| AppError::internal(format!("stored tenant status is invalid: {value}")))
}

fn parse_cell_mode(value: String) -> AppResult<DataCellMode> {
    DataCellMode::parse(&value)
        .ok_or_else(|| AppError::internal(format!("stored data-cell mode is invalid: {value}")))
}

fn parse_cell_status(value: String) -> AppResult<DataCellStatus> {
    DataCellStatus::parse(&value)
        .ok_or_else(|| AppError::internal(format!("stored data-cell status is invalid: {value}")))
}

fn parse_lsn(value: String) -> AppResult<PostgresLsn> {
    value.parse().map_err(stored_error)
}

fn checksum(value: String) -> AppResult<Sha256Checksum> {
    Sha256Checksum::new(value).map_err(stored_error)
}

fn tenant_id(value: i64) -> AppResult<TenantId> {
    TenantId::new(value).map_err(stored_error)
}

fn user_id(value: i64) -> AppResult<UserId> {
    UserId::new(value).map_err(stored_error)
}

fn data_cell_id(value: i64) -> AppResult<DataCellId> {
    DataCellId::new(value).map_err(stored_error)
}

fn move_id(value: i64) -> AppResult<TenantCellMoveId> {
    TenantCellMoveId::new(value).map_err(stored_error)
}

fn move_revision(value: i64) -> AppResult<TenantCellMoveRevision> {
    TenantCellMoveRevision::new(value).map_err(stored_error)
}

fn placement_revision(value: i64) -> AppResult<DataCellPlacementRevision> {
    DataCellPlacementRevision::new(value).map_err(stored_error)
}

fn optional_user(row: &PgRow, column: &'static str) -> AppResult<Option<UserId>> {
    row.try_get::<Option<i64>, _>(column)?
        .map(user_id)
        .transpose()
}

fn required<T>(value: Option<T>, label: &'static str) -> AppResult<T> {
    value.ok_or_else(|| AppError::internal(format!("stored tenant-cell-move {label} is missing")))
}

fn stored_error(error: impl std::fmt::Display) -> AppError {
    AppError::internal(format!("stored tenant-cell-move data is invalid: {error}"))
}
