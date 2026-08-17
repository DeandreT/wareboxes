use sha2::{Digest, Sha256};
use sqlx::Row;
use wareboxes_application::carrier::{
    CancelCarrierManifestCommand, CarrierAddressSnapshot, CarrierManifestAdapterRequest,
    CarrierManifestJobReadModel, CarrierPackageSnapshot, QueueCarrierManifestCommand,
    QueueCarrierManifestResult, RetryCarrierManifestCommand, CANCEL_CARRIER_MANIFEST_OPERATION,
    QUEUE_CARRIER_MANIFEST_OPERATION, RETRY_CARRIER_MANIFEST_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CarrierAccountId, CarrierManifestJobId, CarrierManifestJobStatus, CartonId, FacilityId,
    InventoryOwnerId, ShipmentId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{current_scope_tx, lock_current_scope_tx, require_permission_tx};

use super::mapping::{self, JOB_COLUMNS};
use super::{bind_actor_tx, insert_outbox_tx, CarrierEvent};

const MAX_PAGE: u16 = 100;

#[derive(Debug, Clone, Copy)]
pub struct CarrierManifestJobPageFilter {
    pub shipment_id: ShipmentId,
    pub after_job_id: Option<CarrierManifestJobId>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarrierManifestJobPage {
    pub items: Vec<CarrierManifestJobReadModel>,
    pub next_job_id: Option<CarrierManifestJobId>,
}

struct ShipmentScope {
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    revision: i64,
    state: String,
}

pub async fn queue(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &QueueCarrierManifestCommand,
) -> AppResult<QueueCarrierManifestResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, QUEUE_CARRIER_MANIFEST_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared
        .replayed::<QueueCarrierManifestResult>(&mut tx)
        .await?
    {
        require_job_visible(&scope, &result.job)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_shipment_key(&mut tx, access.tenant_id.get(), command.shipment_id.get()).await?;
    let shipment = lock_shipment(&mut tx, access, command.shipment_id, &scope).await?;
    if shipment.state != "awaiting manifest"
        || shipment.revision != command.expected_shipment_revision.get()
    {
        return Err(AppError::conflict(
            "shipment is not awaiting a carrier manifest at the expected revision",
        ));
    }
    let account = lock_active_account(
        &mut tx,
        access,
        command.account_id,
        shipment.inventory_owner_id,
        shipment.facility_id,
    )
    .await?;
    let active: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM carrier_manifest_jobs
           WHERE tenant_id=$1 AND shipment_id=$2
             AND status IN ('queued','processing','retry_scheduled'))"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.shipment_id.get())
    .fetch_one(&mut *tx)
    .await?;
    if active {
        return Err(AppError::conflict(
            "shipment already has an active carrier manifest job",
        ));
    }
    let request_key = request_key(
        access.tenant_id.get(),
        command.shipment_id.get(),
        context
            .idempotency_key
            .as_deref()
            .ok_or_else(|| AppError::bad_request("carrier manifest requires idempotency"))?,
    );
    let request = CarrierManifestAdapterRequest {
        schema_version: 1,
        request_key: request_key.clone(),
        tenant_id: access.tenant_id,
        account_key: account.account_key.clone(),
        carrier_code: account.carrier_code.clone(),
        service_code: command.service_code.clone(),
        shipment_id: command.shipment_id,
        origin: address_snapshot(&mut tx, access, command.shipment_id, "origin").await?,
        destination: address_snapshot(&mut tx, access, command.shipment_id, "destination").await?,
        packages: package_snapshots(&mut tx, access, command.shipment_id).await?,
    };
    let request_payload = serde_json::to_value(&request)
        .map_err(|error| AppError::internal(format!("serializing carrier request: {error}")))?;
    let now = now_iso();
    let row = sqlx::query(&format!(
        r#"INSERT INTO carrier_manifest_jobs AS job
           (tenant_id,inventory_owner_id,facility_id,shipment_id,carrier_account_id,
            carrier_account_revision,account_key,carrier_code,service_code,
            expected_shipment_revision,request_key,request_payload,request_sha256,
            status,revision,attempt_count,requested_by_user_id,requested_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,
             sha256(convert_to($12::jsonb::text,'UTF8')),'queued',1,0,$13,$14)
           RETURNING {}"#,
        JOB_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(shipment.inventory_owner_id.get())
    .bind(shipment.facility_id.get())
    .bind(command.shipment_id.get())
    .bind(account.account_id.get())
    .bind(account.revision)
    .bind(account.account_key.as_str())
    .bind(account.carrier_code.as_str())
    .bind(command.service_code.as_ref().map(|value| value.as_str()))
    .bind(command.expected_shipment_revision.get())
    .bind(&request_key)
    .bind(request_payload)
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let job = mapping::job(&row)?;
    insert_job_event(&mut tx, &job, context.actor_id.get(), "queued", now).await?;
    Ok(prepared
        .commit(tx, QueueCarrierManifestResult { job })
        .await?)
}

pub async fn cancel(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelCarrierManifestCommand,
) -> AppResult<CarrierManifestJobReadModel> {
    mutate_job(
        db,
        access,
        context,
        CANCEL_CARRIER_MANIFEST_OPERATION,
        command,
        command.shipment_id,
        command.job_id,
        command.expected_revision,
        CarrierManifestJobStatus::Cancelled,
    )
    .await
}

pub async fn retry(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RetryCarrierManifestCommand,
) -> AppResult<CarrierManifestJobReadModel> {
    mutate_job(
        db,
        access,
        context,
        RETRY_CARRIER_MANIFEST_OPERATION,
        command,
        command.shipment_id,
        command.job_id,
        command.expected_revision,
        CarrierManifestJobStatus::RetryScheduled,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn mutate_job<C: serde::Serialize>(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    operation: &'static str,
    command: &C,
    shipment_id: ShipmentId,
    job_id: CarrierManifestJobId,
    expected_revision: u32,
    target: CarrierManifestJobStatus,
) -> AppResult<CarrierManifestJobReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if expected_revision == 0 {
        return Err(AppError::bad_request("expected revision must be positive"));
    }
    let prepared = PreparedCommand::new_v1(context, operation, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        if target == CarrierManifestJobStatus::RetryScheduled {
            "wms_supervisor"
        } else {
            "wms"
        },
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<CarrierManifestJobReadModel>(&mut tx)
        .await?
    {
        require_job_visible(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_job(&mut tx, access, shipment_id, job_id, &scope).await?;
    if current.revision != expected_revision {
        return Err(AppError::conflict("carrier manifest job revision is stale"));
    }
    match target {
        CarrierManifestJobStatus::Cancelled
            if !matches!(
                current.status,
                CarrierManifestJobStatus::Queued | CarrierManifestJobStatus::RetryScheduled
            ) =>
        {
            return Err(AppError::conflict(
                "only a queued or retry-scheduled carrier manifest job can be cancelled",
            ));
        }
        CarrierManifestJobStatus::RetryScheduled
            if current.status != CarrierManifestJobStatus::Failed =>
        {
            return Err(AppError::conflict(
                "only a failed carrier manifest job can be retried",
            ));
        }
        _ => {}
    }
    let now = now_iso();
    let row = if target == CarrierManifestJobStatus::Cancelled {
        sqlx::query(&format!(
            r#"UPDATE carrier_manifest_jobs AS job
               SET status='cancelled',revision=revision+1,next_attempt_at=NULL,
                   completed_at=$4
               WHERE tenant_id=$1 AND id=$2 AND revision=$3 RETURNING {}"#,
            JOB_COLUMNS
        ))
        .bind(access.tenant_id.get())
        .bind(job_id.get())
        .bind(i32::try_from(expected_revision).map_err(|_| {
            AppError::bad_request("carrier manifest job revision exceeds the supported range")
        })?)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
    } else {
        sqlx::query(&format!(
            r#"UPDATE carrier_manifest_jobs AS job
               SET status='retry_scheduled',revision=revision+1,next_attempt_at=$4,
                   completed_at=NULL
               WHERE tenant_id=$1 AND id=$2 AND revision=$3 RETURNING {}"#,
            JOB_COLUMNS
        ))
        .bind(access.tenant_id.get())
        .bind(job_id.get())
        .bind(i32::try_from(expected_revision).map_err(|_| {
            AppError::bad_request("carrier manifest job revision exceeds the supported range")
        })?)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
    }
    .ok_or_else(|| AppError::conflict("carrier manifest job changed concurrently"))?;
    let result = mapping::job(&row)?;
    insert_job_event(
        &mut tx,
        &result,
        context.actor_id.get(),
        if target == CarrierManifestJobStatus::Cancelled {
            "cancelled"
        } else {
            "retry_scheduled"
        },
        now,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn list_jobs(
    db: &Db,
    access: &TenantAccess,
    filter: CarrierManifestJobPageFilter,
) -> AppResult<CarrierManifestJobPage> {
    if filter.limit == 0 || filter.limit > MAX_PAGE {
        return Err(AppError::bad_request(
            "carrier manifest job page size is outside the supported range",
        ));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let rows = sqlx::query(&format!(
        r#"SELECT {} FROM carrier_manifest_jobs job
           WHERE job.tenant_id=$1 AND job.shipment_id=$2
             AND ($3 OR job.facility_id=ANY($4))
             AND ($5 OR job.inventory_owner_id=ANY($6))
             AND ($7::bigint IS NULL OR job.id<$7)
           ORDER BY job.id DESC LIMIT $8"#,
        JOB_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(filter.shipment_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.after_job_id.map(CarrierManifestJobId::get))
    .bind(i64::from(filter.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = rows
        .iter()
        .map(mapping::job)
        .collect::<AppResult<Vec<_>>>()?;
    let has_more = items.len() > usize::from(filter.limit);
    if has_more {
        items.pop();
    }
    let next_job_id = has_more
        .then(|| items.last().map(|item| item.job_id))
        .flatten();
    if items.is_empty() {
        require_shipment_visible(&mut tx, access, filter.shipment_id, &scope).await?;
    }
    tx.commit().await?;
    Ok(CarrierManifestJobPage { items, next_job_id })
}

pub async fn get_job(
    db: &Db,
    access: &TenantAccess,
    shipment_id: ShipmentId,
    job_id: CarrierManifestJobId,
) -> AppResult<CarrierManifestJobReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let result = read_job(&mut tx, access, shipment_id, job_id, &scope, false).await?;
    tx.commit().await?;
    Ok(result)
}

async fn lock_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    shipment_id: ShipmentId,
    job_id: CarrierManifestJobId,
    scope: &crate::repo::access::ScopeBindings,
) -> AppResult<CarrierManifestJobReadModel> {
    read_job(tx, access, shipment_id, job_id, scope, true).await
}

async fn read_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    shipment_id: ShipmentId,
    job_id: CarrierManifestJobId,
    scope: &crate::repo::access::ScopeBindings,
    lock: bool,
) -> AppResult<CarrierManifestJobReadModel> {
    let sql = format!(
        r#"SELECT {} FROM carrier_manifest_jobs job
           WHERE job.tenant_id=$1 AND job.shipment_id=$2 AND job.id=$3
             AND ($4 OR job.facility_id=ANY($5))
             AND ($6 OR job.inventory_owner_id=ANY($7)) {}"#,
        JOB_COLUMNS,
        if lock { "FOR UPDATE OF job" } else { "" }
    );
    let row = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(shipment_id.get())
        .bind(job_id.get())
        .bind(scope.all_facilities)
        .bind(&scope.facility_ids)
        .bind(scope.all_inventory_owners)
        .bind(&scope.inventory_owner_ids)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("carrier manifest job"))?;
    mapping::job(&row)
}

async fn lock_shipment(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    shipment_id: ShipmentId,
    scope: &crate::repo::access::ScopeBindings,
) -> AppResult<ShipmentScope> {
    let row = sqlx::query(
        r#"SELECT inventory_owner_id,facility_id,revision,state FROM shipments
           WHERE tenant_id=$1 AND id=$2 AND ($3 OR facility_id=ANY($4))
             AND ($5 OR inventory_owner_id=ANY($6)) FOR UPDATE"#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("shipment"))?;
    Ok(ShipmentScope {
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        revision: row.try_get("revision")?,
        state: row.try_get("state")?,
    })
}

struct AccountSnapshot {
    account_id: CarrierAccountId,
    revision: i32,
    account_key: wareboxes_domain::CarrierAccountKey,
    carrier_code: wareboxes_domain::CarrierCode,
}

async fn lock_active_account(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    account_id: CarrierAccountId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<AccountSnapshot> {
    let row = sqlx::query(
        r#"SELECT id,revision,account_key,carrier_code FROM carrier_accounts
           WHERE tenant_id=$1 AND id=$2 AND inventory_owner_id=$3 AND facility_id=$4
             AND status='active' FOR SHARE"#,
    )
    .bind(access.tenant_id.get())
    .bind(account_id.get())
    .bind(inventory_owner_id.get())
    .bind(facility_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("active carrier account"))?;
    Ok(AccountSnapshot {
        account_id: CarrierAccountId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        revision: row.try_get("revision")?,
        account_key: wareboxes_domain::CarrierAccountKey::new(
            row.try_get::<String, _>("account_key")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        carrier_code: wareboxes_domain::CarrierCode::new(row.try_get::<String, _>("carrier_code")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

async fn address_snapshot(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    shipment_id: ShipmentId,
    role: &str,
) -> AppResult<CarrierAddressSnapshot> {
    let row = sqlx::query(
        r#"SELECT name,company,line1,line2,postal_code,country,phone,email,state,
                  county,city,territory,district
           FROM shipment_address_snapshots
           WHERE tenant_id=$1 AND shipment_id=$2 AND address_role=$3"#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment_id.get())
    .bind(role)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("shipment address snapshot is incomplete"))?;
    Ok(CarrierAddressSnapshot {
        name: row.try_get("name")?,
        company: row.try_get("company")?,
        line1: row.try_get("line1")?,
        line2: row.try_get("line2")?,
        postal_code: row.try_get("postal_code")?,
        country: row.try_get("country")?,
        phone: row.try_get("phone")?,
        email: row.try_get("email")?,
        state: row.try_get("state")?,
        county: row.try_get("county")?,
        city: row.try_get("city")?,
        territory: row.try_get("territory")?,
        district: row.try_get("district")?,
    })
}

async fn package_snapshots(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    shipment_id: ShipmentId,
) -> AppResult<Vec<CarrierPackageSnapshot>> {
    let rows = sqlx::query(
        r#"SELECT carton_id,carton_barcode,weight_g,length_mm,width_mm,height_mm
           FROM shipment_cartons WHERE tenant_id=$1 AND shipment_id=$2
           ORDER BY carton_id"#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(CarrierPackageSnapshot {
                carton_id: CartonId::new(row.try_get("carton_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                carton_barcode: row.try_get("carton_barcode")?,
                weight_grams: row.try_get::<Option<i64>, _>("weight_g")?.ok_or_else(|| {
                    AppError::conflict("every carton requires weight before carrier manifesting")
                })?,
                length_millimeters: row.try_get("length_mm")?,
                width_millimeters: row.try_get("width_mm")?,
                height_millimeters: row.try_get("height_mm")?,
            })
        })
        .collect()
}

async fn insert_job_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job: &CarrierManifestJobReadModel,
    actor_user_id: i64,
    transition: &str,
    occurred_at: wareboxes_domain::Timestamp,
) -> AppResult<()> {
    let payload = serde_json::to_value(job)
        .map_err(|error| AppError::internal(format!("serializing carrier job: {error}")))?;
    insert_outbox_tx(
        tx,
        CarrierEvent {
            tenant_id: job.tenant_id,
            inventory_owner_id: job.inventory_owner_id,
            facility_id: job.facility_id,
            actor_user_id,
            aggregate_type: "carrier_manifest_job",
            aggregate_id: job.job_id.get().to_string(),
            event_type: match transition {
                "queued" => "carrier.manifest.queued",
                "cancelled" => "carrier.manifest.cancelled",
                _ => "carrier.manifest.retry_scheduled",
            },
            event_key: format!(
                "carrier-manifest-job:{}:{}:{}",
                job.job_id.get(),
                job.revision,
                transition
            ),
            payload: &payload,
            occurred_at,
        },
    )
    .await
}

async fn require_shipment_visible(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    shipment_id: ShipmentId,
    scope: &crate::repo::access::ScopeBindings,
) -> AppResult<()> {
    let visible: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM shipments WHERE tenant_id=$1 AND id=$2
           AND ($3 OR facility_id=ANY($4)) AND ($5 OR inventory_owner_id=ANY($6)))"#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("shipment"))
    }
}

fn require_job_visible(
    scope: &crate::repo::access::ScopeBindings,
    job: &CarrierManifestJobReadModel,
) -> AppResult<()> {
    if scope.includes_facility(job.facility_id.get())
        && scope.includes_inventory_owner(job.inventory_owner_id.get())
    {
        Ok(())
    } else {
        Err(AppError::not_found("carrier manifest job"))
    }
}

async fn lock_shipment_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    shipment_id: i64,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("carrier-manifest:{tenant_id}:{shipment_id}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn request_key(tenant_id: i64, shipment_id: i64, idempotency_key: &str) -> String {
    hex::encode(Sha256::digest(
        format!("carrier-manifest-v1:{tenant_id}:{shipment_id}:{idempotency_key}").as_bytes(),
    ))
}
