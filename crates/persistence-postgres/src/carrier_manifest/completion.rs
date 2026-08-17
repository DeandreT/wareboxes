use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::carrier::{
    validate_carrier_response, CarrierManifestAdapterResponse, CarrierManifestClaim,
};
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_domain::{CarrierManifestId, CartonId, TenantId, Timestamp};
use wareboxes_worker::CarrierGatewayError;

use super::bind_worker;
use crate::db::{bind_tenant_context, now_iso, Db};
use crate::outbox;

struct LockedJob {
    inventory_owner_id: i64,
    facility_id: i64,
    shipment_id: i64,
    expected_shipment_revision: i64,
    carrier_code: String,
    service_code: Option<String>,
    requested_by_user_id: i64,
}

struct LockedShipment {
    packing_session_id: i64,
    order_release_id: i64,
    order_id: i64,
    revision: i64,
    carton_count: i64,
}

struct ShipmentCarton {
    shipment_carton_id: i64,
    carton_id: CartonId,
    license_plate_id: i64,
    sequence: i64,
    weight_g: Option<i64>,
    length_mm: Option<i64>,
    width_mm: Option<i64>,
    height_mm: Option<i64>,
}

pub async fn complete(
    db: &Db,
    claim: &CarrierManifestClaim,
    worker_id: &str,
    response: &CarrierManifestAdapterResponse,
) -> anyhow::Result<bool> {
    validate_carrier_response(&claim.request, response)?;
    let mut response = response.clone();
    response
        .packages
        .sort_unstable_by_key(|package| package.carton_id.get());
    let response_payload = serde_json::to_value(&response)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, claim.job.tenant_id).await?;
    bind_worker(&mut tx, worker_id).await?;
    sqlx::query("SELECT set_config('wareboxes.carrier_manifest_job_id',$1,true)")
        .bind(claim.job.job_id.get().to_string())
        .execute(&mut *tx)
        .await?;
    let Some(job) = lock_job(&mut tx, claim, worker_id).await? else {
        tx.rollback().await?;
        super::record_claim_lost(db, claim, worker_id).await?;
        return Ok(false);
    };
    let order_id: i64 = sqlx::query_scalar(
        r#"SELECT order_id FROM shipments WHERE tenant_id=$1 AND inventory_owner_id=$2
           AND facility_id=$3 AND id=$4"#,
    )
    .bind(claim.job.tenant_id.get())
    .bind(job.inventory_owner_id)
    .bind(job.facility_id)
    .bind(job.shipment_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("carrier manifest shipment no longer exists"))?;
    let order_status: Option<String> = sqlx::query_scalar(
        r#"SELECT status FROM orders WHERE tenant_id=$1 AND inventory_owner_id=$2
           AND id=$3 AND deleted IS NULL FOR UPDATE"#,
    )
    .bind(claim.job.tenant_id.get())
    .bind(job.inventory_owner_id)
    .bind(order_id)
    .fetch_optional(&mut *tx)
    .await?;
    if order_status.as_deref() != Some("awaiting shipment") {
        anyhow::bail!("carrier manifest order is no longer awaiting shipment");
    }
    let shipment = lock_shipment(&mut tx, claim.job.tenant_id, &job).await?;
    if shipment.order_id != order_id {
        anyhow::bail!("carrier manifest shipment order identity changed");
    }
    let cartons = lock_cartons(&mut tx, claim.job.tenant_id, job.shipment_id).await?;
    if i64::try_from(cartons.len())? != shipment.carton_count {
        anyhow::bail!("carrier manifest carton snapshot is incomplete");
    }
    lock_manifest_keys(&mut tx, claim.job.tenant_id, &job, &response).await?;
    require_manifest_keys_available(&mut tx, claim.job.tenant_id, &job, &response).await?;
    let manifested_at = now_iso();
    let next_revision = shipment
        .revision
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("shipment revision overflow"))?;
    let updated = sqlx::query(
        r#"UPDATE shipments SET state='manifested',revision=$3,carrier=$4,service=$5,
             manifested_at=$6
           WHERE tenant_id=$1 AND id=$2 AND state='awaiting manifest' AND revision=$7"#,
    )
    .bind(claim.job.tenant_id.get())
    .bind(job.shipment_id)
    .bind(next_revision)
    .bind(&job.carrier_code)
    .bind(&job.service_code)
    .bind(manifested_at)
    .bind(shipment.revision)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        anyhow::bail!("shipment changed while committing carrier manifest");
    }
    let manifest_id_raw: i64 = sqlx::query_scalar(
        r#"INSERT INTO shipment_manifests
           (tenant_id,inventory_owner_id,facility_id,shipment_id,packing_session_id,
            order_release_id,order_id,manifest_number,carrier,service,expected_revision,
            resulting_revision,package_count,manifested_by_user_id,manifested_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
           RETURNING id"#,
    )
    .bind(claim.job.tenant_id.get())
    .bind(job.inventory_owner_id)
    .bind(job.facility_id)
    .bind(job.shipment_id)
    .bind(shipment.packing_session_id)
    .bind(shipment.order_release_id)
    .bind(shipment.order_id)
    .bind(response.manifest_reference.as_str())
    .bind(&job.carrier_code)
    .bind(&job.service_code)
    .bind(shipment.revision)
    .bind(next_revision)
    .bind(shipment.carton_count)
    .bind(job.requested_by_user_id)
    .bind(manifested_at)
    .fetch_one(&mut *tx)
    .await?;
    let manifest_id = CarrierManifestId::new(manifest_id_raw)?;
    insert_packages(
        &mut tx,
        claim.job.tenant_id,
        &job,
        manifest_id,
        &cartons,
        &response,
        manifested_at,
    )
    .await?;
    sqlx::query(
        r#"INSERT INTO order_activity
           (tenant_id,inventory_owner_id,created,order_id,actor_user_id,action)
           VALUES($1,$2,$3,$4,$5,$6)"#,
    )
    .bind(claim.job.tenant_id.get())
    .bind(job.inventory_owner_id)
    .bind(manifested_at)
    .bind(shipment.order_id)
    .bind(job.requested_by_user_id)
    .bind(format!(
        "manifested shipment {} with {} via carrier gateway {}",
        job.shipment_id, response.manifest_reference, job.carrier_code
    ))
    .execute(&mut *tx)
    .await?;
    let completed = sqlx::query_scalar::<_, i64>(
        r#"UPDATE carrier_manifest_jobs
           SET status='succeeded',revision=revision+1,claimed_by=NULL,claimed_at=NULL,
               lease_expires_at=NULL,next_attempt_at=NULL,last_error_code=NULL,
               last_error_message=NULL,response_payload=$5,
               response_sha256=sha256(convert_to($5::jsonb::text,'UTF8')),
               carrier_manifest_id=$6,manifest_reference=$7,completed_at=$8
           WHERE tenant_id=$1 AND id=$2 AND status='processing' AND revision=$3
             AND claimed_by=$4 AND lease_expires_at>=CURRENT_TIMESTAMP RETURNING id"#,
    )
    .bind(claim.job.tenant_id.get())
    .bind(claim.job.job_id.get())
    .bind(i32::try_from(claim.claim_version)?)
    .bind(worker_id)
    .bind(&response_payload)
    .bind(manifest_id.get())
    .bind(response.manifest_reference.as_str())
    .bind(manifested_at)
    .fetch_optional(&mut *tx)
    .await?;
    if completed.is_none() {
        tx.rollback().await?;
        super::record_claim_lost(db, claim, worker_id).await?;
        return Ok(false);
    }
    sqlx::query(
        r#"INSERT INTO carrier_manifest_attempt_results
           (tenant_id,carrier_manifest_job_id,attempt_number,claim_version,outcome,
            response_sha256,recorded_by_worker_id,completed_at)
           VALUES($1,$2,$3,$4,'succeeded',sha256(convert_to($5::jsonb::text,'UTF8')),$6,$7)"#,
    )
    .bind(claim.job.tenant_id.get())
    .bind(claim.job.job_id.get())
    .bind(i32::try_from(claim.job.attempt_count)?)
    .bind(i32::try_from(claim.claim_version)?)
    .bind(&response_payload)
    .bind(worker_id)
    .bind(manifested_at)
    .execute(&mut *tx)
    .await?;
    enqueue_shipping_event(
        &mut tx,
        claim,
        &job,
        &shipment,
        manifest_id,
        &response,
        next_revision,
        manifested_at,
    )
    .await?;
    enqueue_job_event(&mut tx, claim, &job, manifest_id, &response, manifested_at).await?;
    tx.commit().await?;
    Ok(true)
}

async fn lock_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &CarrierManifestClaim,
    worker_id: &str,
) -> anyhow::Result<Option<LockedJob>> {
    let row = sqlx::query(
        r#"SELECT inventory_owner_id,facility_id,shipment_id,expected_shipment_revision,
                  carrier_code,service_code,requested_by_user_id
           FROM carrier_manifest_jobs
           WHERE tenant_id=$1 AND id=$2 AND status='processing' AND revision=$3
             AND claimed_by=$4 AND lease_expires_at>=CURRENT_TIMESTAMP FOR UPDATE"#,
    )
    .bind(claim.job.tenant_id.get())
    .bind(claim.job.job_id.get())
    .bind(i32::try_from(claim.claim_version)?)
    .bind(worker_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(LockedJob {
            inventory_owner_id: row.try_get("inventory_owner_id")?,
            facility_id: row.try_get("facility_id")?,
            shipment_id: row.try_get("shipment_id")?,
            expected_shipment_revision: row.try_get("expected_shipment_revision")?,
            carrier_code: row.try_get("carrier_code")?,
            service_code: row.try_get("service_code")?,
            requested_by_user_id: row.try_get("requested_by_user_id")?,
        })
    })
    .transpose()
}

async fn lock_shipment(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    job: &LockedJob,
) -> anyhow::Result<LockedShipment> {
    let row = sqlx::query(
        r#"SELECT packing_session_id,order_release_id,order_id,revision,carton_count,state
           FROM shipments WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
             AND id=$4 FOR UPDATE"#,
    )
    .bind(tenant_id.get())
    .bind(job.inventory_owner_id)
    .bind(job.facility_id)
    .bind(job.shipment_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("carrier manifest shipment no longer exists"))?;
    if row.try_get::<String, _>("state")? != "awaiting manifest"
        || row.try_get::<i64, _>("revision")? != job.expected_shipment_revision
    {
        anyhow::bail!("carrier manifest shipment changed before completion");
    }
    Ok(LockedShipment {
        packing_session_id: row.try_get("packing_session_id")?,
        order_release_id: row.try_get("order_release_id")?,
        order_id: row.try_get("order_id")?,
        revision: row.try_get("revision")?,
        carton_count: row.try_get("carton_count")?,
    })
}

async fn lock_cartons(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: i64,
) -> anyhow::Result<Vec<ShipmentCarton>> {
    let rows = sqlx::query(
        r#"SELECT id,carton_id,license_plate_id,sequence,weight_g,length_mm,width_mm,height_mm
           FROM shipment_cartons WHERE tenant_id=$1 AND shipment_id=$2
           ORDER BY carton_id"#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ShipmentCarton {
                shipment_carton_id: row.try_get("id")?,
                carton_id: CartonId::new(row.try_get("carton_id")?)?,
                license_plate_id: row.try_get("license_plate_id")?,
                sequence: row.try_get("sequence")?,
                weight_g: row.try_get("weight_g")?,
                length_mm: row.try_get("length_mm")?,
                width_mm: row.try_get("width_mm")?,
                height_mm: row.try_get("height_mm")?,
            })
        })
        .collect()
}

async fn insert_packages(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    job: &LockedJob,
    manifest_id: CarrierManifestId,
    cartons: &[ShipmentCarton],
    response: &CarrierManifestAdapterResponse,
    created_at: Timestamp,
) -> anyhow::Result<()> {
    let assignments = response
        .packages
        .iter()
        .map(|package| (package.carton_id, &package.tracking_number))
        .collect::<HashMap<_, _>>();
    for carton in cartons {
        let tracking = assignments
            .get(&carton.carton_id)
            .ok_or_else(|| anyhow::anyhow!("carrier response omitted a carton"))?;
        sqlx::query(
            r#"INSERT INTO shipment_manifest_packages
               (tenant_id,inventory_owner_id,facility_id,shipment_id,manifest_id,
                shipment_carton_id,carton_id,license_plate_id,sequence,carrier,service,
                tracking_number,weight_g,length_mm,width_mm,height_mm,created_at)
               VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)"#,
        )
        .bind(tenant_id.get())
        .bind(job.inventory_owner_id)
        .bind(job.facility_id)
        .bind(job.shipment_id)
        .bind(manifest_id.get())
        .bind(carton.shipment_carton_id)
        .bind(carton.carton_id.get())
        .bind(carton.license_plate_id)
        .bind(carton.sequence)
        .bind(&job.carrier_code)
        .bind(&job.service_code)
        .bind(tracking.as_str())
        .bind(carton.weight_g)
        .bind(carton.length_mm)
        .bind(carton.width_mm)
        .bind(carton.height_mm)
        .bind(created_at)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn lock_manifest_keys(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    job: &LockedJob,
    response: &CarrierManifestAdapterResponse,
) -> anyhow::Result<()> {
    let mut keys = response
        .packages
        .iter()
        .map(|package| {
            format!(
                "shipment-tracking:{tenant_id}:{}:{}",
                job.carrier_code, package.tracking_number
            )
        })
        .collect::<Vec<_>>();
    keys.push(format!(
        "shipment-manifest:{tenant_id}:{}:{}",
        job.carrier_code, response.manifest_reference
    ));
    keys.sort_unstable();
    keys.dedup();
    for key in keys {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(key)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn require_manifest_keys_available(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    job: &LockedJob,
    response: &CarrierManifestAdapterResponse,
) -> anyhow::Result<()> {
    let manifest_exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM shipment_manifests
           WHERE tenant_id=$1 AND carrier=$2 AND manifest_number=$3)"#,
    )
    .bind(tenant_id.get())
    .bind(&job.carrier_code)
    .bind(response.manifest_reference.as_str())
    .fetch_one(&mut **tx)
    .await?;
    let tracking = response
        .packages
        .iter()
        .map(|package| package.tracking_number.as_str())
        .collect::<Vec<_>>();
    let tracking_exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM shipment_manifest_packages
           WHERE tenant_id=$1 AND carrier=$2 AND tracking_number=ANY($3))"#,
    )
    .bind(tenant_id.get())
    .bind(&job.carrier_code)
    .bind(&tracking)
    .fetch_one(&mut **tx)
    .await?;
    if manifest_exists || tracking_exists {
        anyhow::bail!("carrier gateway returned a manifest or tracking identity already in use");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_shipping_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &CarrierManifestClaim,
    job: &LockedJob,
    shipment: &LockedShipment,
    manifest_id: CarrierManifestId,
    response: &CarrierManifestAdapterResponse,
    revision: i64,
    occurred_at: Timestamp,
) -> anyhow::Result<()> {
    let payload = serde_json::json!({
        "shipment_id": job.shipment_id,
        "order_id": shipment.order_id,
        "manifest_id": manifest_id,
        "manifest_reference": response.manifest_reference,
        "carrier_code": job.carrier_code,
        "service_code": job.service_code,
        "package_count": shipment.carton_count,
        "expected_revision": shipment.revision,
        "revision": revision,
        "manifested_at": occurred_at,
        "source": "carrier_gateway",
        "carrier_manifest_job_id": claim.job.job_id,
        "carrier_account_id": claim.job.account_id,
        "carrier_account_revision": claim.job.account_revision,
        "request_key": claim.job.request_key,
        "request_sha256": claim.job.request_sha256,
    });
    enqueue_event(
        tx,
        claim.job.tenant_id,
        Some(job.inventory_owner_id),
        Some(job.facility_id),
        Some(job.requested_by_user_id),
        "order",
        &shipment.order_id.to_string(),
        &format!("order:{}", shipment.order_id),
        "shipping.shipment_manifested",
        &format!("shipment:{}:manifested", job.shipment_id),
        &payload,
        occurred_at,
    )
    .await
}

async fn enqueue_job_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &CarrierManifestClaim,
    job: &LockedJob,
    manifest_id: CarrierManifestId,
    response: &CarrierManifestAdapterResponse,
    occurred_at: Timestamp,
) -> anyhow::Result<()> {
    let payload = serde_json::json!({
        "carrier_manifest_job_id": claim.job.job_id,
        "shipment_id": claim.job.shipment_id,
        "carrier_account_id": claim.job.account_id,
        "carrier_account_revision": claim.job.account_revision,
        "request_key": claim.job.request_key,
        "request_sha256": claim.job.request_sha256,
        "manifest_id": manifest_id,
        "manifest_reference": response.manifest_reference,
        "tracking": response.packages,
        "attempt_count": claim.job.attempt_count,
        "completed_at": occurred_at,
    });
    enqueue_event(
        tx,
        claim.job.tenant_id,
        Some(job.inventory_owner_id),
        Some(job.facility_id),
        Some(job.requested_by_user_id),
        "carrier_manifest_job",
        &claim.job.job_id.get().to_string(),
        &format!("carrier_manifest_job:{}", claim.job.job_id.get()),
        "carrier.manifest.succeeded",
        &format!(
            "carrier-manifest-job:{}:{}:succeeded",
            claim.job.job_id.get(),
            claim.claim_version + 1
        ),
        &payload,
        occurred_at,
    )
    .await
}

pub(super) async fn enqueue_failure_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &CarrierManifestClaim,
    error: &CarrierGatewayError,
    retry_scheduled: bool,
    retry_after_seconds: Option<i64>,
    occurred_at: Timestamp,
) -> anyhow::Result<()> {
    let resulting_revision = claim
        .claim_version
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("carrier job revision overflow"))?;
    let outcome = if retry_scheduled {
        "retry_scheduled"
    } else {
        "failed"
    };
    let payload = serde_json::json!({
        "carrier_manifest_job_id": claim.job.job_id,
        "shipment_id": claim.job.shipment_id,
        "carrier_account_id": claim.job.account_id,
        "carrier_account_revision": claim.job.account_revision,
        "request_key": claim.job.request_key,
        "request_sha256": claim.job.request_sha256,
        "attempt_count": claim.job.attempt_count,
        "error_code": error.code(),
        "error_message": error.message(),
        "retry_after_seconds": retry_after_seconds,
        "resulting_revision": resulting_revision,
        "completed_at": occurred_at,
    });
    enqueue_event(
        tx,
        claim.job.tenant_id,
        Some(claim.job.inventory_owner_id.get()),
        Some(claim.job.facility_id.get()),
        Some(claim.job.requested_by.get()),
        "carrier_manifest_job",
        &claim.job.job_id.get().to_string(),
        &format!("carrier_manifest_job:{}", claim.job.job_id.get()),
        &format!("carrier.manifest.{outcome}"),
        &format!(
            "carrier-manifest-job:{}:{}:{outcome}",
            claim.job.job_id.get(),
            resulting_revision
        ),
        &payload,
        occurred_at,
    )
    .await
}

pub(super) async fn enqueue_claim_lost_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    job_id: i64,
    attempt_number: i32,
    claim_version: i32,
    recovered_by_worker_id: &str,
    occurred_at: Timestamp,
) -> anyhow::Result<()> {
    let row = sqlx::query(
        r#"SELECT inventory_owner_id,facility_id,shipment_id
           FROM carrier_manifest_jobs WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(tenant_id.get())
    .bind(job_id)
    .fetch_one(&mut **tx)
    .await?;
    let inventory_owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    let shipment_id: i64 = row.try_get("shipment_id")?;
    let payload = serde_json::json!({
        "carrier_manifest_job_id": job_id,
        "shipment_id": shipment_id,
        "attempt_number": attempt_number,
        "claim_version": claim_version,
        "reason": "lease_expired",
        "recovered_by_worker_id": recovered_by_worker_id,
        "recorded_at": occurred_at,
    });
    enqueue_event(
        tx,
        tenant_id,
        Some(inventory_owner_id),
        Some(facility_id),
        None,
        "carrier_manifest_job",
        &job_id.to_string(),
        &format!("carrier_manifest_job:{job_id}"),
        "carrier.manifest.claim_lost",
        &format!("carrier-manifest-job:{job_id}:{claim_version}:claim-lost"),
        &payload,
        occurred_at,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: Option<i64>,
    facility_id: Option<i64>,
    actor_user_id: Option<i64>,
    aggregate_type: &str,
    aggregate_id: &str,
    ordering_key: &str,
    event_type: &str,
    event_key: &str,
    payload: &serde_json::Value,
    occurred_at: Timestamp,
) -> anyhow::Result<()> {
    let sequence = next_sequence(tx, tenant_id, ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: inventory_owner_id
                .map(wareboxes_domain::InventoryOwnerId::new)
                .transpose()?,
            facility_id: facility_id
                .map(wareboxes_domain::FacilityId::new)
                .transpose()?,
            actor_user_id,
            event_key,
            aggregate_type,
            aggregate_id,
            ordering_key,
            aggregate_sequence: sequence,
            event_type,
            schema_version: 1,
            payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

async fn next_sequence(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    ordering_key: &str,
) -> anyhow::Result<i64> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("outbox-sequence:{tenant_id}:{ordering_key}"))
        .execute(&mut **tx)
        .await?;
    Ok(sqlx::query_scalar(
        r#"SELECT COALESCE((SELECT last_sequence FROM outbox_aggregate_sequences
           WHERE tenant_id=$1 AND ordering_key=$2),0)+1"#,
    )
    .bind(tenant_id.get())
    .bind(ordering_key)
    .fetch_one(&mut **tx)
    .await?)
}
