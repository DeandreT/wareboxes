use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, Transaction};

use super::{decode_command, EdgeStore, StoreError};
use crate::command::CommandRecord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudDelivery {
    pub cloud_command_id: i64,
    pub cloud_device_id: i64,
    pub delivery_token: String,
    pub delivery_revision: u32,
}

impl CloudDelivery {
    pub(super) fn validate(&self) -> Result<(), StoreError> {
        if self.cloud_command_id <= 0
            || self.cloud_device_id <= 0
            || !(32..=200).contains(&self.delivery_token.len())
            || self.delivery_revision == 0
        {
            return Err(StoreError::InvalidCloudDelivery);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudDeliveryRecord {
    pub delivery: CloudDelivery,
    pub local_command_id: String,
    pub acknowledgement_revision: Option<u32>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub reported_revision: Option<u32>,
    pub reported_status: Option<String>,
    pub reported_at: Option<DateTime<Utc>>,
    pub last_cloud_error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCloudCommand {
    pub cloud: CloudDeliveryRecord,
    pub command: CommandRecord,
}

impl EdgeStore {
    pub fn pending_cloud_acknowledgements(
        &self,
        limit: usize,
    ) -> Result<Vec<PendingCloudCommand>, StoreError> {
        self.pending_cloud_commands("delivery.acknowledged_at_ms IS NULL", limit)
    }

    pub fn pending_cloud_reports(
        &self,
        limit: usize,
    ) -> Result<Vec<PendingCloudCommand>, StoreError> {
        self.pending_cloud_commands(
            r#"delivery.acknowledged_at_ms IS NOT NULL
              AND delivery.reported_at_ms IS NULL
              AND command.state IN
                ('succeeded','failed','manual_review','resolved_manually','cancelled')"#,
            limit,
        )
    }

    fn pending_cloud_commands(
        &self,
        predicate: &str,
        limit: usize,
    ) -> Result<Vec<PendingCloudCommand>, StoreError> {
        if !(1..=1_000).contains(&limit) {
            return Err(StoreError::CorruptRecord(
                "cloud delivery query limit must be between 1 and 1,000".into(),
            ));
        }
        let query = format!(
            r#"SELECT delivery.cloud_command_id,delivery.cloud_device_id,
              delivery.local_command_id,delivery.delivery_token,delivery.delivery_revision,
              delivery.acknowledgement_revision,delivery.acknowledged_at_ms,
              delivery.reported_revision,delivery.reported_status,delivery.reported_at_ms,
              delivery.last_cloud_error,delivery.updated_at_ms,
              command.request_hash,command.request_json,command.state,command.attempt_count,
              command.created_at_ms,command.updated_at_ms,command.next_attempt_at_ms,
              command.result_json,command.last_error,command.resolution_note
            FROM edge_cloud_deliveries delivery
            JOIN edge_commands command ON command.command_id=delivery.local_command_id
            WHERE {predicate}
            ORDER BY delivery.cloud_command_id LIMIT ?1"#,
        );
        let mut statement = self.connection.prepare(&query)?;
        let rows = statement.query_map(
            [i64::try_from(limit).map_err(|_| {
                StoreError::CorruptRecord("cloud delivery query limit overflowed".into())
            })?],
            |row| {
                let cloud = cloud_record_from_row(row)?;
                let command = super::RawCommand {
                    request_hash: row.get(12)?,
                    request_json: row.get(13)?,
                    state: row.get(14)?,
                    attempt_count: row.get(15)?,
                    created_at_ms: row.get(16)?,
                    updated_at_ms: row.get(17)?,
                    next_attempt_at_ms: row.get(18)?,
                    result_json: row.get(19)?,
                    last_error: row.get(20)?,
                    resolution_note: row.get(21)?,
                };
                Ok((cloud, command))
            },
        )?;
        rows.map(|row| {
            let (cloud, command) = row?;
            Ok(PendingCloudCommand {
                cloud,
                command: decode_command(command)?,
            })
        })
        .collect()
    }

    pub fn cloud_delivery(&self, cloud_command_id: i64) -> Result<CloudDeliveryRecord, StoreError> {
        self.connection
            .query_row(
                r#"SELECT cloud_command_id,cloud_device_id,local_command_id,
                delivery_token,delivery_revision,acknowledgement_revision,
                acknowledged_at_ms,reported_revision,reported_status,reported_at_ms,
                last_cloud_error,updated_at_ms
                FROM edge_cloud_deliveries WHERE cloud_command_id=?1"#,
                [cloud_command_id],
                cloud_record_from_row,
            )
            .optional()?
            .ok_or(StoreError::CloudDeliveryNotFound(cloud_command_id))
    }

    pub fn mark_cloud_acknowledged(
        &mut self,
        cloud_command_id: i64,
        delivery_revision: u32,
        acknowledgement_revision: u32,
        now: DateTime<Utc>,
    ) -> Result<CloudDeliveryRecord, StoreError> {
        if delivery_revision == 0 || acknowledgement_revision == 0 {
            return Err(StoreError::InvalidCloudDelivery);
        }
        let tx = self.connection.transaction()?;
        let current = load_cloud_delivery(&tx, cloud_command_id)?
            .ok_or(StoreError::CloudDeliveryNotFound(cloud_command_id))?;
        if current.delivery.delivery_revision != delivery_revision {
            return Err(StoreError::CloudDeliveryConflict);
        }
        if let Some(existing) = current.acknowledgement_revision {
            if existing != acknowledgement_revision {
                return Err(StoreError::CloudDeliveryConflict);
            }
            tx.commit()?;
            return Ok(current);
        }
        tx.execute(
            r#"UPDATE edge_cloud_deliveries SET acknowledgement_revision=?2,
            acknowledged_at_ms=?3,last_cloud_error=NULL,updated_at_ms=?3
            WHERE cloud_command_id=?1 AND acknowledged_at_ms IS NULL"#,
            params![
                cloud_command_id,
                i64::from(acknowledgement_revision),
                now.timestamp_millis()
            ],
        )?;
        let result = load_cloud_delivery(&tx, cloud_command_id)?
            .ok_or(StoreError::CloudDeliveryNotFound(cloud_command_id))?;
        tx.commit()?;
        Ok(result)
    }

    pub fn mark_cloud_reported(
        &mut self,
        cloud_command_id: i64,
        reported_revision: u32,
        reported_status: &str,
        now: DateTime<Utc>,
    ) -> Result<CloudDeliveryRecord, StoreError> {
        if reported_revision == 0
            || !matches!(reported_status, "succeeded" | "failed" | "manual_review")
        {
            return Err(StoreError::InvalidCloudDelivery);
        }
        let tx = self.connection.transaction()?;
        let current = load_cloud_delivery(&tx, cloud_command_id)?
            .ok_or(StoreError::CloudDeliveryNotFound(cloud_command_id))?;
        if current.acknowledgement_revision.is_none() {
            return Err(StoreError::CloudDeliveryConflict);
        }
        if let Some(existing) = current.reported_revision {
            if existing != reported_revision
                || current.reported_status.as_deref() != Some(reported_status)
            {
                return Err(StoreError::CloudDeliveryConflict);
            }
            tx.commit()?;
            return Ok(current);
        }
        tx.execute(
            r#"UPDATE edge_cloud_deliveries SET reported_revision=?2,
            reported_status=?3,reported_at_ms=?4,last_cloud_error=NULL,updated_at_ms=?4
            WHERE cloud_command_id=?1 AND reported_at_ms IS NULL"#,
            params![
                cloud_command_id,
                i64::from(reported_revision),
                reported_status,
                now.timestamp_millis()
            ],
        )?;
        let result = load_cloud_delivery(&tx, cloud_command_id)?
            .ok_or(StoreError::CloudDeliveryNotFound(cloud_command_id))?;
        tx.commit()?;
        Ok(result)
    }

    pub fn record_cloud_error(
        &mut self,
        cloud_command_id: i64,
        message: &str,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let message = super::truncate_message(message);
        let changed = self.connection.execute(
            r#"UPDATE edge_cloud_deliveries SET last_cloud_error=?2,updated_at_ms=?3
            WHERE cloud_command_id=?1"#,
            params![cloud_command_id, message, now.timestamp_millis()],
        )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StoreError::CloudDeliveryNotFound(cloud_command_id))
        }
    }

    pub fn cloud_reportable_count(&self, device_id: &str) -> Result<(u32, u32), StoreError> {
        let (queued, review): (i64, i64) = self.connection.query_row(
            r#"SELECT
              count(*) FILTER (WHERE state IN
                ('queued','executing','retry_wait','recovery_wait')),
              count(*) FILTER (WHERE state='manual_review')
            FROM edge_commands WHERE device_id=?1"#,
            [device_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((
            u32::try_from(queued)
                .map_err(|_| StoreError::CorruptRecord("queued count overflowed".into()))?,
            u32::try_from(review)
                .map_err(|_| StoreError::CorruptRecord("manual-review count overflowed".into()))?,
        ))
    }
}

pub(super) fn upsert_cloud_delivery_tx(
    tx: &Transaction<'_>,
    local_command_id: &str,
    delivery: &CloudDelivery,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    delivery.validate()?;
    if let Some(current) = load_cloud_delivery(tx, delivery.cloud_command_id)? {
        if current.local_command_id != local_command_id
            || current.delivery.cloud_device_id != delivery.cloud_device_id
            || (current.acknowledgement_revision.is_some() && current.delivery != *delivery)
        {
            return Err(StoreError::CloudDeliveryConflict);
        }
        if current.acknowledgement_revision.is_none() {
            tx.execute(
                r#"UPDATE edge_cloud_deliveries SET delivery_token=?2,
                delivery_revision=?3,last_cloud_error=NULL,updated_at_ms=?4
                WHERE cloud_command_id=?1"#,
                params![
                    delivery.cloud_command_id,
                    delivery.delivery_token,
                    i64::from(delivery.delivery_revision),
                    now.timestamp_millis()
                ],
            )?;
        }
        return Ok(());
    }
    tx.execute(
        r#"INSERT INTO edge_cloud_deliveries
        (cloud_command_id,cloud_device_id,local_command_id,delivery_token,
         delivery_revision,updated_at_ms)
        VALUES(?1,?2,?3,?4,?5,?6)"#,
        params![
            delivery.cloud_command_id,
            delivery.cloud_device_id,
            local_command_id,
            delivery.delivery_token,
            i64::from(delivery.delivery_revision),
            now.timestamp_millis()
        ],
    )?;
    Ok(())
}

fn load_cloud_delivery(
    tx: &Transaction<'_>,
    cloud_command_id: i64,
) -> Result<Option<CloudDeliveryRecord>, StoreError> {
    tx.query_row(
        r#"SELECT cloud_command_id,cloud_device_id,local_command_id,
            delivery_token,delivery_revision,acknowledgement_revision,
            acknowledged_at_ms,reported_revision,reported_status,reported_at_ms,
            last_cloud_error,updated_at_ms
            FROM edge_cloud_deliveries WHERE cloud_command_id=?1"#,
        [cloud_command_id],
        cloud_record_from_row,
    )
    .optional()
    .map_err(StoreError::from)
}

fn cloud_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CloudDeliveryRecord> {
    Ok(CloudDeliveryRecord {
        delivery: CloudDelivery {
            cloud_command_id: row.get(0)?,
            cloud_device_id: row.get(1)?,
            delivery_token: row.get(3)?,
            delivery_revision: checked_u32_sql(row.get(4)?)?,
        },
        local_command_id: row.get(2)?,
        acknowledgement_revision: optional_u32_sql(row.get(5)?)?,
        acknowledged_at: row
            .get::<_, Option<i64>>(6)?
            .map(super::timestamp)
            .transpose()
            .map_err(to_sql_error)?,
        reported_revision: optional_u32_sql(row.get(7)?)?,
        reported_status: row.get(8)?,
        reported_at: row
            .get::<_, Option<i64>>(9)?
            .map(super::timestamp)
            .transpose()
            .map_err(to_sql_error)?,
        last_cloud_error: row.get(10)?,
        updated_at: super::timestamp(row.get(11)?).map_err(to_sql_error)?,
    })
}

fn checked_u32_sql(value: i64) -> rusqlite::Result<u32> {
    u32::try_from(value).map_err(|_| {
        to_sql_error(StoreError::CorruptRecord(
            "cloud revision does not fit in u32".into(),
        ))
    })
}

fn optional_u32_sql(value: Option<i64>) -> rusqlite::Result<Option<u32>> {
    value.map(checked_u32_sql).transpose()
}

fn to_sql_error(error: StoreError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Integer, Box::new(error))
}
