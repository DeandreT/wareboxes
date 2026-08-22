use serde_json::Value;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_domain::{TenantCellMoveId, TenantCellMoveStatus, TenantId, Timestamp, UserId};

use crate::error::{AppError, AppResult};

pub(super) struct TenantCellMoveEvent<'a> {
    pub tenant_cell_move_id: TenantCellMoveId,
    pub tenant_id: TenantId,
    pub action: &'a str,
    pub revision: i64,
    pub previous_status: Option<TenantCellMoveStatus>,
    pub resulting_status: TenantCellMoveStatus,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
    pub reason: Option<&'a str>,
    pub request_id: &'a str,
    pub evidence: &'a Value,
}

pub(super) async fn record_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: &TenantCellMoveEvent<'_>,
) -> AppResult<()> {
    super::super::tenant_lifecycle::bind_platform_tenant_tx(tx, event.tenant_id).await?;
    sqlx::query(
        r#"INSERT INTO tenant_cell_move_events
        (tenant_id,tenant_cell_move_id,action,move_revision,previous_status,
         resulting_status,actor_user_id,occurred_at,reason,request_id,evidence)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)"#,
    )
    .bind(event.tenant_id.get())
    .bind(event.tenant_cell_move_id.get())
    .bind(event.action)
    .bind(event.revision)
    .bind(event.previous_status.map(TenantCellMoveStatus::as_str))
    .bind(event.resulting_status.as_str())
    .bind(event.actor_id.get())
    .bind(event.occurred_at)
    .bind(event.reason)
    .bind(event.request_id)
    .bind(event.evidence)
    .execute(&mut **tx)
    .await?;

    let aggregate_id = event.tenant_cell_move_id.get().to_string();
    let ordering_key = format!("tenant-cell-move:{}", event.tenant_cell_move_id.get());
    let event_key = format!("{ordering_key}:revision:{}", event.revision);
    let aggregate_sequence: i64 = sqlx::query_scalar(
        r#"SELECT COALESCE((SELECT last_sequence FROM outbox_aggregate_sequences
        WHERE tenant_id=$1 AND ordering_key=$2),0)+1"#,
    )
    .bind(event.tenant_id.get())
    .bind(&ordering_key)
    .fetch_one(&mut **tx)
    .await?;
    let event_type = match event.action {
        "planned" => "tenant_cell_move.planned.v1",
        "copy_started" => "tenant_cell_move.copy_started.v1",
        "checkpoint_recorded" => "tenant_cell_move.checkpoint_recorded.v1",
        "writes_frozen" => "tenant_cell_move.writes_frozen.v1",
        "validated" => "tenant_cell_move.validated.v1",
        "cut_over" => "tenant_cell_move.cut_over.v1",
        "post_cutover_verified" => "tenant_cell_move.post_cutover_verified.v1",
        "completed" => "tenant_cell_move.completed.v1",
        "rolled_back" => "tenant_cell_move.rolled_back.v1",
        "cancelled" => "tenant_cell_move.cancelled.v1",
        _ => return Err(AppError::internal("unknown tenant-cell-move event action")),
    };
    wareboxes_persistence_postgres::outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: event.tenant_id,
            inventory_owner_id: None,
            facility_id: None,
            actor_user_id: Some(event.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "tenant_cell_move",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type,
            schema_version: 1,
            payload: event.evidence,
            occurred_at: event.occurred_at,
        },
    )
    .await?;
    Ok(())
}
