use serde_json::Value;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{DataCellId, DataCellStatus, Timestamp, UserId};

use crate::error::{AppError, AppResult};

pub(super) struct DataCellEvent<'a> {
    pub data_cell_id: DataCellId,
    pub action: &'a str,
    pub revision: i64,
    pub previous_status: Option<DataCellStatus>,
    pub resulting_status: DataCellStatus,
    pub actor_id: UserId,
    pub occurred_at: Timestamp,
    pub reason: Option<&'a str>,
    pub request_id: &'a str,
    pub evidence: &'a Value,
}

pub(super) async fn record_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_access: &TenantAccess,
    event: &DataCellEvent<'_>,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO data_cell_events
        (data_cell_id,action,cell_revision,previous_status,resulting_status,
         actor_user_id,occurred_at,reason,request_id,evidence)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"#,
    )
    .bind(event.data_cell_id.get())
    .bind(event.action)
    .bind(event.revision)
    .bind(event.previous_status.map(DataCellStatus::as_str))
    .bind(event.resulting_status.as_str())
    .bind(event.actor_id.get())
    .bind(event.occurred_at)
    .bind(event.reason)
    .bind(event.request_id)
    .bind(event.evidence)
    .execute(&mut **tx)
    .await?;

    let aggregate_id = event.data_cell_id.get().to_string();
    let ordering_key = format!("data-cell:{}", event.data_cell_id.get());
    let event_key = format!("{ordering_key}:revision:{}", event.revision);
    let aggregate_sequence: i64 = sqlx::query_scalar(
        r#"SELECT COALESCE((SELECT last_sequence FROM outbox_aggregate_sequences
        WHERE tenant_id=$1 AND ordering_key=$2),0)+1"#,
    )
    .bind(actor_access.tenant_id.get())
    .bind(&ordering_key)
    .fetch_one(&mut **tx)
    .await?;
    let event_type = match event.action {
        "registered" => "data_cell.registered.v1",
        "reconfigured" => "data_cell.reconfigured.v1",
        "activated" => "data_cell.activated.v1",
        "draining" => "data_cell.draining.v1",
        "reactivated" => "data_cell.reactivated.v1",
        "retired" => "data_cell.retired.v1",
        _ => return Err(AppError::internal("unknown data-cell event action")),
    };
    wareboxes_persistence_postgres::outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: actor_access.tenant_id,
            inventory_owner_id: None,
            facility_id: None,
            actor_user_id: Some(event.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "data_cell",
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
