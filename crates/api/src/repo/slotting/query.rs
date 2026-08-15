use sqlx::Row;
use wareboxes_application::slotting::{
    SlottingProfileCursor, SlottingProfilePage, SlottingProfilePageQuery, SlottingProfileReadModel,
    SlottingRecommendationCursor, SlottingRecommendationPage, SlottingRecommendationPageQuery,
    SlottingRecommendationReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, InventoryBalanceId, InventoryOwnerId, LocationId, SlottingAdvisoryMode,
    SlottingDismissalReason, SlottingProfileDefinition, SlottingProfileId, SlottingProfileRevision,
    SlottingRecommendationId, SlottingRecommendationReason, SlottingRecommendationStatus,
    SlottingRunId, SlottingScore, SlottingScoreEvidence, UserId,
};

use super::{invalid_data, require_scope};
use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{current_scope_tx, require_permission_tx};

const READ_PERMISSION: &str = "wms";

fn i64_to_u16(value: i64, label: &str) -> AppResult<u16> {
    u16::try_from(value).map_err(|_| AppError::internal(format!("invalid {label}: {value}")))
}

fn i64_to_u32(value: i64, label: &str) -> AppResult<u32> {
    u32::try_from(value).map_err(|_| AppError::internal(format!("invalid {label}: {value}")))
}

fn parse_mode(value: &str) -> AppResult<SlottingAdvisoryMode> {
    SlottingAdvisoryMode::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid slotting mode: {value}")))
}

fn parse_reason(value: &str) -> AppResult<SlottingRecommendationReason> {
    SlottingRecommendationReason::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid slotting reason: {value}")))
}

fn parse_status(value: &str) -> AppResult<SlottingRecommendationStatus> {
    SlottingRecommendationStatus::parse(value)
        .ok_or_else(|| AppError::internal(format!("invalid slotting status: {value}")))
}

fn parse_dismissal(value: Option<String>) -> AppResult<Option<SlottingDismissalReason>> {
    value
        .map(|value| {
            SlottingDismissalReason::parse(&value).ok_or_else(|| {
                AppError::internal(format!("invalid slotting dismissal reason: {value}"))
            })
        })
        .transpose()
}

pub(super) fn profile_from_row(row: &sqlx::postgres::PgRow) -> AppResult<SlottingProfileReadModel> {
    let tenant_id =
        wareboxes_domain::TenantId::new(row.try_get("tenant_id")?).map_err(invalid_data)?;
    let inventory_owner_id =
        InventoryOwnerId::new(row.try_get("inventory_owner_id")?).map_err(invalid_data)?;
    let facility_id = FacilityId::new(row.try_get("facility_id")?).map_err(invalid_data)?;
    Ok(SlottingProfileReadModel {
        slotting_profile_id: SlottingProfileId::new(row.try_get("id")?).map_err(invalid_data)?,
        definition: SlottingProfileDefinition {
            tenant_id,
            inventory_owner_id,
            facility_id,
            mode: parse_mode(&row.try_get::<String, _>("mode")?)?,
            demand_lookback_days: i64_to_u16(
                row.try_get("demand_lookback_days")?,
                "slotting lookback",
            )?,
            demand_weight: i64_to_u32(row.try_get("demand_weight")?, "demand weight")?,
            travel_weight: i64_to_u32(row.try_get("travel_weight")?, "travel weight")?,
            activity_weight: i64_to_u32(row.try_get("activity_weight")?, "activity weight")?,
            minimum_demand_quantity: row.try_get("minimum_demand_quantity")?,
            max_recommendations: i64_to_u16(
                row.try_get("max_recommendations")?,
                "slotting recommendation limit",
            )?,
            default_task_priority: i64_to_u16(
                row.try_get("default_task_priority")?,
                "slotting task priority",
            )?,
        },
        revision: SlottingProfileRevision::new(row.try_get("revision")?).map_err(invalid_data)?,
        configured_by: UserId::new(row.try_get("configured_by_user_id")?).map_err(invalid_data)?,
        configured_at: row.try_get("configured_at")?,
        effective_from: row.try_get("effective_from")?,
        supersedes_slotting_profile_id: row
            .try_get::<Option<i64>, _>("supersedes_slotting_profile_id")?
            .map(SlottingProfileId::new)
            .transpose()
            .map_err(invalid_data)?,
        effective_to: row.try_get("effective_to")?,
    })
}

pub(super) fn recommendation_from_row(
    row: &sqlx::postgres::PgRow,
) -> AppResult<SlottingRecommendationReadModel> {
    let reason = parse_reason(&row.try_get::<String, _>("reason")?)?;
    let evidence = SlottingScoreEvidence {
        outstanding_demand_quantity: row.try_get("outstanding_demand_quantity")?,
        historical_pick_quantity: row.try_get("historical_pick_quantity")?,
        historical_pick_count: row.try_get("historical_pick_count")?,
        source_travel_sequence: i64_to_u32(
            row.try_get("source_travel_sequence")?,
            "source travel sequence",
        )?,
        destination_travel_sequence: i64_to_u32(
            row.try_get("destination_travel_sequence")?,
            "destination travel sequence",
        )?,
        source_on_hand: row.try_get("source_on_hand")?,
        source_movable_quantity: row.try_get("source_movable_quantity")?,
        destination_on_hand: row.try_get("destination_on_hand")?,
        destination_inbound_planned_quantity: row
            .try_get("destination_inbound_planned_quantity")?,
        destination_capacity: row.try_get("destination_capacity")?,
        recommended_quantity: row.try_get("recommended_quantity")?,
    };
    Ok(SlottingRecommendationReadModel {
        slotting_recommendation_id: SlottingRecommendationId::new(row.try_get("id")?)
            .map_err(invalid_data)?,
        slotting_run_id: SlottingRunId::new(row.try_get("slotting_run_id")?)
            .map_err(invalid_data)?,
        tenant_id: wareboxes_domain::TenantId::new(row.try_get("tenant_id")?)
            .map_err(invalid_data)?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(invalid_data)?,
        facility_id: FacilityId::new(row.try_get("facility_id")?).map_err(invalid_data)?,
        source_inventory_balance_id: InventoryBalanceId::new(
            row.try_get("source_inventory_balance_id")?,
        )
        .map_err(invalid_data)?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        uom: row.try_get("uom")?,
        source_location_id: LocationId::new(row.try_get("source_location_id")?)
            .map_err(invalid_data)?,
        source_location_label: row.try_get("source_location_label")?,
        source_zone_code: row.try_get("source_zone_code")?,
        destination_location_id: LocationId::new(row.try_get("destination_location_id")?)
            .map_err(invalid_data)?,
        destination_location_label: row.try_get("destination_location_label")?,
        destination_zone_code: row.try_get("destination_zone_code")?,
        recommended_quantity: evidence.recommended_quantity,
        reason,
        score: SlottingScore {
            demand_component: row.try_get("demand_score")?,
            travel_component: row.try_get("travel_score")?,
            activity_component: row.try_get("activity_score")?,
            total: row.try_get("total_score")?,
            reason,
        },
        evidence,
        item_storage_policy_id: row.try_get("item_storage_policy_id")?,
        item_storage_policy_revision: row.try_get("item_storage_policy_revision")?,
        status: parse_status(&row.try_get::<String, _>("status")?)?,
        revision: row.try_get("revision")?,
        decided_by: row
            .try_get::<Option<i64>, _>("decided_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(invalid_data)?,
        decided_at: row.try_get("decided_at")?,
        dismissal_reason: parse_dismissal(row.try_get("dismissal_reason")?)?,
        dismissal_note: row.try_get("dismissal_note")?,
        inventory_relocation_task_id: row.try_get("inventory_relocation_task_id")?,
        created_at: row.try_get("created_at")?,
    })
}

pub async fn profile_page(
    db: &Db,
    access: &TenantAccess,
    query: SlottingProfilePageQuery,
) -> AppResult<SlottingProfilePage> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        READ_PERMISSION,
    )
    .await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    if let Some(owner_id) = query.inventory_owner_id {
        if !scope.includes_inventory_owner(owner_id.get()) {
            return Err(AppError::not_found("slotting profile"));
        }
    }
    if let Some(facility_id) = query.facility_id {
        if !scope.includes_facility(facility_id.get()) {
            return Err(AppError::not_found("slotting profile"));
        }
    }
    let fetch_limit = i64::from(query.limit) + 1;
    let rows = sqlx::query(
        r#"
        SELECT * FROM slotting_profiles profile
        WHERE profile.tenant_id=$1
          AND ($2::bigint IS NULL OR profile.inventory_owner_id=$2)
          AND ($3::bigint IS NULL OR profile.facility_id=$3)
          AND ($4 OR profile.effective_to IS NULL)
          AND ($5 OR profile.inventory_owner_id=ANY($6))
          AND ($7 OR profile.facility_id=ANY($8))
          AND ($9::timestamptz IS NULL OR (profile.configured_at,profile.id)<($9,$10))
        ORDER BY profile.configured_at DESC,profile.id DESC LIMIT $11
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(query.facility_id.map(FacilityId::get))
    .bind(query.include_history)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(query.cursor.map(|cursor| cursor.after_configured_at))
    .bind(
        query
            .cursor
            .map(|cursor| cursor.after_slotting_profile_id.get()),
    )
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(profile_from_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if rows.len() > usize::from(query.limit) {
        items.last().map(|item| SlottingProfileCursor {
            after_configured_at: item.configured_at,
            after_slotting_profile_id: item.slotting_profile_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(SlottingProfilePage {
        items: std::mem::take(&mut items),
        next_cursor,
    })
}

pub async fn recommendation_page(
    db: &Db,
    access: &TenantAccess,
    query: SlottingRecommendationPageQuery,
) -> AppResult<SlottingRecommendationPage> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        READ_PERMISSION,
    )
    .await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    if let (Some(owner_id), Some(facility_id)) = (query.inventory_owner_id, query.facility_id) {
        require_scope(
            &scope,
            owner_id.get(),
            facility_id.get(),
            "slotting recommendation",
        )?;
    }
    let rows = sqlx::query(
        r#"
        SELECT * FROM slotting_recommendations recommendation
        WHERE recommendation.tenant_id=$1
          AND ($2::bigint IS NULL OR recommendation.inventory_owner_id=$2)
          AND ($3::bigint IS NULL OR recommendation.facility_id=$3)
          AND ($4::bigint IS NULL OR recommendation.slotting_run_id=$4)
          AND ($5::text IS NULL OR recommendation.status=$5)
          AND ($6 OR recommendation.inventory_owner_id=ANY($7))
          AND ($8 OR recommendation.facility_id=ANY($9))
          AND ($10::bigint IS NULL OR (recommendation.total_score,recommendation.id)<($10,$11))
        ORDER BY recommendation.total_score DESC,recommendation.id DESC LIMIT $12
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(query.facility_id.map(FacilityId::get))
    .bind(query.slotting_run_id.map(SlottingRunId::get))
    .bind(query.status.map(SlottingRecommendationStatus::as_str))
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(query.cursor.map(|cursor| cursor.after_score))
    .bind(
        query
            .cursor
            .map(|cursor| cursor.after_slotting_recommendation_id.get()),
    )
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(recommendation_from_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if rows.len() > usize::from(query.limit) {
        items.last().map(|item| SlottingRecommendationCursor {
            after_score: item.score.total,
            after_slotting_recommendation_id: item.slotting_recommendation_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(SlottingRecommendationPage { items, next_cursor })
}
