mod completion;
mod mapping;

use std::time::Duration;

use anyhow::{bail, Context};
use async_trait::async_trait;
use wareboxes_application::carrier::{CarrierManifestAdapterResponse, CarrierManifestClaim};
use wareboxes_domain::{TenantId, Timestamp};
use wareboxes_worker::{
    CarrierFailureClass, CarrierFailureDisposition, CarrierGatewayError, CarrierManifestStore,
};

use crate::db::{bind_tenant_context, now_iso, Db};

#[derive(Clone)]
pub struct PostgresCarrierManifestStore {
    db: Db,
}

impl PostgresCarrierManifestStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl CarrierManifestStore for PostgresCarrierManifestStore {
    async fn ready_tenants(
        &self,
        after: Option<TenantId>,
        limit: usize,
    ) -> anyhow::Result<Vec<TenantId>> {
        if !(1..=10_000).contains(&limit) {
            bail!("carrier tenant page limit must be between 1 and 10000");
        }
        let rows: Vec<i64> = sqlx::query_scalar(
            r#"SELECT id FROM tenants WHERE deleted IS NULL AND status='active' AND id>$1
               ORDER BY id LIMIT $2"#,
        )
        .bind(after.map_or(0, TenantId::get))
        .bind(i64::try_from(limit).context("carrier tenant page limit exceeds i64")?)
        .fetch_all(&self.db)
        .await?;
        rows.into_iter()
            .map(|id| TenantId::new(id).context("database returned invalid tenant ID"))
            .collect()
    }

    async fn claim(
        &self,
        tenant_id: TenantId,
        worker_id: &str,
        batch_size: i64,
        lease: Duration,
    ) -> anyhow::Result<Vec<CarrierManifestClaim>> {
        let lease_seconds = duration_seconds(lease, "carrier lease")?;
        let mut tx = self.db.begin().await?;
        bind_tenant_context(&mut tx, tenant_id).await?;
        bind_worker(&mut tx, worker_id).await?;
        let now = now_iso();
        let recovered = sqlx::query_as::<_, (i64, i32, i32)>(
            r#"INSERT INTO carrier_manifest_attempt_results
               (tenant_id,carrier_manifest_job_id,attempt_number,claim_version,outcome,
                error_code,error_message,recorded_by_worker_id,completed_at)
               SELECT job.tenant_id,job.id,attempt.attempt_number,attempt.claim_version,
                 'claim_lost','lease_expired','carrier worker claim lease expired',$3,$2
               FROM carrier_manifest_jobs job
               JOIN carrier_manifest_attempts attempt
                 ON attempt.tenant_id=job.tenant_id
                AND attempt.carrier_manifest_job_id=job.id
                AND attempt.attempt_number=job.attempt_count
                AND attempt.claim_version=job.revision
               WHERE job.tenant_id=$1 AND job.status='processing'
                 AND job.lease_expires_at<$2
               ON CONFLICT(tenant_id,carrier_manifest_job_id,attempt_number) DO NOTHING
               RETURNING carrier_manifest_job_id,attempt_number,claim_version"#,
        )
        .bind(tenant_id.get())
        .bind(now)
        .bind(worker_id)
        .fetch_all(&mut *tx)
        .await?;
        for (job_id, attempt_number, claim_version) in recovered {
            completion::enqueue_claim_lost_event(
                &mut tx,
                tenant_id,
                job_id,
                attempt_number,
                claim_version,
                worker_id,
                now,
            )
            .await?;
        }
        let rows = sqlx::query(&format!(
            r#"WITH candidates AS (
                 SELECT id FROM carrier_manifest_jobs
                 WHERE tenant_id=$1 AND (
                   status='queued'
                   OR (status='retry_scheduled' AND next_attempt_at<=$2)
                   OR (status='processing' AND lease_expires_at<$2))
                 ORDER BY id FOR UPDATE SKIP LOCKED LIMIT $3
               )
               UPDATE carrier_manifest_jobs AS job
               SET status='processing',revision=job.revision+1,
                   attempt_count=job.attempt_count+1,claimed_by=$4,claimed_at=$2,
                   lease_expires_at=$2+make_interval(secs=>$5),next_attempt_at=NULL,
                   completed_at=NULL
               FROM candidates WHERE job.tenant_id=$1 AND job.id=candidates.id
               RETURNING {}"#,
            mapping::CLAIM_COLUMNS
        ))
        .bind(tenant_id.get())
        .bind(now)
        .bind(batch_size)
        .bind(worker_id)
        .bind(lease_seconds)
        .fetch_all(&mut *tx)
        .await?;
        let mut claims = Vec::with_capacity(rows.len());
        for row in rows {
            let claim = mapping::claim(&row)?;
            sqlx::query(
                r#"INSERT INTO carrier_manifest_attempts
                   (tenant_id,inventory_owner_id,facility_id,shipment_id,
                    carrier_manifest_job_id,attempt_number,claim_version,worker_id,
                    request_sha256,claimed_at,lease_expires_at)
                   SELECT tenant_id,inventory_owner_id,facility_id,shipment_id,id,
                     attempt_count,revision,claimed_by,request_sha256,claimed_at,lease_expires_at
                   FROM carrier_manifest_jobs WHERE tenant_id=$1 AND id=$2"#,
            )
            .bind(tenant_id.get())
            .bind(claim.job.job_id.get())
            .execute(&mut *tx)
            .await?;
            claims.push(claim);
        }
        tx.commit().await?;
        Ok(claims)
    }

    async fn complete(
        &self,
        claim: &CarrierManifestClaim,
        worker_id: &str,
        response: &CarrierManifestAdapterResponse,
    ) -> anyhow::Result<bool> {
        completion::complete(&self.db, claim, worker_id, response).await
    }

    async fn fail(
        &self,
        claim: &CarrierManifestClaim,
        worker_id: &str,
        error: &CarrierGatewayError,
        retry_after: Duration,
        max_attempts: u32,
    ) -> anyhow::Result<CarrierFailureDisposition> {
        let mut tx = self.db.begin().await?;
        bind_tenant_context(&mut tx, claim.job.tenant_id).await?;
        bind_worker(&mut tx, worker_id).await?;
        let now = now_iso();
        let retry_after_seconds = duration_seconds(retry_after, "carrier retry delay")?;
        let retry = error.class() == CarrierFailureClass::Retryable
            && claim.job.attempt_count < max_attempts;
        let result = if retry {
            sqlx::query_scalar::<_, i64>(
                r#"UPDATE carrier_manifest_jobs
                   SET status='retry_scheduled',revision=revision+1,claimed_by=NULL,
                       claimed_at=NULL,lease_expires_at=NULL,
                       next_attempt_at=$5+make_interval(secs=>$6),
                       last_error_code=$7,last_error_message=$8,completed_at=NULL
                   WHERE tenant_id=$1 AND id=$2 AND status='processing'
                     AND revision=$3 AND claimed_by=$4 AND lease_expires_at>=CURRENT_TIMESTAMP
                   RETURNING id"#,
            )
            .bind(claim.job.tenant_id.get())
            .bind(claim.job.job_id.get())
            .bind(i32::try_from(claim.claim_version)?)
            .bind(worker_id)
            .bind(now)
            .bind(retry_after_seconds)
            .bind(error.code())
            .bind(error.message())
            .fetch_optional(&mut *tx)
            .await?
        } else {
            sqlx::query_scalar::<_, i64>(
                r#"UPDATE carrier_manifest_jobs
                   SET status='failed',revision=revision+1,claimed_by=NULL,claimed_at=NULL,
                       lease_expires_at=NULL,next_attempt_at=NULL,last_error_code=$5,
                       last_error_message=$6,completed_at=$7
                   WHERE tenant_id=$1 AND id=$2 AND status='processing'
                     AND revision=$3 AND claimed_by=$4 AND lease_expires_at>=CURRENT_TIMESTAMP
                   RETURNING id"#,
            )
            .bind(claim.job.tenant_id.get())
            .bind(claim.job.job_id.get())
            .bind(i32::try_from(claim.claim_version)?)
            .bind(worker_id)
            .bind(error.code())
            .bind(error.message())
            .bind(now)
            .fetch_optional(&mut *tx)
            .await?
        };
        let Some(_) = result else {
            if insert_claim_lost(&mut tx, claim, worker_id, error, now).await? {
                completion::enqueue_claim_lost_event(
                    &mut tx,
                    claim.job.tenant_id,
                    claim.job.job_id.get(),
                    i32::try_from(claim.job.attempt_count)?,
                    i32::try_from(claim.claim_version)?,
                    worker_id,
                    now,
                )
                .await?;
            }
            tx.commit().await?;
            return Ok(CarrierFailureDisposition::LostClaim);
        };
        sqlx::query(
            r#"INSERT INTO carrier_manifest_attempt_results
               (tenant_id,carrier_manifest_job_id,attempt_number,claim_version,outcome,
                response_sha256,error_code,error_message,retry_after_seconds,
                recorded_by_worker_id,completed_at)
               VALUES($1,$2,$3,$4,$5,NULL,$6,$7,$8,$9,$10)"#,
        )
        .bind(claim.job.tenant_id.get())
        .bind(claim.job.job_id.get())
        .bind(i32::try_from(claim.job.attempt_count)?)
        .bind(i32::try_from(claim.claim_version)?)
        .bind(if retry { "retry_scheduled" } else { "failed" })
        .bind(error.code())
        .bind(error.message())
        .bind(retry.then_some(retry_after_seconds))
        .bind(worker_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        completion::enqueue_failure_event(
            &mut tx,
            claim,
            error,
            retry,
            retry.then_some(retry_after_seconds),
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(if retry {
            CarrierFailureDisposition::RetryScheduled
        } else {
            CarrierFailureDisposition::Failed
        })
    }
}

pub(super) async fn bind_worker(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    worker_id: &str,
) -> anyhow::Result<()> {
    sqlx::query("SELECT set_config('wareboxes.carrier_worker_id',$1,true)")
        .bind(worker_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn insert_claim_lost(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &CarrierManifestClaim,
    worker_id: &str,
    error: &CarrierGatewayError,
    completed_at: Timestamp,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"INSERT INTO carrier_manifest_attempt_results
           (tenant_id,carrier_manifest_job_id,attempt_number,claim_version,outcome,
            error_code,error_message,recorded_by_worker_id,completed_at)
           VALUES($1,$2,$3,$4,'claim_lost',$5,$6,$7,$8)
           ON CONFLICT(tenant_id,carrier_manifest_job_id,attempt_number) DO NOTHING"#,
    )
    .bind(claim.job.tenant_id.get())
    .bind(claim.job.job_id.get())
    .bind(i32::try_from(claim.job.attempt_count)?)
    .bind(i32::try_from(claim.claim_version)?)
    .bind(error.code())
    .bind(error.message())
    .bind(worker_id)
    .bind(completed_at)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub(super) async fn record_claim_lost(
    db: &Db,
    claim: &CarrierManifestClaim,
    worker_id: &str,
) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, claim.job.tenant_id).await?;
    bind_worker(&mut tx, worker_id).await?;
    let occurred_at = now_iso();
    let error = CarrierGatewayError::retryable(
        "claim_lost",
        "carrier manifest claim changed or expired before completion",
    );
    if insert_claim_lost(&mut tx, claim, worker_id, &error, occurred_at).await? {
        completion::enqueue_claim_lost_event(
            &mut tx,
            claim.job.tenant_id,
            claim.job.job_id.get(),
            i32::try_from(claim.job.attempt_count)?,
            i32::try_from(claim.claim_version)?,
            worker_id,
            occurred_at,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

fn duration_seconds(duration: Duration, label: &str) -> anyhow::Result<i64> {
    let seconds = duration
        .as_secs()
        .checked_add(u64::from(duration.subsec_nanos() != 0))
        .with_context(|| format!("{label} does not fit in whole seconds"))?;
    i64::try_from(seconds).with_context(|| format!("{label} does not fit in i64"))
}
