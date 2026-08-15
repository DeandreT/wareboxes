//! Tenant-scoped advisory slotting configuration, deterministic runs, and decisions.

mod query;

pub use query::{profile_page, recommendation_page};

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::slotting::{
    AcceptSlottingRecommendationCommand, AcceptSlottingRecommendationResult,
    ConfigureSlottingProfileCommand, ConfigureSlottingProfileResult,
    DismissSlottingRecommendationCommand, DismissSlottingRecommendationResult, RunSlottingCommand,
    RunSlottingResult, SlottingProfileReadModel, SlottingRecommendationReadModel,
    SlottingRunReadModel, ACCEPT_SLOTTING_RECOMMENDATION_OPERATION,
    CONFIGURE_SLOTTING_PROFILE_OPERATION, DISMISS_SLOTTING_RECOMMENDATION_OPERATION,
    RUN_SLOTTING_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    score_slotting_candidate, validate_slotting_dismissal, FacilityId, InventoryOwnerId,
    SlottingAdvisoryMode, SlottingProfileId, SlottingProfileRevision, SlottingRecommendationId,
    SlottingRunId, SlottingScore, SlottingScoreEvidence, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::next_outbox_sequence_tx;
use crate::repo::tasks::{create_advisory_loose_relocation_task_tx, AdvisoryLooseRelocation};

const SUPERVISOR_PERMISSION: &str = "wms_supervisor";
const MAX_RUN_CANDIDATES: usize = 20_000;

#[derive(Debug)]
struct Candidate {
    source_inventory_balance_id: i64,
    item_id: i64,
    item_description: Option<String>,
    uom: String,
    source_location_id: i64,
    source_location_label: String,
    source_zone_code: String,
    destination_location_id: i64,
    destination_location_label: String,
    destination_zone_code: String,
    item_storage_policy_id: i64,
    item_storage_policy_revision: i64,
    outstanding_demand_quantity: i64,
    historical_pick_quantity: i64,
    historical_pick_count: i64,
    source_travel_sequence: i64,
    destination_travel_sequence: i64,
    source_on_hand: i64,
    source_movable_quantity: i64,
    destination_on_hand: i64,
    destination_inbound_planned_quantity: i64,
    destination_capacity: Option<i64>,
    recommended_quantity: i64,
}

pub(super) fn require_scope(
    scope: &ScopeBindings,
    inventory_owner_id: i64,
    facility_id: i64,
    label: &str,
) -> AppResult<()> {
    if scope.includes_inventory_owner(inventory_owner_id) && scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::not_found(label))
    }
}

fn bad_domain(error: impl std::fmt::Display) -> AppError {
    AppError::bad_request(error.to_string())
}

pub(super) fn invalid_data(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

fn i64_to_u32(value: i64, label: &str) -> AppResult<u32> {
    u32::try_from(value).map_err(|_| AppError::internal(format!("invalid {label}: {value}")))
}

use query::{profile_from_row, recommendation_from_row};

async fn read_profile_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    profile_id: SlottingProfileId,
) -> AppResult<SlottingProfileReadModel> {
    let row = sqlx::query("SELECT * FROM slotting_profiles WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(profile_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("slotting profile"))?;
    profile_from_row(&row)
}

async fn read_recommendation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    recommendation_id: SlottingRecommendationId,
) -> AppResult<SlottingRecommendationReadModel> {
    let row = sqlx::query("SELECT * FROM slotting_recommendations WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(recommendation_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("slotting recommendation"))?;
    recommendation_from_row(&row)
}

struct SlottingEvent<'a> {
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    actor_id: UserId,
    aggregate_type: &'a str,
    aggregate_id: i64,
    transition: &'a str,
    occurred_at: Timestamp,
    payload: &'a serde_json::Value,
}

async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    event: SlottingEvent<'_>,
) -> AppResult<()> {
    let ordering_key = format!("{}:{}", event.aggregate_type, event.aggregate_id);
    let event_key = format!("{ordering_key}:{}", event.transition);
    let event_type = format!(
        "optimization.slotting.{}.{}",
        event.aggregate_type, event.transition
    );
    let aggregate_id = event.aggregate_id.to_string();
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(event.inventory_owner_id),
            facility_id: Some(event.facility_id),
            actor_user_id: Some(event.actor_id.get()),
            event_key: &event_key,
            aggregate_type: event.aggregate_type,
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: &event_type,
            schema_version: 1,
            payload: event.payload,
            occurred_at: event.occurred_at,
        },
    )
    .await?;
    Ok(())
}

async fn require_owner_facility_active_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
          SELECT 1 FROM inventory_owner_facilities assignment
          JOIN inventory_owners owner ON owner.tenant_id=assignment.tenant_id
            AND owner.id=assignment.inventory_owner_id AND owner.deleted IS NULL
          JOIN facilities facility ON facility.tenant_id=assignment.tenant_id
            AND facility.id=assignment.facility_id AND facility.deleted IS NULL
          WHERE assignment.tenant_id=$1 AND assignment.inventory_owner_id=$2
            AND assignment.facility_id=$3 AND assignment.deleted IS NULL
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(facility_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::not_found("slotting owner facility"))
    }
}

async fn bind_slotting_actor_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: UserId,
) -> AppResult<()> {
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(actor_id.get().to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn configure_profile(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureSlottingProfileCommand,
) -> AppResult<ConfigureSlottingProfileResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    command.definition.validate().map_err(bad_domain)?;
    if command.definition.tenant_id != access.tenant_id {
        return Err(AppError::not_found("slotting profile"));
    }
    let prepared = PreparedCommand::new_v1(context, CONFIGURE_SLOTTING_PROFILE_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    bind_slotting_actor_tx(&mut tx, context.actor_id).await?;
    require_scope(
        &scope,
        command.definition.inventory_owner_id.get(),
        command.definition.facility_id.get(),
        "slotting profile",
    )?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    require_owner_facility_active_tx(
        &mut tx,
        access.tenant_id,
        command.definition.inventory_owner_id,
        command.definition.facility_id,
    )
    .await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "slotting-profile:{}:{}:{}",
            access.tenant_id.get(),
            command.definition.inventory_owner_id.get(),
            command.definition.facility_id.get()
        ))
        .execute(&mut *tx)
        .await?;
    let latest = sqlx::query(
        r#"
        SELECT id,revision,effective_to FROM slotting_profiles
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
        ORDER BY revision DESC,id DESC LIMIT 1 FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.definition.inventory_owner_id.get())
    .bind(command.definition.facility_id.get())
    .fetch_optional(&mut *tx)
    .await?;
    let (predecessor_id, revision) = match (latest.as_ref(), command.expected_revision) {
        (None, None) => (None, SlottingProfileRevision::new(1).map_err(invalid_data)?),
        (Some(row), Some(expected))
            if row
                .try_get::<Option<Timestamp>, _>("effective_to")?
                .is_none()
                && row.try_get::<i64, _>("revision")? == expected.get() =>
        {
            let current =
                SlottingProfileRevision::new(row.try_get("revision")?).map_err(invalid_data)?;
            (
                Some(row.try_get::<i64, _>("id")?),
                current
                    .checked_next()
                    .ok_or_else(|| AppError::internal("slotting profile revision overflow"))?,
            )
        }
        (Some(_), None) => return Err(AppError::conflict("slotting profile already exists")),
        _ => {
            return Err(AppError::conflict(
                "slotting profile revision does not match expected revision",
            ))
        }
    };
    let configured_at = now_iso();
    if let Some(predecessor_id) = predecessor_id {
        sqlx::query(
            "UPDATE slotting_profiles SET effective_to=$3 WHERE tenant_id=$1 AND id=$2 AND effective_to IS NULL",
        )
        .bind(access.tenant_id.get())
        .bind(predecessor_id)
        .bind(configured_at)
        .execute(&mut *tx)
        .await?;
    }
    let profile_id = SlottingProfileId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO slotting_profiles (
              tenant_id,inventory_owner_id,facility_id,mode,demand_lookback_days,
              demand_weight,travel_weight,activity_weight,minimum_demand_quantity,
              max_recommendations,default_task_priority,revision,
              supersedes_slotting_profile_id,effective_from,configured_by_user_id,configured_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$14)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(command.definition.inventory_owner_id.get())
        .bind(command.definition.facility_id.get())
        .bind(command.definition.mode.as_str())
        .bind(i64::from(command.definition.demand_lookback_days))
        .bind(i64::from(command.definition.demand_weight))
        .bind(i64::from(command.definition.travel_weight))
        .bind(i64::from(command.definition.activity_weight))
        .bind(command.definition.minimum_demand_quantity)
        .bind(i64::from(command.definition.max_recommendations))
        .bind(i64::from(command.definition.default_task_priority))
        .bind(revision.get())
        .bind(predecessor_id)
        .bind(configured_at)
        .bind(context.actor_id.get())
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(invalid_data)?;
    let result = read_profile_tx(&mut tx, access.tenant_id, profile_id).await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        SlottingEvent {
            inventory_owner_id: result.definition.inventory_owner_id,
            facility_id: result.definition.facility_id,
            actor_id: context.actor_id,
            aggregate_type: "profile",
            aggregate_id: profile_id.get(),
            transition: "configured",
            occurred_at: configured_at,
            payload: &serde_json::to_value(&result).map_err(invalid_data)?,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn load_active_profile_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    expected_revision: SlottingProfileRevision,
) -> AppResult<SlottingProfileReadModel> {
    let row = sqlx::query(
        r#"
        SELECT * FROM slotting_profiles
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3 AND effective_to IS NULL
        FOR SHARE
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(facility_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("slotting profile is not configured"))?;
    let profile = profile_from_row(&row)?;
    if profile.revision != expected_revision {
        return Err(AppError::conflict(
            "slotting profile revision does not match expected revision",
        ));
    }
    Ok(profile)
}

async fn candidate_rows_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    profile: &SlottingProfileReadModel,
    demand_window_started_at: Timestamp,
) -> AppResult<Vec<Candidate>> {
    let rows = sqlx::query(
        r#"
        WITH reservation_demand AS (
          SELECT item_id,uom,sum(qty)::bigint AS quantity
          FROM inventory_reservations
          WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
            AND status='active' AND deleted IS NULL
          GROUP BY item_id,uom
        ), pick_demand AS (
          SELECT item_id,uom,sum(picked_qty)::bigint AS quantity,count(*)::bigint AS pick_count
          FROM pick_confirmations
          WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
            AND confirmed_at >= $4 AND confirmed_at <= transaction_timestamp()
          GROUP BY item_id,uom
        ), ranked AS (
          SELECT source.id AS source_inventory_balance_id,source.item_id,
            item.description AS item_description,source.uom,
            source.location_id AS source_location_id,
            COALESCE(NULLIF(source_location.name,''),source_location.barcode,
              'Location #'||source_location.id::text) AS source_location_label,
            source_zone.code AS source_zone_code,
            destination.location_id AS destination_location_id,
            destination.location_label AS destination_location_label,
            destination.zone_code AS destination_zone_code,
            policy.id AS item_storage_policy_id,policy.revision AS item_storage_policy_revision,
            COALESCE(reservation.quantity,0)::bigint AS outstanding_demand_quantity,
            COALESCE(picks.quantity,0)::bigint AS historical_pick_quantity,
            COALESCE(picks.pick_count,0)::bigint AS historical_pick_count,
            source_zone.travel_sequence AS source_travel_sequence,
            destination.travel_sequence AS destination_travel_sequence,
            source.qty_on_hand AS source_on_hand,
            (source.qty_on_hand-source.qty_reserved-source.qty_held)::bigint
              AS source_movable_quantity,
            destination.destination_on_hand,
            destination.destination_inbound_planned_quantity,
            policy.max_quantity_per_location AS destination_capacity,
            LEAST(
              source.qty_on_hand-source.qty_reserved-source.qty_held,
              COALESCE(reservation.quantity,0)+COALESCE(picks.quantity,0),
              CASE WHEN policy.max_quantity_per_location IS NULL
                THEN source.qty_on_hand-source.qty_reserved-source.qty_held
                ELSE policy.max_quantity_per_location-destination.destination_on_hand
                  -destination.destination_inbound_planned_quantity END
            )::bigint AS recommended_quantity,
            row_number() OVER (PARTITION BY source.item_id,source.uom
              ORDER BY (COALESCE(reservation.quantity,0)+COALESCE(picks.quantity,0)) DESC,
                (source_zone.travel_sequence-destination.travel_sequence) DESC,
                (source.qty_on_hand-source.qty_reserved-source.qty_held) DESC,
                source.id,destination.location_id) AS item_rank
          FROM inventory_balances source
          JOIN items item ON item.tenant_id=source.tenant_id AND item.id=source.item_id
            AND item.deleted IS NULL
          JOIN locations source_location ON source_location.tenant_id=source.tenant_id
            AND source_location.facility_id=source.facility_id
            AND source_location.id=source.location_id AND source_location.deleted IS NULL
            AND source_location.active
          JOIN storage_zone_locations source_member ON source_member.tenant_id=source.tenant_id
            AND source_member.facility_id=source.facility_id
            AND source_member.location_id=source.location_id
          JOIN storage_zones source_zone ON source_zone.tenant_id=source_member.tenant_id
            AND source_zone.facility_id=source_member.facility_id
            AND source_zone.id=source_member.storage_zone_id AND source_zone.effective_to IS NULL
          JOIN item_storage_policies policy ON policy.tenant_id=source.tenant_id
            AND policy.inventory_owner_id=source.inventory_owner_id
            AND policy.facility_id=source.facility_id AND policy.item_id=source.item_id
            AND policy.uom=source.uom AND policy.effective_to IS NULL
          JOIN item_storage_policy_zone_purposes allowed_pick
            ON allowed_pick.tenant_id=policy.tenant_id
            AND allowed_pick.item_storage_policy_id=policy.id AND allowed_pick.purpose='pick'
          LEFT JOIN reservation_demand reservation
            ON reservation.item_id=source.item_id AND reservation.uom=source.uom
          LEFT JOIN pick_demand picks ON picks.item_id=source.item_id AND picks.uom=source.uom
          JOIN LATERAL (
            SELECT location.id AS location_id,
              COALESCE(NULLIF(location.name,''),location.barcode,
                'Location #'||location.id::text) AS location_label,
              zone.code AS zone_code,zone.travel_sequence,
              COALESCE(sum(existing.qty_on_hand) FILTER (
                WHERE existing.inventory_owner_id=source.inventory_owner_id
                  AND existing.item_id=source.item_id AND existing.uom=source.uom
                  AND existing.deleted IS NULL),0)::bigint AS destination_on_hand,
              public.slotting_destination_planned_quantity(
                source.tenant_id,source.inventory_owner_id,source.facility_id,
                location.id,source.item_id,source.uom)
                AS destination_inbound_planned_quantity
            FROM storage_zone_locations member
            JOIN storage_zones zone ON zone.tenant_id=member.tenant_id
              AND zone.facility_id=member.facility_id AND zone.id=member.storage_zone_id
              AND zone.effective_to IS NULL AND zone.purpose='pick'
            JOIN locations location ON location.tenant_id=member.tenant_id
              AND location.facility_id=member.facility_id AND location.id=member.location_id
              AND location.deleted IS NULL AND location.active AND location.pickable
              AND NOT location.receivable AND NULLIF(btrim(location.barcode),'') IS NOT NULL
            LEFT JOIN inventory_balances existing ON existing.tenant_id=location.tenant_id
              AND existing.facility_id=location.facility_id AND existing.location_id=location.id
            WHERE member.tenant_id=source.tenant_id AND member.facility_id=source.facility_id
              AND location.id<>source.location_id
              AND zone.travel_sequence<source_zone.travel_sequence
              AND NOT EXISTS (
                SELECT 1 FROM inventory_balances conflict_balance
                JOIN item_batches existing_batch ON existing_batch.tenant_id=conflict_balance.tenant_id
                  AND existing_batch.id=conflict_balance.item_batch_id
                JOIN item_batches incoming_batch ON incoming_batch.tenant_id=source.tenant_id
                  AND incoming_batch.id=source.item_batch_id
                WHERE conflict_balance.tenant_id=source.tenant_id
                  AND conflict_balance.inventory_owner_id=source.inventory_owner_id
                  AND conflict_balance.location_id=location.id
                  AND conflict_balance.item_id=source.item_id
                  AND conflict_balance.item_batch_id<>source.item_batch_id
                  AND conflict_balance.deleted IS NULL AND conflict_balance.qty_on_hand>0
                  AND (existing_batch.lot IS DISTINCT FROM incoming_batch.lot
                    OR existing_batch.expiration IS DISTINCT FROM incoming_batch.expiration)
              )
            GROUP BY location.id,location.name,location.barcode,zone.code,zone.travel_sequence,
              member.location_sequence
            HAVING policy.max_quantity_per_location IS NULL
              OR COALESCE(sum(existing.qty_on_hand) FILTER (
                WHERE existing.inventory_owner_id=source.inventory_owner_id
                  AND existing.item_id=source.item_id AND existing.uom=source.uom
                  AND existing.deleted IS NULL),0)
                +public.slotting_destination_planned_quantity(
                  source.tenant_id,source.inventory_owner_id,source.facility_id,
                  location.id,source.item_id,source.uom)<policy.max_quantity_per_location
            ORDER BY zone.travel_sequence,member.location_sequence,location.id
            LIMIT 1
          ) destination ON true
          WHERE source.tenant_id=$1 AND source.inventory_owner_id=$2 AND source.facility_id=$3
            AND source.deleted IS NULL AND source.license_plate_id IS NULL
            AND source.status='available' AND source.qty_on_hand-source.qty_reserved-source.qty_held>0
            AND COALESCE(reservation.quantity,0)+COALESCE(picks.quantity,0)>=$5
            AND NOT EXISTS (SELECT 1 FROM inventory_relocation_tasks movement
              WHERE movement.tenant_id=source.tenant_id
                AND movement.source_inventory_balance_id=source.id AND movement.closed_at IS NULL)
            AND NOT EXISTS (SELECT 1 FROM putaway_tasks movement
              WHERE movement.tenant_id=source.tenant_id
                AND movement.source_inventory_balance_id=source.id AND movement.closed_at IS NULL)
            AND NOT EXISTS (SELECT 1 FROM slotting_recommendations existing_recommendation
              WHERE existing_recommendation.tenant_id=source.tenant_id
                AND existing_recommendation.source_inventory_balance_id=source.id
                AND existing_recommendation.status='pending')
        )
        SELECT * FROM ranked WHERE item_rank=1 AND recommended_quantity>0
        ORDER BY outstanding_demand_quantity+historical_pick_quantity DESC,
          source_travel_sequence-destination_travel_sequence DESC,
          source_inventory_balance_id,destination_location_id
        LIMIT $6
        "#,
    )
    .bind(profile.definition.tenant_id.get())
    .bind(profile.definition.inventory_owner_id.get())
    .bind(profile.definition.facility_id.get())
    .bind(demand_window_started_at)
    .bind(profile.definition.minimum_demand_quantity)
    .bind(i64::try_from(MAX_RUN_CANDIDATES + 1).map_err(invalid_data)?)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() > MAX_RUN_CANDIDATES {
        return Err(AppError::conflict(
            "slotting candidate envelope exceeded; narrow the owner/facility inventory set",
        ));
    }
    rows.into_iter()
        .map(|row| {
            Ok(Candidate {
                source_inventory_balance_id: row.try_get("source_inventory_balance_id")?,
                item_id: row.try_get("item_id")?,
                item_description: row.try_get("item_description")?,
                uom: row.try_get("uom")?,
                source_location_id: row.try_get("source_location_id")?,
                source_location_label: row.try_get("source_location_label")?,
                source_zone_code: row.try_get("source_zone_code")?,
                destination_location_id: row.try_get("destination_location_id")?,
                destination_location_label: row.try_get("destination_location_label")?,
                destination_zone_code: row.try_get("destination_zone_code")?,
                item_storage_policy_id: row.try_get("item_storage_policy_id")?,
                item_storage_policy_revision: row.try_get("item_storage_policy_revision")?,
                outstanding_demand_quantity: row.try_get("outstanding_demand_quantity")?,
                historical_pick_quantity: row.try_get("historical_pick_quantity")?,
                historical_pick_count: row.try_get("historical_pick_count")?,
                source_travel_sequence: row.try_get("source_travel_sequence")?,
                destination_travel_sequence: row.try_get("destination_travel_sequence")?,
                source_on_hand: row.try_get("source_on_hand")?,
                source_movable_quantity: row.try_get("source_movable_quantity")?,
                destination_on_hand: row.try_get("destination_on_hand")?,
                destination_inbound_planned_quantity: row
                    .try_get("destination_inbound_planned_quantity")?,
                destination_capacity: row.try_get("destination_capacity")?,
                recommended_quantity: row.try_get("recommended_quantity")?,
            })
        })
        .collect()
}

async fn lock_candidate_destinations_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    profile: &SlottingProfileReadModel,
    candidates: &[Candidate],
) -> AppResult<()> {
    let mut keys = candidates
        .iter()
        .map(|candidate| (candidate.destination_location_id, candidate.item_id))
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    for (location_id, item_id) in keys {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!(
                "inventory-location-item:{}:{}:{location_id}:{item_id}",
                profile.definition.tenant_id.get(),
                profile.definition.inventory_owner_id.get()
            ))
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

fn scored_candidate(
    profile: &SlottingProfileReadModel,
    candidate: Candidate,
) -> AppResult<(Candidate, SlottingScoreEvidence, SlottingScore)> {
    let evidence = SlottingScoreEvidence {
        outstanding_demand_quantity: candidate.outstanding_demand_quantity,
        historical_pick_quantity: candidate.historical_pick_quantity,
        historical_pick_count: candidate.historical_pick_count,
        source_travel_sequence: i64_to_u32(
            candidate.source_travel_sequence,
            "source travel sequence",
        )?,
        destination_travel_sequence: i64_to_u32(
            candidate.destination_travel_sequence,
            "destination travel sequence",
        )?,
        source_on_hand: candidate.source_on_hand,
        source_movable_quantity: candidate.source_movable_quantity,
        destination_on_hand: candidate.destination_on_hand,
        destination_inbound_planned_quantity: candidate.destination_inbound_planned_quantity,
        destination_capacity: candidate.destination_capacity,
        recommended_quantity: candidate.recommended_quantity,
    };
    let score = score_slotting_candidate(&profile.definition, &evidence).map_err(invalid_data)?;
    Ok((candidate, evidence, score))
}

pub async fn run_slotting(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RunSlottingCommand,
) -> AppResult<RunSlottingResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.tenant_id != access.tenant_id {
        return Err(AppError::not_found("slotting profile"));
    }
    let prepared = PreparedCommand::new_v1(context, RUN_SLOTTING_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    bind_slotting_actor_tx(&mut tx, context.actor_id).await?;
    require_scope(
        &scope,
        command.inventory_owner_id.get(),
        command.facility_id.get(),
        "slotting profile",
    )?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "slotting-run:{}:{}:{}",
            access.tenant_id.get(),
            command.inventory_owner_id.get(),
            command.facility_id.get()
        ))
        .execute(&mut *tx)
        .await?;
    let profile = load_active_profile_tx(
        &mut tx,
        access.tenant_id,
        command.inventory_owner_id,
        command.facility_id,
        command.expected_profile_revision,
    )
    .await?;
    if profile.definition.mode == SlottingAdvisoryMode::Disabled {
        return Err(AppError::conflict(
            "slotting optimization is disabled for this owner and facility",
        ));
    }
    let generated_at = now_iso();
    let demand_window_started_at =
        generated_at - chrono::Duration::days(i64::from(profile.definition.demand_lookback_days));
    let initial_candidates = candidate_rows_tx(&mut tx, &profile, demand_window_started_at).await?;
    lock_candidate_destinations_tx(&mut tx, &profile, &initial_candidates).await?;
    // Re-read after acquiring the same destination/item locks used by inventory
    // movement planning. This turns any wait into current evidence instead of a
    // stale recommendation snapshot.
    let candidates = candidate_rows_tx(&mut tx, &profile, demand_window_started_at).await?;
    let candidate_count = i64::try_from(candidates.len()).map_err(invalid_data)?;
    let mut scored = candidates
        .into_iter()
        .map(|candidate| scored_candidate(&profile, candidate))
        .collect::<AppResult<Vec<_>>>()?;
    scored.sort_by(|left, right| {
        right
            .2
            .total
            .cmp(&left.2.total)
            .then_with(|| {
                left.0
                    .source_inventory_balance_id
                    .cmp(&right.0.source_inventory_balance_id)
            })
            .then_with(|| {
                left.0
                    .destination_location_id
                    .cmp(&right.0.destination_location_id)
            })
    });
    scored.truncate(usize::from(profile.definition.max_recommendations));
    let recommendation_count = i64::try_from(scored.len()).map_err(invalid_data)?;
    let configuration_snapshot = serde_json::to_value(&profile).map_err(invalid_data)?;
    let configuration_snapshot_text =
        serde_json::to_string(&configuration_snapshot).map_err(invalid_data)?;
    let run_id = SlottingRunId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO slotting_runs (
              tenant_id,inventory_owner_id,facility_id,slotting_profile_id,profile_revision,
              demand_window_started_at,input_snapshot_at,configuration_snapshot,
              candidate_count,recommendation_count,generated_by_user_id,generated_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8::jsonb,$9,$10,$11,$7) RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(profile.slotting_profile_id.get())
        .bind(profile.revision.get())
        .bind(demand_window_started_at)
        .bind(generated_at)
        .bind(configuration_snapshot_text)
        .bind(candidate_count)
        .bind(recommendation_count)
        .bind(context.actor_id.get())
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(invalid_data)?;
    for (candidate, evidence, score) in scored {
        sqlx::query(
            r#"
            INSERT INTO slotting_recommendations (
              tenant_id,inventory_owner_id,facility_id,slotting_run_id,
              source_inventory_balance_id,item_id,item_description,uom,
              source_location_id,source_location_label,source_zone_code,
              destination_location_id,destination_location_label,destination_zone_code,
              item_storage_policy_id,item_storage_policy_revision,recommended_quantity,reason,
              demand_score,travel_score,activity_score,total_score,
              outstanding_demand_quantity,historical_pick_quantity,historical_pick_count,
              source_travel_sequence,destination_travel_sequence,source_on_hand,
              source_movable_quantity,destination_on_hand,
              destination_inbound_planned_quantity,destination_capacity,
              status,revision,created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,
              $19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,'pending',1,$33)
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(command.inventory_owner_id.get())
        .bind(command.facility_id.get())
        .bind(run_id.get())
        .bind(candidate.source_inventory_balance_id)
        .bind(candidate.item_id)
        .bind(candidate.item_description)
        .bind(candidate.uom)
        .bind(candidate.source_location_id)
        .bind(candidate.source_location_label)
        .bind(candidate.source_zone_code)
        .bind(candidate.destination_location_id)
        .bind(candidate.destination_location_label)
        .bind(candidate.destination_zone_code)
        .bind(candidate.item_storage_policy_id)
        .bind(candidate.item_storage_policy_revision)
        .bind(evidence.recommended_quantity)
        .bind(score.reason.as_str())
        .bind(score.demand_component)
        .bind(score.travel_component)
        .bind(score.activity_component)
        .bind(score.total)
        .bind(evidence.outstanding_demand_quantity)
        .bind(evidence.historical_pick_quantity)
        .bind(evidence.historical_pick_count)
        .bind(i64::from(evidence.source_travel_sequence))
        .bind(i64::from(evidence.destination_travel_sequence))
        .bind(evidence.source_on_hand)
        .bind(evidence.source_movable_quantity)
        .bind(evidence.destination_on_hand)
        .bind(evidence.destination_inbound_planned_quantity)
        .bind(evidence.destination_capacity)
        .bind(generated_at)
        .execute(&mut *tx)
        .await?;
    }
    let result = SlottingRunReadModel {
        slotting_run_id: run_id,
        tenant_id: access.tenant_id,
        inventory_owner_id: command.inventory_owner_id,
        facility_id: command.facility_id,
        slotting_profile_id: profile.slotting_profile_id,
        profile_revision: profile.revision,
        demand_window_started_at,
        input_snapshot_at: generated_at,
        configuration_snapshot,
        candidate_count,
        recommendation_count,
        generated_by: context.actor_id,
        generated_at,
    };
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        SlottingEvent {
            inventory_owner_id: command.inventory_owner_id,
            facility_id: command.facility_id,
            actor_id: context.actor_id,
            aggregate_type: "run",
            aggregate_id: run_id.get(),
            transition: "generated",
            occurred_at: generated_at,
            payload: &serde_json::to_value(&result).map_err(invalid_data)?,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn lock_recommendation_for_decision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    recommendation_id: SlottingRecommendationId,
) -> AppResult<sqlx::postgres::PgRow> {
    sqlx::query("SELECT * FROM slotting_recommendations WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
        .bind(tenant_id.get())
        .bind(recommendation_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("slotting recommendation"))
}

async fn require_current_recommendation_evidence_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &sqlx::postgres::PgRow,
) -> AppResult<()> {
    let tenant_id: i64 = row.try_get("tenant_id")?;
    let owner_id: i64 = row.try_get("inventory_owner_id")?;
    let facility_id: i64 = row.try_get("facility_id")?;
    let item_id: i64 = row.try_get("item_id")?;
    let uom: String = row.try_get("uom")?;
    let destination_location_id: i64 = row.try_get("destination_location_id")?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "inventory-location-item:{tenant_id}:{owner_id}:{destination_location_id}:{item_id}"
        ))
        .execute(&mut **tx)
        .await?;
    let valid: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
          SELECT 1 FROM item_storage_policies policy
          JOIN item_storage_policy_zone_purposes purpose
            ON purpose.tenant_id=policy.tenant_id
            AND purpose.item_storage_policy_id=policy.id AND purpose.purpose='pick'
          JOIN storage_zone_locations member ON member.tenant_id=policy.tenant_id
            AND member.facility_id=policy.facility_id AND member.location_id=$6
          JOIN storage_zones zone ON zone.tenant_id=member.tenant_id
            AND zone.facility_id=member.facility_id AND zone.id=member.storage_zone_id
            AND zone.effective_to IS NULL AND zone.purpose='pick'
          JOIN locations location ON location.tenant_id=member.tenant_id
            AND location.facility_id=member.facility_id AND location.id=member.location_id
            AND location.deleted IS NULL AND location.active AND location.pickable
          WHERE policy.tenant_id=$1 AND policy.inventory_owner_id=$2
            AND policy.facility_id=$3 AND policy.item_id=$4 AND policy.uom=$5
            AND policy.id=$7 AND policy.revision=$8 AND policy.effective_to IS NULL
            AND (policy.max_quantity_per_location IS NULL OR
              (SELECT COALESCE(sum(balance.qty_on_hand),0)
               FROM inventory_balances balance WHERE balance.tenant_id=$1
                 AND balance.inventory_owner_id=$2 AND balance.facility_id=$3
                 AND balance.location_id=$6 AND balance.item_id=$4 AND balance.uom=$5
                 AND balance.deleted IS NULL)
              +public.slotting_destination_planned_quantity($1,$2,$3,$6,$4,$5)
              +$9 <= policy.max_quantity_per_location)
        )
        "#,
    )
    .bind(tenant_id)
    .bind(owner_id)
    .bind(facility_id)
    .bind(item_id)
    .bind(&uom)
    .bind(destination_location_id)
    .bind(row.try_get::<i64, _>("item_storage_policy_id")?)
    .bind(row.try_get::<i64, _>("item_storage_policy_revision")?)
    .bind(row.try_get::<i64, _>("recommended_quantity")?)
    .fetch_one(&mut **tx)
    .await?;
    if valid {
        Ok(())
    } else {
        Err(AppError::conflict(
            "slotting recommendation compatibility or capacity evidence is stale",
        ))
    }
}

pub async fn accept_recommendation(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &AcceptSlottingRecommendationCommand,
) -> AppResult<AcceptSlottingRecommendationResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.expected_revision <= 0 {
        return Err(AppError::bad_request("expected revision must be positive"));
    }
    let prepared =
        PreparedCommand::new_v1(context, ACCEPT_SLOTTING_RECOMMENDATION_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    bind_slotting_actor_tx(&mut tx, context.actor_id).await?;
    let row =
        lock_recommendation_for_decision_tx(&mut tx, access.tenant_id, command.recommendation_id)
            .await?;
    require_scope(
        &scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
        "slotting recommendation",
    )?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    if row.try_get::<String, _>("status")? != "pending"
        || row.try_get::<i64, _>("revision")? != command.expected_revision
    {
        return Err(AppError::conflict(
            "slotting recommendation is no longer pending at the expected revision",
        ));
    }
    require_current_recommendation_evidence_tx(&mut tx, &row).await?;
    // The default is read through the frozen run profile, so later configuration
    // changes cannot silently change an accepted recommendation's work priority.
    let priority = match command.task_priority {
        Some(priority) => i64::from(priority),
        None => {
            sqlx::query_scalar(
                r#"
            SELECT (configuration_snapshot->'definition'->>'default_task_priority')::bigint
            FROM slotting_runs WHERE tenant_id=$1 AND id=$2
            "#,
            )
            .bind(access.tenant_id.get())
            .bind(row.try_get::<i64, _>("slotting_run_id")?)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    let metadata = serde_json::json!({
        "source": "slotting_recommendation",
        "slotting_recommendation_id": command.recommendation_id.get(),
        "slotting_run_id": row.try_get::<i64, _>("slotting_run_id")?,
        "score": row.try_get::<i64, _>("total_score")?,
    });
    let metadata_json = serde_json::to_string(&metadata).map_err(invalid_data)?;
    let task_id = create_advisory_loose_relocation_task_tx(
        &mut tx,
        access.tenant_id,
        AdvisoryLooseRelocation {
            actor_id: context.actor_id.get(),
            source_inventory_balance_id: row.try_get("source_inventory_balance_id")?,
            destination_location_id: row.try_get("destination_location_id")?,
            quantity: row.try_get("recommended_quantity")?,
            priority,
            instructions: command.instructions.as_deref(),
            metadata_json: &metadata_json,
        },
    )
    .await?;
    let decided_at = now_iso();
    let changed = sqlx::query(
        r#"
        UPDATE slotting_recommendations SET status='accepted',revision=revision+1,
          decided_by_user_id=$3,decided_at=$4,inventory_relocation_task_id=$5
        WHERE tenant_id=$1 AND id=$2 AND status='pending' AND revision=$6
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.recommendation_id.get())
    .bind(context.actor_id.get())
    .bind(decided_at)
    .bind(task_id)
    .bind(command.expected_revision)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(AppError::conflict(
            "slotting recommendation decision raced with another supervisor",
        ));
    }
    let result =
        read_recommendation_tx(&mut tx, access.tenant_id, command.recommendation_id).await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        SlottingEvent {
            inventory_owner_id: result.inventory_owner_id,
            facility_id: result.facility_id,
            actor_id: context.actor_id,
            aggregate_type: "recommendation",
            aggregate_id: command.recommendation_id.get(),
            transition: "accepted",
            occurred_at: decided_at,
            payload: &serde_json::to_value(&result).map_err(invalid_data)?,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn dismiss_recommendation(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &DismissSlottingRecommendationCommand,
) -> AppResult<DismissSlottingRecommendationResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.expected_revision <= 0 {
        return Err(AppError::bad_request("expected revision must be positive"));
    }
    validate_slotting_dismissal(command.reason, command.note.as_deref()).map_err(bad_domain)?;
    let prepared =
        PreparedCommand::new_v1(context, DISMISS_SLOTTING_RECOMMENDATION_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        SUPERVISOR_PERMISSION,
    )
    .await?;
    bind_slotting_actor_tx(&mut tx, context.actor_id).await?;
    let row =
        lock_recommendation_for_decision_tx(&mut tx, access.tenant_id, command.recommendation_id)
            .await?;
    require_scope(
        &scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
        "slotting recommendation",
    )?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    if row.try_get::<String, _>("status")? != "pending"
        || row.try_get::<i64, _>("revision")? != command.expected_revision
    {
        return Err(AppError::conflict(
            "slotting recommendation is no longer pending at the expected revision",
        ));
    }
    let decided_at = now_iso();
    sqlx::query(
        r#"
        UPDATE slotting_recommendations SET status='dismissed',revision=revision+1,
          decided_by_user_id=$3,decided_at=$4,dismissal_reason=$5,dismissal_note=$6
        WHERE tenant_id=$1 AND id=$2 AND status='pending' AND revision=$7
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.recommendation_id.get())
    .bind(context.actor_id.get())
    .bind(decided_at)
    .bind(command.reason.as_str())
    .bind(command.note.as_deref())
    .bind(command.expected_revision)
    .execute(&mut *tx)
    .await?;
    let result =
        read_recommendation_tx(&mut tx, access.tenant_id, command.recommendation_id).await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        SlottingEvent {
            inventory_owner_id: result.inventory_owner_id,
            facility_id: result.facility_id,
            actor_id: context.actor_id,
            aggregate_type: "recommendation",
            aggregate_id: command.recommendation_id.get(),
            transition: "dismissed",
            occurred_at: decided_at,
            payload: &serde_json::to_value(&result).map_err(invalid_data)?,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}
