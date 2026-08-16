//! Scheduled inventory reconciliation runs and transition alerts.

use chrono::Timelike;
use sqlx::Row;
use wareboxes_application::inventory_integrity::{
    InventoryReconciliationAlert, InventoryReconciliationHealth, InventoryReconciliationRunResult,
};
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_domain::{InventoryReconciliationRunId, TenantId, Timestamp};

use crate::db::{begin_tenant_transaction, Db};
use crate::outbox;
use crate::{PersistenceError, PersistenceResult};

pub async fn execute(
    db: &Db,
    tenant_id: TenantId,
    worker_id: &str,
    scheduled_for: Timestamp,
    interval_seconds: i64,
) -> PersistenceResult<InventoryReconciliationRunResult> {
    validate_request(worker_id, scheduled_for, interval_seconds)?;
    let mut tx = begin_tenant_transaction(db, tenant_id).await?;
    let row = sqlx::query(
        r#"
        SELECT * FROM execute_inventory_reconciliation($1,$2,$3,$4)
        "#,
    )
    .bind(tenant_id.get())
    .bind(worker_id)
    .bind(scheduled_for)
    .bind(interval_seconds)
    .fetch_one(&mut *tx)
    .await?;
    let created: bool = row.try_get("created")?;
    let stored_alert = row
        .try_get::<Option<String>, _>("alert_type")?
        .as_deref()
        .map(parse_alert)
        .transpose()?;
    let result = InventoryReconciliationRunResult {
        run_id: InventoryReconciliationRunId::new(row.try_get("run_id")?)
            .map_err(|error| PersistenceError::invalid_data(error.to_string()))?,
        tenant_id: TenantId::new(row.try_get("tenant_id")?)
            .map_err(|error| PersistenceError::invalid_data(error.to_string()))?,
        scheduled_for: row.try_get("scheduled_for")?,
        completed_at: row.try_get("completed_at")?,
        next_due_at: row.try_get("next_due_at")?,
        interval_seconds: row.try_get("interval_seconds")?,
        health: parse_health(&row.try_get::<String, _>("health")?)?,
        previous_health: row
            .try_get::<Option<String>, _>("previous_health")?
            .as_deref()
            .map(parse_health)
            .transpose()?,
        journal_projection_issue_count: row.try_get("journal_projection_issue_count")?,
        commitment_issue_count: row.try_get("commitment_issue_count")?,
        affected_inventory_owner_count: row.try_get("affected_inventory_owner_count")?,
        affected_facility_count: row.try_get("affected_facility_count")?,
        max_severity_quantity: row.try_get("max_severity_quantity")?,
        issue_digest: row.try_get("issue_digest")?,
        state_revision: row.try_get("state_revision")?,
        created,
        alert: if created { stored_alert } else { None },
    };
    if let Some(alert) = result.alert {
        enqueue_alert(&mut tx, &result, alert).await?;
    }
    tx.commit().await?;
    Ok(result)
}

fn validate_request(
    worker_id: &str,
    scheduled_for: Timestamp,
    interval_seconds: i64,
) -> PersistenceResult<()> {
    if worker_id.trim().is_empty() || worker_id.chars().count() > 200 {
        return Err(PersistenceError::invalid_input(
            "inventory reconciliation worker ID must contain between 1 and 200 characters",
        ));
    }
    if scheduled_for.second() != 0 || scheduled_for.nanosecond() != 0 {
        return Err(PersistenceError::invalid_input(
            "inventory reconciliation schedule must be aligned to a UTC minute",
        ));
    }
    if !(60..=86_400).contains(&interval_seconds) {
        return Err(PersistenceError::invalid_input(
            "inventory reconciliation interval must be between 60 and 86400 seconds",
        ));
    }
    Ok(())
}

fn parse_health(value: &str) -> PersistenceResult<InventoryReconciliationHealth> {
    match value {
        "healthy" => Ok(InventoryReconciliationHealth::Healthy),
        "issues_detected" => Ok(InventoryReconciliationHealth::IssuesDetected),
        _ => Err(PersistenceError::invalid_data(format!(
            "database returned invalid inventory reconciliation health: {value}"
        ))),
    }
}

fn parse_alert(value: &str) -> PersistenceResult<InventoryReconciliationAlert> {
    match value {
        "issues_detected" => Ok(InventoryReconciliationAlert::IssuesDetected),
        "issues_changed" => Ok(InventoryReconciliationAlert::IssuesChanged),
        "restored" => Ok(InventoryReconciliationAlert::Restored),
        _ => Err(PersistenceError::invalid_data(format!(
            "database returned invalid inventory reconciliation alert: {value}"
        ))),
    }
}

async fn enqueue_alert(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    result: &InventoryReconciliationRunResult,
    alert: InventoryReconciliationAlert,
) -> PersistenceResult<()> {
    let event_key = format!("inventory-reconciliation-run:{}", result.run_id);
    let aggregate_id = result.tenant_id.to_string();
    let ordering_key = format!("inventory-reconciliation:{}", result.tenant_id);
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "outbox-sequence:{}:{ordering_key}",
            result.tenant_id
        ))
        .execute(&mut **tx)
        .await?;
    let aggregate_sequence: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE((SELECT last_sequence
          FROM outbox_aggregate_sequences
          WHERE tenant_id=$1 AND ordering_key=$2 FOR UPDATE),0)+1
        "#,
    )
    .bind(result.tenant_id.get())
    .bind(&ordering_key)
    .fetch_one(&mut **tx)
    .await?;
    let payload = serde_json::json!({
        "run_id": result.run_id.get(),
        "scheduled_for": result.scheduled_for,
        "completed_at": result.completed_at,
        "next_due_at": result.next_due_at,
        "interval_seconds": result.interval_seconds,
        "health": result.health.as_str(),
        "previous_health": result.previous_health.map(InventoryReconciliationHealth::as_str),
        "journal_projection_issue_count": result.journal_projection_issue_count,
        "commitment_issue_count": result.commitment_issue_count,
        "affected_inventory_owner_count": result.affected_inventory_owner_count,
        "affected_facility_count": result.affected_facility_count,
        "max_severity_quantity": result.max_severity_quantity,
        "issue_digest": &result.issue_digest,
        "state_revision": result.state_revision,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: result.tenant_id,
            inventory_owner_id: None,
            facility_id: None,
            actor_user_id: None,
            event_key: &event_key,
            aggregate_type: "inventory_reconciliation",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type: alert.event_type(),
            schema_version: 1,
            payload: &payload,
            occurred_at: result.completed_at,
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn schedule_and_worker_validation_fail_closed() {
        let minute = Utc.with_ymd_and_hms(2026, 8, 15, 12, 30, 0).unwrap();
        assert!(validate_request("worker-a", minute, 60).is_ok());
        assert!(validate_request("", minute, 60).is_err());
        assert!(validate_request(
            "worker-a",
            Utc.with_ymd_and_hms(2026, 8, 15, 12, 30, 1).unwrap(),
            60
        )
        .is_err());
        assert!(validate_request("worker-a", minute, 59).is_err());
    }
}
