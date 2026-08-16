use serde_json::json;
use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::pick_zone::{
    ClaimNextZonePickCommand, PickZoneQueueReadModel, PickZoneWorkspace, PickZoneWorkspaceQuery,
    CLAIM_NEXT_ZONE_PICK_OPERATION,
};
use wareboxes_application::picking::PickClaim;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, InventoryOwnerId, PickTaskId, PickZoneClaimId, StorageZoneId, StorageZoneRevision,
    StorageZoneTravelSequence, TenantId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::next_outbox_sequence_tx;

use super::claim::{
    active_task_for_user_tx, claim_open_task_tx, load_claim_tx, release_expired_claims_tx,
    release_inaccessible_claim_tx, require_task_visible_tx,
};

const MAX_ZONE_QUEUES: i64 = 200;

struct ZoneClaimEvent {
    zone_claim_id: PickZoneClaimId,
    actor_user_id: i64,
    claimed_at: wareboxes_domain::Timestamp,
}

pub async fn workspace(
    db: &Db,
    access: &TenantAccess,
    query: PickZoneWorkspaceQuery,
) -> AppResult<PickZoneWorkspace> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        "wms_supervisor",
    )
    .await?;
    require_scope(
        &scope,
        query.inventory_owner_id,
        query.facility_id,
        "pick zone workspace",
    )?;
    require_owner_facility_tx(
        &mut tx,
        access.tenant_id,
        query.inventory_owner_id,
        query.facility_id,
    )
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT zone.id,zone.code,zone.name,zone.revision,zone.travel_sequence,
          COUNT(DISTINCT task.id) FILTER(WHERE task.status='open') AS open_task_count,
          COUNT(DISTINCT task.id) FILTER(WHERE task.status='in_progress') AS active_task_count,
          MIN(task.created_at) FILTER(WHERE task.status='open') AS oldest_open_task_at
        FROM storage_zones zone
        JOIN storage_zone_locations zone_location
          ON zone_location.tenant_id=zone.tenant_id
         AND zone_location.facility_id=zone.facility_id
         AND zone_location.storage_zone_id=zone.id
        LEFT JOIN pick_task_contents content
          ON content.tenant_id=zone_location.tenant_id
         AND content.facility_id=zone_location.facility_id
         AND content.source_location_id=zone_location.location_id
         AND content.state='pending'
        LEFT JOIN pick_tasks task
          ON task.tenant_id=content.tenant_id AND task.id=content.task_id
         AND task.inventory_owner_id=$2 AND task.facility_id=$3
         AND task.status IN('open','in_progress')
         AND NOT EXISTS(
           SELECT 1 FROM pick_cluster_members cluster_member
           JOIN pick_clusters cluster ON cluster.tenant_id=cluster_member.tenant_id
             AND cluster.id=cluster_member.cluster_id
           WHERE cluster_member.tenant_id=task.tenant_id
             AND cluster_member.task_id=task.id
             AND cluster.status IN('planned','in_progress'))
        WHERE zone.tenant_id=$1 AND zone.facility_id=$3
          AND zone.purpose='pick' AND zone.effective_to IS NULL
        GROUP BY zone.id,zone.code,zone.name,zone.revision,zone.travel_sequence
        ORDER BY zone.travel_sequence,zone.code,zone.id
        LIMIT $4
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(query.inventory_owner_id.get())
    .bind(query.facility_id.get())
    .bind(MAX_ZONE_QUEUES + 1)
    .fetch_all(&mut *tx)
    .await?;
    if i64::try_from(rows.len()).unwrap_or(i64::MAX) > MAX_ZONE_QUEUES {
        return Err(AppError::conflict(
            "pick zone workspace exceeds the supported queue count",
        ));
    }
    let queues = rows
        .into_iter()
        .map(|row| {
            Ok(PickZoneQueueReadModel {
                storage_zone_id: StorageZoneId::new(row.try_get("id")?).map_err(invalid_data)?,
                code: row.try_get("code")?,
                name: row.try_get("name")?,
                revision: StorageZoneRevision::new(row.try_get("revision")?)
                    .map_err(invalid_data)?,
                travel_sequence: StorageZoneTravelSequence::new(
                    u32::try_from(row.try_get::<i64, _>("travel_sequence")?)
                        .map_err(invalid_data)?,
                ),
                open_task_count: row.try_get("open_task_count")?,
                active_task_count: row.try_get("active_task_count")?,
                oldest_open_task_at: row.try_get("oldest_open_task_at")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(PickZoneWorkspace { queues })
}

pub async fn claim_next(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: ClaimNextZonePickCommand,
) -> AppResult<Option<PickClaim>> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CLAIM_NEXT_ZONE_PICK_OPERATION, &command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;

    if let Some(result) = prepared.replayed::<Option<PickClaim>>(&mut tx).await? {
        if let Some(claim) = result.as_ref() {
            require_task_visible_tx(&mut tx, access.tenant_id, claim.task_id, &scope).await?;
            if claim.execution.storage_zone_id != Some(command.storage_zone_id) {
                return Err(AppError::conflict(
                    "stored zone claim does not match the requested zone",
                ));
            }
        }
        tx.commit().await?;
        return Ok(result);
    }

    release_expired_claims_tx(&mut tx, access.tenant_id, &scope).await?;
    release_inaccessible_claim_tx(&mut tx, access.tenant_id, context.actor_id.get(), &scope)
        .await?;
    if active_task_for_user_tx(&mut tx, access.tenant_id, context.actor_id.get())
        .await?
        .is_some()
    {
        return Err(AppError::conflict(
            "operator already has active pick work; resume or release it first",
        ));
    }

    let zone = sqlx::query(
        r#"
        SELECT id,facility_id,code,revision,travel_sequence
        FROM storage_zones
        WHERE tenant_id=$1 AND id=$2 AND purpose='pick' AND effective_to IS NULL
        FOR SHARE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.storage_zone_id.get())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("pick zone"))?;
    let facility_id = FacilityId::new(zone.try_get("facility_id")?).map_err(invalid_data)?;
    if !scope.includes_facility(facility_id.get()) {
        return Err(AppError::not_found("pick zone"));
    }

    let candidate = sqlx::query(
        r#"
        SELECT task.id,task.inventory_owner_id,task.facility_id,
          content.source_location_id,content.source_inventory_balance_id
        FROM storage_zone_locations zone_location
        JOIN pick_task_contents content
          ON content.tenant_id=zone_location.tenant_id
         AND content.facility_id=zone_location.facility_id
         AND content.source_location_id=zone_location.location_id
         AND content.state='pending'
        JOIN pick_tasks task
          ON task.tenant_id=content.tenant_id AND task.id=content.task_id
        JOIN facilities facility
          ON facility.tenant_id=task.tenant_id AND facility.id=task.facility_id
         AND facility.deleted IS NULL
        JOIN inventory_owners owner
          ON owner.tenant_id=task.tenant_id AND owner.id=task.inventory_owner_id
         AND owner.deleted IS NULL
        JOIN inventory_owner_facilities owner_facility
          ON owner_facility.tenant_id=task.tenant_id
         AND owner_facility.inventory_owner_id=task.inventory_owner_id
         AND owner_facility.facility_id=task.facility_id
         AND owner_facility.deleted IS NULL
        WHERE zone_location.tenant_id=$1
          AND zone_location.facility_id=$2 AND zone_location.storage_zone_id=$3
          AND task.status='open' AND task.assigned_user_id IS NULL
          AND ($4 OR task.inventory_owner_id=ANY($5))
          AND NOT EXISTS(
            SELECT 1 FROM pick_cluster_members cluster_member
            JOIN pick_clusters cluster ON cluster.tenant_id=cluster_member.tenant_id
              AND cluster.id=cluster_member.cluster_id
            WHERE cluster_member.tenant_id=task.tenant_id
              AND cluster_member.task_id=task.id
              AND cluster.status IN('planned','in_progress'))
        ORDER BY zone_location.location_sequence,task.priority DESC,
          task.ship_by NULLS LAST,task.created_at,task.id
        FOR UPDATE OF task SKIP LOCKED
        LIMIT 1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(facility_id.get())
    .bind(command.storage_zone_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;

    let result = match candidate {
        None => None,
        Some(candidate) => {
            let task_id = PickTaskId::new(candidate.try_get("id")?).map_err(invalid_data)?;
            let owner_id = InventoryOwnerId::new(candidate.try_get("inventory_owner_id")?)
                .map_err(invalid_data)?;
            let claimed_at = now_iso();
            claim_open_task_tx(
                &mut tx,
                access.tenant_id,
                task_id.get(),
                owner_id,
                facility_id,
                context.actor_id.get(),
                claimed_at,
            )
            .await?;
            let zone_claim_id = PickZoneClaimId::new(
                sqlx::query_scalar(
                    r#"
                    INSERT INTO pick_zone_claims(
                      tenant_id,inventory_owner_id,facility_id,task_id,
                      claimed_by_user_id,claimed_at,storage_zone_id,storage_zone_code,
                      storage_zone_revision,storage_zone_travel_sequence,
                      source_location_id,source_inventory_balance_id)
                    VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
                    RETURNING id
                    "#,
                )
                .bind(access.tenant_id.get())
                .bind(owner_id.get())
                .bind(facility_id.get())
                .bind(task_id.get())
                .bind(context.actor_id.get())
                .bind(claimed_at)
                .bind(command.storage_zone_id.get())
                .bind(zone.try_get::<String, _>("code")?)
                .bind(zone.try_get::<i64, _>("revision")?)
                .bind(zone.try_get::<i64, _>("travel_sequence")?)
                .bind(candidate.try_get::<i64, _>("source_location_id")?)
                .bind(candidate.try_get::<i64, _>("source_inventory_balance_id")?)
                .fetch_one(&mut *tx)
                .await?,
            )
            .map_err(invalid_data)?;
            let claim =
                load_claim_tx(&mut tx, access.tenant_id, task_id, context.actor_id.get()).await?;
            enqueue_claim_event_tx(
                &mut tx,
                access.tenant_id,
                ZoneClaimEvent {
                    zone_claim_id,
                    actor_user_id: context.actor_id.get(),
                    claimed_at,
                },
                &claim,
            )
            .await?;
            Some(claim)
        }
    };
    Ok(prepared.commit(tx, result).await?)
}

async fn bind_actor_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_user_id: i64,
) -> AppResult<()> {
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(actor_user_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn require_owner_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
          SELECT 1 FROM inventory_owner_facilities assignment
          JOIN inventory_owners owner ON owner.tenant_id=assignment.tenant_id
            AND owner.id=assignment.inventory_owner_id AND owner.deleted IS NULL
          JOIN facilities facility ON facility.tenant_id=assignment.tenant_id
            AND facility.id=assignment.facility_id AND facility.deleted IS NULL
          WHERE assignment.tenant_id=$1 AND assignment.inventory_owner_id=$2
            AND assignment.facility_id=$3 AND assignment.deleted IS NULL)"#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::not_found("pick zone workspace"))
    }
}

fn require_scope(
    scope: &ScopeBindings,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    resource: &str,
) -> AppResult<()> {
    if scope.includes_inventory_owner(owner_id.get()) && scope.includes_facility(facility_id.get())
    {
        Ok(())
    } else {
        Err(AppError::not_found(resource))
    }
}

async fn enqueue_claim_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    event: ZoneClaimEvent,
    claim: &PickClaim,
) -> AppResult<()> {
    let aggregate_id = event.zone_claim_id.to_string();
    let ordering_key = format!("pick-task:{}", claim.task_id.get());
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(claim.inventory_owner_id),
            facility_id: Some(claim.facility_id),
            actor_user_id: Some(event.actor_user_id),
            event_key: &format!("pick-zone-claim:{}:claimed", event.zone_claim_id.get()),
            aggregate_type: "pick_zone_claim",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: "outbound.pick_zone.claimed",
            schema_version: 1,
            payload: &json!({
                "zone_claim_id": event.zone_claim_id,
                "task_id": claim.task_id,
                "inventory_owner_id": claim.inventory_owner_id,
                "facility_id": claim.facility_id,
                "claimed_by_user_id": event.actor_user_id,
                "storage_zone_id": claim.execution.storage_zone_id,
                "storage_zone_code": claim.execution.storage_zone_code,
                "storage_zone_revision": claim.execution.storage_zone_revision,
                "storage_zone_travel_sequence": claim.execution.storage_zone_travel_sequence,
                "source_location_id": claim.content.source_location_id,
                "source_inventory_balance_id": claim.content.source_inventory_balance_id,
            }),
            occurred_at: event.claimed_at,
        },
    )
    .await?;
    Ok(())
}

fn invalid_data(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}
