use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::inbound_load::{
    CloseInboundLoadCommand, CloseInboundLoadResult, InboundLoadClosedStatus,
    InboundLoadReceivedStatus, CLOSE_INBOUND_LOAD_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{validate_inbound_load_closure, InboundLoadClosureId, LocationId};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};

pub async fn close_inbound_load(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CloseInboundLoadCommand,
) -> AppResult<CloseInboundLoadResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CLOSE_INBOUND_LOAD_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_closure_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed::<CloseInboundLoadResult>(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }

    let row = sqlx::query(
        r#"
        SELECT inventory_owner_id, facility_id, type, status, execution_barcode,
               dock_door_location_id, receive_completed
        FROM loads
        WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
          AND ($3 OR facility_id=ANY($4))
          AND ($5 OR inventory_owner_id=ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.load_id().get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("inbound load"))?;
    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    let load_type: String = row.try_get("type")?;
    let status: String = row.try_get("status")?;
    let execution_barcode: String = row.try_get("execution_barcode")?;
    let receiving_location_id: Option<i64> = row.try_get("dock_door_location_id")?;
    let receive_completed: bool = row.try_get("receive_completed")?;
    if load_type != "inbound" {
        return Err(AppError::not_found("inbound load"));
    }
    if status != "received" || !receive_completed {
        return Err(AppError::conflict(
            "inbound load must be fully received before it can be closed",
        ));
    }
    let unresolved_line_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM load_lines
        WHERE tenant_id=$1 AND load_id=$2 AND deleted IS NULL
          AND status IN ('pending','partial')
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.load_id().get())
    .fetch_one(&mut *tx)
    .await?;
    if unresolved_line_count != 0 {
        return Err(AppError::conflict(
            "inbound load has unresolved receiving lines",
        ));
    }
    if !execution_barcode.eq_ignore_ascii_case(command.load_scan().as_str()) {
        return Err(AppError::bad_request(
            "load scan does not match inbound load",
        ));
    }
    let receiving_location_id = receiving_location_id
        .ok_or_else(|| AppError::conflict("inbound load has no assigned receiving location"))?;
    let location_barcode: Option<String> = sqlx::query_scalar(
        r#"
        SELECT barcode FROM locations
        WHERE tenant_id=$1 AND facility_id=$2 AND id=$3
          AND deleted IS NULL AND active AND receivable
          AND NULLIF(BTRIM(barcode),'') IS NOT NULL
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(facility_id)
    .bind(receiving_location_id)
    .fetch_optional(&mut *tx)
    .await?;
    let location_barcode = location_barcode
        .ok_or_else(|| AppError::conflict("assigned receiving location is no longer executable"))?;
    if !location_barcode.eq_ignore_ascii_case(command.receiving_location_scan().as_str()) {
        return Err(AppError::bad_request(
            "receiving location scan does not match the assigned location",
        ));
    }
    let server_time = now_iso();
    let closed_at = command.closed_at().copied().unwrap_or(server_time);
    validate_inbound_load_closure(closed_at, server_time)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    let closure_id = InboundLoadClosureId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO inbound_load_closures
                (tenant_id,inventory_owner_id,facility_id,load_id,receiving_location_id,
                 observed_load_barcode,observed_receiving_location_barcode,
                 closed_by_user_id,closed_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(command.load_id().get())
        .bind(receiving_location_id)
        .bind(command.load_scan().as_str())
        .bind(command.receiving_location_scan().as_str())
        .bind(context.actor_id.get())
        .bind(closed_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let updated = sqlx::query(
        r#"
        UPDATE loads SET status='closed', closed=$1, closed_by=$2
        WHERE tenant_id=$3 AND id=$4 AND status='received'
          AND receive_completed AND deleted IS NULL
        "#,
    )
    .bind(closed_at)
    .bind(context.actor_id.get())
    .bind(access.tenant_id.get())
    .bind(command.load_id().get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "inbound load state changed while it was closed",
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO load_activity
            (tenant_id,created,load_id,user_id,action,message,metadata_json)
        VALUES ($1,$2,$3,$4,'closed','inbound load closed after receiving',$5)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(server_time)
    .bind(command.load_id().get())
    .bind(context.actor_id.get())
    .bind(
        serde_json::json!({
            "closure_id": closure_id.get(),
            "receiving_location_id": receiving_location_id,
            "closed_at": closed_at,
        })
        .to_string(),
    )
    .execute(&mut *tx)
    .await?;
    let result = CloseInboundLoadResult {
        closure_id,
        load_id: command.load_id(),
        previous_status: InboundLoadReceivedStatus::Received,
        status: InboundLoadClosedStatus::Closed,
        receiving_location_id: LocationId::new(receiving_location_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        closed_by: context.actor_id,
        closed_at,
    };
    enqueue_closed_event(
        &mut tx,
        access,
        context,
        inventory_owner_id,
        facility_id,
        &result,
    )
    .await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

async fn require_stored_closure_visible_before_replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored_load_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT (result_json->>'load_id')::BIGINT
        FROM command_idempotency_records
        WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(CLOSE_INBOUND_LOAD_OPERATION)
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(load_id) = stored_load_id else {
        return Ok(());
    };
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM loads
            WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL
              AND ($3 OR facility_id=ANY($4))
              AND ($5 OR inventory_owner_id=ANY($6))
        )
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(load_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("inbound load"))
    }
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_closed_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: i64,
    facility_id: i64,
    result: &CloseInboundLoadResult,
) -> AppResult<()> {
    let ordering_key = format!("inbound-load:{}", result.load_id.get());
    let sequence: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT last_sequence FROM outbox_aggregate_sequences WHERE tenant_id=$1 AND ordering_key=$2),0)+1",
    )
    .bind(access.tenant_id.get())
    .bind(&ordering_key)
    .fetch_one(&mut **tx)
    .await?;
    let event_key = format!("inbound-load:{}:closed", result.load_id.get());
    let aggregate_id = result.load_id.to_string();
    let payload = serde_json::json!({
        "closure_id": result.closure_id.get(),
        "load_id": result.load_id.get(),
        "previous_status": "received",
        "status": "closed",
        "inventory_owner_id": inventory_owner_id,
        "facility_id": facility_id,
        "receiving_location_id": result.receiving_location_id.get(),
        "closed_by": result.closed_by.get(),
        "closed_at": result.closed_at,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(
                wareboxes_domain::InventoryOwnerId::new(inventory_owner_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            facility_id: Some(
                wareboxes_domain::FacilityId::new(facility_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "inbound_load",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "inbound.load.closed",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.closed_at,
        },
    )
    .await?;
    Ok(())
}
