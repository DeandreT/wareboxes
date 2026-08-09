use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbound_qa::{
    CompleteOutboundQaCommand, CompleteOutboundQaResult, OutboundQaSessionReadModel,
    StartOutboundQaCommand, StartOutboundQaResult, VerifyOutboundQaCartonCommand,
    VerifyOutboundQaCartonResult, COMPLETE_OUTBOUND_QA_OPERATION, START_OUTBOUND_QA_OPERATION,
    VERIFY_OUTBOUND_QA_CARTON_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    begin_outbound_qa, complete_outbound_qa, record_outbound_qa_carton, CartonId, FacilityId,
    InventoryOwnerId, LicensePlateId, OrderId, OutboundQaCartonVerificationId, OutboundQaProgress,
    OutboundQaScanValue, OutboundQaSessionId, OutboundQaSessionRevision, OutboundQaSessionStatus,
    PackSessionId, TenantId,
};
use wareboxes_persistence_postgres::db::{
    begin_tenant_transaction, bind_tenant_context, now_iso, Db,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{
    current_scope_tx, lock_current_scope_tx, require_permission_tx, ScopeBindings,
};
use crate::repo::orders::insert_order_activity_tx;

use super::{
    active_policy_tx, enqueue_event_tx, load_session_tx, require_scope,
    require_stored_visible_before_replay_tx,
};

#[derive(Debug)]
struct SessionHint {
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    order_id: OrderId,
}

#[derive(Debug)]
struct LockedSession {
    id: OutboundQaSessionId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    packing_session_id: PackSessionId,
    order_id: OrderId,
    status: OutboundQaSessionStatus,
    revision: OutboundQaSessionRevision,
    progress: OutboundQaProgress,
}

#[derive(Debug)]
struct CartonTarget {
    carton_id: CartonId,
    license_plate_id: LicensePlateId,
    sequence: i64,
    barcode: OutboundQaScanValue,
    content_count: i64,
    packed_qty: i64,
}

pub async fn start(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &StartOutboundQaCommand,
) -> AppResult<StartOutboundQaResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, START_OUTBOUND_QA_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed::<StartOutboundQaResult>(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let hint =
        session_hint_for_packing_tx(&mut tx, access.tenant_id, command.packing_session_id).await?;
    require_scope(
        &scope,
        hint.owner_id.get(),
        hint.facility_id.get(),
        "packing session",
    )?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "shipment-order:{}:{}",
            access.tenant_id, hint.order_id
        ))
        .execute(&mut *tx)
        .await?;
    let order_revision: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT revision FROM orders
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND id=$3
          AND status='awaiting shipment' AND deleted IS NULL
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(hint.owner_id.get())
    .bind(hint.order_id.get())
    .fetch_optional(&mut *tx)
    .await?;
    if order_revision != Some(command.expected_order_revision.get()) {
        return Err(AppError::conflict(
            "outbound QA order revision is stale or order is not ready",
        ));
    }
    let carton_count: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT closed_carton_count FROM packing_sessions
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND order_id=$4 AND id=$5 AND state='ready_to_manifest'
          AND revision=$6
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(hint.owner_id.get())
    .bind(hint.facility_id.get())
    .bind(hint.order_id.get())
    .bind(command.packing_session_id.get())
    .bind(command.expected_order_revision.get())
    .fetch_optional(&mut *tx)
    .await?;
    let carton_count = carton_count
        .ok_or_else(|| AppError::conflict("packing session is not ready for outbound QA"))?;
    let policy = active_policy_tx(
        &mut tx,
        access.tenant_id,
        hint.owner_id,
        hint.facility_id,
        true,
    )
    .await?
    .ok_or_else(|| AppError::conflict("outbound QA is not required at this scope"))?;
    let (status, progress) = begin_outbound_qa(policy.requirement, carton_count)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let existing: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (SELECT 1 FROM outbound_qa_sessions
        WHERE tenant_id=$1 AND packing_session_id=$2 AND policy_id=$3)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.packing_session_id.get())
    .bind(policy.policy_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if existing {
        return Err(AppError::conflict(
            "outbound QA session already exists for this policy",
        ));
    }
    let started_at = now_iso();
    let session_id_raw: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO outbound_qa_sessions (
            tenant_id,inventory_owner_id,facility_id,packing_session_id,order_id,
            policy_id,policy_revision,state,revision,expected_order_revision,
            expected_carton_count,verified_carton_count,started_by_user_id,started_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,1,$9,$10,0,$11,$12)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(hint.owner_id.get())
    .bind(hint.facility_id.get())
    .bind(command.packing_session_id.get())
    .bind(hint.order_id.get())
    .bind(policy.policy_id.get())
    .bind(policy.revision.get())
    .bind(status.as_str())
    .bind(command.expected_order_revision.get())
    .bind(progress.expected_carton_count())
    .bind(context.actor_id.get())
    .bind(started_at)
    .fetch_one(&mut *tx)
    .await?;
    let session_id = OutboundQaSessionId::new(session_id_raw)
        .map_err(|error| AppError::internal(error.to_string()))?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        hint.owner_id,
        hint.order_id.get(),
        Some(context.actor_id.get()),
        &format!("started outbound QA for {carton_count} carton(s)"),
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        hint.owner_id,
        hint.facility_id,
        context.actor_id.get(),
        &format!("order:{}", hint.order_id),
        "order",
        hint.order_id.get(),
        "outbound.qa.started",
        &format!("qa:{session_id}:started"),
        &serde_json::json!({
            "session_id": session_id,
            "packing_session_id": command.packing_session_id,
            "order_id": hint.order_id,
            "policy_id": policy.policy_id,
            "policy_revision": policy.revision,
            "expected_carton_count": carton_count,
            "started_at": started_at,
        }),
        started_at,
    )
    .await?;
    let result = load_session_tx(&mut tx, access.tenant_id, session_id.get()).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn verify_carton(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &VerifyOutboundQaCartonCommand,
) -> AppResult<VerifyOutboundQaCartonResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, VERIFY_OUTBOUND_QA_CARTON_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<VerifyOutboundQaCartonResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let session = lock_session_tx(&mut tx, access.tenant_id, command.session_id, &scope).await?;
    if session.revision != command.expected_revision {
        return Err(AppError::conflict("outbound QA session revision is stale"));
    }
    let carton =
        lock_carton_by_scan_tx(&mut tx, access.tenant_id, &session, &command.carton_barcode)
            .await?;
    let duplicate: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (SELECT 1 FROM outbound_qa_carton_verifications
        WHERE tenant_id=$1 AND outbound_qa_session_id=$2 AND carton_id=$3)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(session.id.get())
    .bind(carton.carton_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let next_progress = record_outbound_qa_carton(session.status, session.progress, duplicate)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let next_revision = session
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("outbound QA session revision overflow"))?;
    let verified_at = now_iso();
    let verification_id_raw: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO outbound_qa_carton_verifications (
            tenant_id,inventory_owner_id,facility_id,outbound_qa_session_id,
            packing_session_id,order_id,carton_id,license_plate_id,sequence,
            carton_barcode,content_count,packed_qty,expected_session_revision,
            resulting_session_revision,verified_by_user_id,verified_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(session.owner_id.get())
    .bind(session.facility_id.get())
    .bind(session.id.get())
    .bind(session.packing_session_id.get())
    .bind(session.order_id.get())
    .bind(carton.carton_id.get())
    .bind(carton.license_plate_id.get())
    .bind(carton.sequence)
    .bind(carton.barcode.as_str())
    .bind(carton.content_count)
    .bind(carton.packed_qty)
    .bind(session.revision.get())
    .bind(next_revision.get())
    .bind(context.actor_id.get())
    .bind(verified_at)
    .fetch_one(&mut *tx)
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE outbound_qa_sessions
        SET revision=$1,verified_carton_count=$2
        WHERE tenant_id=$3 AND id=$4 AND state='open' AND revision=$5
        "#,
    )
    .bind(next_revision.get())
    .bind(next_progress.verified_carton_count())
    .bind(access.tenant_id.get())
    .bind(session.id.get())
    .bind(session.revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("outbound QA session changed"));
    }
    let verification_id = OutboundQaCartonVerificationId::new(verification_id_raw)
        .map_err(|error| AppError::internal(error.to_string()))?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        session.owner_id,
        session.facility_id,
        context.actor_id.get(),
        &format!("order:{}", session.order_id),
        "order",
        session.order_id.get(),
        "outbound.qa.carton_verified",
        &format!("qa:{}:verification:{verification_id}", session.id),
        &serde_json::json!({
            "session_id": session.id,
            "verification_id": verification_id,
            "carton_id": carton.carton_id,
            "sequence": carton.sequence,
            "verified_carton_count": next_progress.verified_carton_count(),
            "expected_carton_count": next_progress.expected_carton_count(),
            "session_revision": next_revision,
            "verified_at": verified_at,
        }),
        verified_at,
    )
    .await?;
    let result = load_session_tx(&mut tx, access.tenant_id, session.id.get()).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn complete(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CompleteOutboundQaCommand,
) -> AppResult<CompleteOutboundQaResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, COMPLETE_OUTBOUND_QA_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<CompleteOutboundQaResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let session = lock_session_tx(&mut tx, access.tenant_id, command.session_id, &scope).await?;
    if session.revision != command.expected_revision {
        return Err(AppError::conflict("outbound QA session revision is stale"));
    }
    let status = complete_outbound_qa(session.status, session.progress)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let next_revision = session
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("outbound QA session revision overflow"))?;
    let completed_at = now_iso();
    sqlx::query(
        r#"
        INSERT INTO outbound_qa_completions (
            tenant_id,inventory_owner_id,facility_id,outbound_qa_session_id,
            packing_session_id,order_id,expected_session_revision,
            resulting_session_revision,carton_count,completed_by_user_id,completed_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(session.owner_id.get())
    .bind(session.facility_id.get())
    .bind(session.id.get())
    .bind(session.packing_session_id.get())
    .bind(session.order_id.get())
    .bind(session.revision.get())
    .bind(next_revision.get())
    .bind(session.progress.expected_carton_count())
    .bind(context.actor_id.get())
    .bind(completed_at)
    .execute(&mut *tx)
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE outbound_qa_sessions
        SET state=$1,revision=$2,passed_by_user_id=$3,passed_at=$4
        WHERE tenant_id=$5 AND id=$6 AND state='open' AND revision=$7
        "#,
    )
    .bind(status.as_str())
    .bind(next_revision.get())
    .bind(context.actor_id.get())
    .bind(completed_at)
    .bind(access.tenant_id.get())
    .bind(session.id.get())
    .bind(session.revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict("outbound QA session changed"));
    }
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        session.owner_id,
        session.order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "passed outbound QA for {} carton(s)",
            session.progress.expected_carton_count()
        ),
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        session.owner_id,
        session.facility_id,
        context.actor_id.get(),
        &format!("order:{}", session.order_id),
        "order",
        session.order_id.get(),
        "outbound.qa.passed",
        &format!("qa:{}:passed", session.id),
        &serde_json::json!({
            "session_id": session.id,
            "packing_session_id": session.packing_session_id,
            "order_id": session.order_id,
            "carton_count": session.progress.expected_carton_count(),
            "session_revision": next_revision,
            "passed_at": completed_at,
        }),
        completed_at,
    )
    .await?;
    let result = load_session_tx(&mut tx, access.tenant_id, session.id.get()).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn get_session(
    db: &Db,
    access: &TenantAccess,
    session_id: OutboundQaSessionId,
) -> AppResult<OutboundQaSessionReadModel> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let hint = hint_for_session_tx(&mut tx, access.tenant_id, session_id).await?;
    require_scope(
        &scope,
        hint.owner_id.get(),
        hint.facility_id.get(),
        "outbound QA session",
    )?;
    let result = load_session_tx(&mut tx, access.tenant_id, session_id.get()).await?;
    tx.commit().await?;
    Ok(result)
}

async fn session_hint_for_packing_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    packing_session_id: PackSessionId,
) -> AppResult<SessionHint> {
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id,order_id FROM packing_sessions WHERE tenant_id=$1 AND id=$2",
    )
    .bind(tenant_id.get())
    .bind(packing_session_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("packing session"))?;
    hint_from_row(&row)
}

async fn hint_for_session_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: OutboundQaSessionId,
) -> AppResult<SessionHint> {
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id,order_id FROM outbound_qa_sessions WHERE tenant_id=$1 AND id=$2",
    )
    .bind(tenant_id.get())
    .bind(session_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("outbound QA session"))?;
    hint_from_row(&row)
}

fn hint_from_row(row: &sqlx::postgres::PgRow) -> AppResult<SessionHint> {
    Ok(SessionHint {
        owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

async fn lock_session_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: OutboundQaSessionId,
    scope: &ScopeBindings,
) -> AppResult<LockedSession> {
    let row = sqlx::query(
        r#"
        SELECT id,inventory_owner_id,facility_id,packing_session_id,order_id,
               state,revision,expected_carton_count,verified_carton_count
        FROM outbound_qa_sessions
        WHERE tenant_id=$1 AND id=$2
          AND ($3 OR facility_id=ANY($4))
          AND ($5 OR inventory_owner_id=ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("outbound QA session"))?;
    let state: String = row.try_get("state")?;
    Ok(LockedSession {
        id: OutboundQaSessionId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        packing_session_id: PackSessionId::new(row.try_get("packing_session_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status: OutboundQaSessionStatus::parse(&state)
            .ok_or_else(|| AppError::internal("outbound QA session has invalid status"))?,
        revision: OutboundQaSessionRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        progress: OutboundQaProgress::new(
            row.try_get("expected_carton_count")?,
            row.try_get("verified_carton_count")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

async fn lock_carton_by_scan_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session: &LockedSession,
    scan: &OutboundQaScanValue,
) -> AppResult<CartonTarget> {
    let row = sqlx::query(
        r#"
        SELECT carton.id, carton.license_plate_id, plate.barcode,
               (SELECT COUNT(*)::bigint FROM cartons preceding
                WHERE preceding.tenant_id=carton.tenant_id
                  AND preceding.packing_session_id=carton.packing_session_id
                  AND preceding.state='closed' AND preceding.id <= carton.id) AS sequence
        FROM cartons carton
        JOIN license_plates plate
          ON plate.tenant_id=carton.tenant_id
         AND plate.inventory_owner_id=carton.inventory_owner_id
         AND plate.facility_id=carton.facility_id
         AND plate.id=carton.license_plate_id
        WHERE carton.tenant_id=$1 AND carton.inventory_owner_id=$2
          AND carton.facility_id=$3 AND carton.packing_session_id=$4
          AND carton.state='closed' AND plate.deleted IS NULL AND plate.barcode=$5
        FOR SHARE OF carton,plate
        "#,
    )
    .bind(tenant_id.get())
    .bind(session.owner_id.get())
    .bind(session.facility_id.get())
    .bind(session.packing_session_id.get())
    .bind(scan.as_str())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::bad_request("carton scan does not match this packing session"))?;
    let carton_id: i64 = row.try_get("id")?;
    let totals = sqlx::query(
        r#"
        SELECT COUNT(*)::bigint AS content_count,
               COALESCE(SUM(packed_qty),0)::bigint AS packed_qty
        FROM carton_contents content
        INNER JOIN packing_allocation_positions position
          ON position.tenant_id=content.tenant_id
         AND position.inventory_owner_id=content.inventory_owner_id
         AND position.facility_id=content.facility_id
         AND position.packing_session_id=content.packing_session_id
         AND position.packing_session_allocation_id=content.packing_session_allocation_id
         AND position.current_carton_content_id=content.id
         AND position.state='packed'
        WHERE content.tenant_id=$1 AND content.inventory_owner_id=$2
          AND content.facility_id=$3 AND content.packing_session_id=$4
          AND content.carton_id=$5
        "#,
    )
    .bind(tenant_id.get())
    .bind(session.owner_id.get())
    .bind(session.facility_id.get())
    .bind(session.packing_session_id.get())
    .bind(carton_id)
    .fetch_one(&mut **tx)
    .await?;
    let content_count: i64 = totals.try_get("content_count")?;
    let packed_qty: i64 = totals.try_get("packed_qty")?;
    if content_count <= 0 || packed_qty <= 0 {
        return Err(AppError::conflict("packed carton contents changed"));
    }
    Ok(CartonTarget {
        carton_id: CartonId::new(carton_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        license_plate_id: LicensePlateId::new(row.try_get("license_plate_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        sequence: row.try_get("sequence")?,
        barcode: OutboundQaScanValue::new(row.try_get::<String, _>("barcode")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        content_count,
        packed_qty,
    })
}
