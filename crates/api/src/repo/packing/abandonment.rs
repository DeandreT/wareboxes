use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::packing::{
    AbandonPackSessionCommand, AbandonPackSessionResult, ABANDON_PACK_SESSION_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    abandon_empty_packing, PackSessionAbandonmentReason, PackSessionId, PackSessionStatus,
    PackingProgress, TenantId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::insert_order_activity_tx;

use super::{
    enqueue_order_event_tx, lock_order_tx, lock_session_tx, require_replayed_ids_visible_tx,
    require_revision, session_order_hint_tx,
};

pub async fn abandon_session(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &AbandonPackSessionCommand,
) -> AppResult<AbandonPackSessionResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, ABANDON_PACK_SESSION_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;

    require_stored_abandonment_visible_before_replay_tx(
        &mut tx,
        access.tenant_id,
        prepared.idempotency_key(),
        &scope,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<AbandonPackSessionResult>(&mut tx)
        .await?
    {
        require_replayed_ids_visible_tx(
            &mut tx,
            access.tenant_id,
            result.session_id,
            result.order_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order_id = session_order_hint_tx(&mut tx, access.tenant_id, command.session_id).await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
    let session = lock_session_tx(&mut tx, access.tenant_id, command.session_id, &scope).await?;
    if session.order_id != order_id || session.state != "open" {
        return Err(AppError::conflict("packing session is not open"));
    }
    let revision = require_revision(&order, Some(&session), command.expected_revision)?;
    let progress = PackingProgress::new(
        session.expected_allocation_count,
        session.packed_allocation_count,
        session.expected_qty,
        session.packed_qty,
        session.open_carton_count,
        session.closed_carton_count,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let order_status = abandon_empty_packing(order.status, progress)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    lock_and_require_empty_execution_tx(&mut tx, access.tenant_id, command.session_id.get())
        .await?;

    let abandoned_at = now_iso();
    let reason = reason_code(command.details.reason());
    let note = command.details.note().map(|value| value.as_str());
    let session_update = sqlx::query(
        r#"
        UPDATE packing_sessions
        SET state='abandoned', revision=$1, abandonment_reason=$2,
            abandonment_note=$3, abandoned_by_user_id=$4, abandoned_at=$5
        WHERE tenant_id=$6 AND id=$7 AND state='open' AND revision=$8
        "#,
    )
    .bind(revision.get())
    .bind(reason)
    .bind(note)
    .bind(context.actor_id.get())
    .bind(abandoned_at)
    .bind(access.tenant_id.get())
    .bind(command.session_id.get())
    .bind(command.expected_revision.get())
    .execute(&mut *tx)
    .await?;
    if session_update.rows_affected() != 1 {
        return Err(AppError::conflict(
            "packing session changed during abandonment",
        ));
    }
    let order_update = sqlx::query(
        r#"
        UPDATE orders SET status=$1, revision=$2
        WHERE tenant_id=$3 AND id=$4 AND status=$5 AND revision=$6
        "#,
    )
    .bind(order_status.as_str())
    .bind(revision.get())
    .bind(access.tenant_id.get())
    .bind(order_id.get())
    .bind(order.status.as_str())
    .bind(command.expected_revision.get())
    .execute(&mut *tx)
    .await?;
    if order_update.rows_affected() != 1 {
        return Err(AppError::conflict(
            "order changed during pack-session abandonment",
        ));
    }

    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        order_id.get(),
        Some(context.actor_id.get()),
        &format!("abandoned empty packing session {} ({reason})", session.id),
    )
    .await?;
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        order.inventory_owner_id,
        session.facility_id,
        context.actor_id.get(),
        order_id,
        "packing.session_abandoned",
        &format!("packing-session:{}:abandoned", session.id.get()),
        serde_json::json!({
            "packing_session_id": session.id,
            "order_id": order_id,
            "facility_id": session.facility_id,
            "expected_revision": command.expected_revision,
            "revision": revision,
            "reason": reason,
            "note": note,
            "abandoned_by": context.actor_id,
            "abandoned_at": abandoned_at,
        }),
        abandoned_at,
    )
    .await?;

    let result = AbandonPackSessionResult {
        session_id: session.id,
        order_id,
        previous_order_status: order.status,
        order_status,
        session_status: PackSessionStatus::Abandoned,
        revision,
        progress,
        details: command.details.clone(),
        abandoned_by: context.actor_id,
        abandoned_at,
    };
    Ok(prepared.commit(tx, result).await?)
}

async fn require_stored_abandonment_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    idempotency_key: &str,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored: Option<(i64, i64)> = sqlx::query_as(
        r#"
        SELECT (result_json->>'session_id')::BIGINT,
               (result_json->>'order_id')::BIGINT
        FROM command_idempotency_records
        WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3
        "#,
    )
    .bind(tenant_id.get())
    .bind(ABANDON_PACK_SESSION_OPERATION)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((session_id, order_id)) = stored {
        require_replayed_ids_visible_tx(
            tx,
            tenant_id,
            PackSessionId::new(session_id)
                .map_err(|error| AppError::internal(error.to_string()))?,
            wareboxes_domain::OrderId::new(order_id)
                .map_err(|error| AppError::internal(error.to_string()))?,
            scope,
        )
        .await?;
    }
    Ok(())
}

async fn lock_and_require_empty_execution_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: i64,
) -> AppResult<()> {
    let carton_rows = sqlx::query(
        r#"
        SELECT state FROM cartons
        WHERE tenant_id=$1 AND packing_session_id=$2
        ORDER BY id FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in carton_rows {
        if row.try_get::<String, _>("state")? != "voided" {
            return Err(AppError::conflict(
                "every packing carton must be empty and voided before abandonment",
            ));
        }
    }
    let active_positions: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM packing_allocation_positions
        WHERE tenant_id=$1 AND packing_session_id=$2 AND state='packed'
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id)
    .fetch_one(&mut **tx)
    .await?;
    let downstream_exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (SELECT 1 FROM outbound_qa_sessions
                       WHERE tenant_id=$1 AND packing_session_id=$2)
            OR EXISTS (SELECT 1 FROM shipments
                       WHERE tenant_id=$1 AND packing_session_id=$2)
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id)
    .fetch_one(&mut **tx)
    .await?;
    if active_positions != 0 || downstream_exists {
        return Err(AppError::conflict(
            "packing session still has physical or downstream execution",
        ));
    }
    Ok(())
}

const fn reason_code(reason: PackSessionAbandonmentReason) -> &'static str {
    match reason {
        PackSessionAbandonmentReason::OrderCancellation => "order_cancellation",
        PackSessionAbandonmentReason::Repack => "repack",
        PackSessionAbandonmentReason::StationIssue => "station_issue",
        PackSessionAbandonmentReason::Other => "other",
    }
}
