use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    AcceptSlottingRecommendationRequest, ConfigureSlottingProfileRequest,
    DismissSlottingRecommendationRequest, OpaqueCursor, Revision, RunSlottingRequest,
    SlottingAdvisoryMode as ApiMode, SlottingDismissalReason as ApiDismissal,
    SlottingProfilePage as ApiProfilePage, SlottingProfilePageRequest, SlottingProfileResponse,
    SlottingRecommendationPage as ApiRecommendationPage, SlottingRecommendationPageRequest,
    SlottingRecommendationReason as ApiReason, SlottingRecommendationResponse,
    SlottingRecommendationStatus as ApiStatus, SlottingRunResponse, SlottingScoreEvidenceResponse,
    SlottingScoreResponse,
};
use wareboxes_application::slotting::{
    AcceptSlottingRecommendationCommand, ConfigureSlottingProfileCommand,
    DismissSlottingRecommendationCommand, RunSlottingCommand, SlottingProfileCursor,
    SlottingProfilePageQuery, SlottingProfileReadModel, SlottingRecommendationCursor,
    SlottingRecommendationPageQuery, SlottingRecommendationReadModel, SlottingRunReadModel,
};
use wareboxes_domain::{
    SlottingAdvisoryMode, SlottingDismissalReason, SlottingProfileDefinition, SlottingProfileId,
    SlottingProfileRevision, SlottingRecommendationId, SlottingRecommendationReason,
    SlottingRecommendationStatus, SlottingRunId,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const READ_PERMISSION: &str = "wms";
const SUPERVISOR_PERMISSION: &str = "wms_supervisor";
const PROFILE_CURSOR_PREFIX: &str = "slp1.";
const RECOMMENDATION_CURSOR_PREFIX: &str = "slr1.";

pub async fn list_profiles(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<SlottingProfilePageRequest>,
) -> V1Result<Json<ApiProfilePage>> {
    user.require_permission(&state.db, READ_PERMISSION).await?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| user.require_inventory_owner(id))
        .transpose()?;
    let facility_id = request
        .facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_profile_cursor(cursor, &request))
        .transpose()?;
    let page = repo::slotting::profile_page(
        &state.db,
        &user.tenant,
        SlottingProfilePageQuery {
            inventory_owner_id,
            facility_id,
            include_history: request.include_history,
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_profile_cursor(cursor, &request))
        .transpose()?;
    Ok(Json(ApiProfilePage::new(
        page.items
            .into_iter()
            .map(map_profile)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn configure_profile(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ConfigureSlottingProfileRequest>,
) -> V1Result<Json<SlottingProfileResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let inventory_owner_id = user.require_inventory_owner(body.inventory_owner_id)?;
    let facility_id = user.require_facility(body.facility_id)?;
    let command = ConfigureSlottingProfileCommand {
        definition: SlottingProfileDefinition {
            tenant_id: user.tenant.tenant_id,
            inventory_owner_id,
            facility_id,
            mode: map_mode(body.mode),
            demand_lookback_days: body.demand_lookback_days,
            demand_weight: body.demand_weight,
            travel_weight: body.travel_weight,
            activity_weight: body.activity_weight,
            minimum_demand_quantity: body.minimum_demand_quantity,
            max_recommendations: body.max_recommendations,
            default_task_priority: body.default_task_priority,
        },
        expected_revision: body
            .expected_revision
            .map(|revision| SlottingProfileRevision::new(revision.get()).map_err(validation))
            .transpose()?,
    };
    let result = repo::slotting::configure_profile(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_profile(result)?))
}

pub async fn run(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<RunSlottingRequest>,
) -> V1Result<Json<SlottingRunResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let command = RunSlottingCommand {
        tenant_id: user.tenant.tenant_id,
        inventory_owner_id: user.require_inventory_owner(body.inventory_owner_id)?,
        facility_id: user.require_facility(body.facility_id)?,
        expected_profile_revision: SlottingProfileRevision::new(
            body.expected_profile_revision.get(),
        )
        .map_err(validation)?,
    };
    let result = repo::slotting::run_slotting(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_run(result)?))
}

pub async fn list_recommendations(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<SlottingRecommendationPageRequest>,
) -> V1Result<Json<ApiRecommendationPage>> {
    user.require_permission(&state.db, READ_PERMISSION).await?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(|id| user.require_inventory_owner(id))
        .transpose()?;
    let facility_id = request
        .facility_id
        .map(|id| user.require_facility(id))
        .transpose()?;
    let slotting_run_id = request
        .slotting_run_id
        .map(|id| SlottingRunId::new(id).map_err(validation))
        .transpose()?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_recommendation_cursor(cursor, &request))
        .transpose()?;
    let page = repo::slotting::recommendation_page(
        &state.db,
        &user.tenant,
        SlottingRecommendationPageQuery {
            inventory_owner_id,
            facility_id,
            slotting_run_id,
            status: request.status.map(map_status),
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_recommendation_cursor(cursor, &request))
        .transpose()?;
    Ok(Json(ApiRecommendationPage::new(
        page.items
            .into_iter()
            .map(map_recommendation)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    )))
}

pub async fn accept(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(recommendation_id): Path<i64>,
    Json(body): Json<AcceptSlottingRecommendationRequest>,
) -> V1Result<Json<SlottingRecommendationResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let command = AcceptSlottingRecommendationCommand {
        recommendation_id: SlottingRecommendationId::new(recommendation_id).map_err(validation)?,
        expected_revision: body.expected_revision.get(),
        task_priority: body.task_priority,
        instructions: body.instructions,
    };
    let result = repo::slotting::accept_recommendation(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_recommendation(result)?))
}

pub async fn dismiss(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(recommendation_id): Path<i64>,
    Json(body): Json<DismissSlottingRecommendationRequest>,
) -> V1Result<Json<SlottingRecommendationResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let command = DismissSlottingRecommendationCommand {
        recommendation_id: SlottingRecommendationId::new(recommendation_id).map_err(validation)?,
        expected_revision: body.expected_revision.get(),
        reason: map_dismissal(body.reason),
        note: body.note,
    };
    let result = repo::slotting::dismiss_recommendation(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_recommendation(result)?))
}

fn map_profile(value: SlottingProfileReadModel) -> V1Result<SlottingProfileResponse> {
    Ok(SlottingProfileResponse {
        slotting_profile_id: value.slotting_profile_id.get(),
        inventory_owner_id: value.definition.inventory_owner_id.get(),
        facility_id: value.definition.facility_id.get(),
        mode: map_mode_to_api(value.definition.mode),
        demand_lookback_days: value.definition.demand_lookback_days,
        demand_weight: value.definition.demand_weight,
        travel_weight: value.definition.travel_weight,
        activity_weight: value.definition.activity_weight,
        minimum_demand_quantity: value.definition.minimum_demand_quantity,
        max_recommendations: value.definition.max_recommendations,
        default_task_priority: value.definition.default_task_priority,
        revision: Revision::new(value.revision.get()).map_err(invalid_result)?,
        configured_by: value.configured_by.get(),
        configured_at: value.configured_at.to_rfc3339(),
        effective_from: value.effective_from.to_rfc3339(),
        supersedes_slotting_profile_id: value
            .supersedes_slotting_profile_id
            .map(SlottingProfileId::get),
        effective_to: value.effective_to.map(|at| at.to_rfc3339()),
    })
}

fn map_run(value: SlottingRunReadModel) -> V1Result<SlottingRunResponse> {
    Ok(SlottingRunResponse {
        slotting_run_id: value.slotting_run_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        facility_id: value.facility_id.get(),
        slotting_profile_id: value.slotting_profile_id.get(),
        profile_revision: Revision::new(value.profile_revision.get()).map_err(invalid_result)?,
        demand_window_started_at: value.demand_window_started_at.to_rfc3339(),
        input_snapshot_at: value.input_snapshot_at.to_rfc3339(),
        configuration_snapshot: value.configuration_snapshot,
        candidate_count: value.candidate_count,
        recommendation_count: value.recommendation_count,
        generated_by: value.generated_by.get(),
        generated_at: value.generated_at.to_rfc3339(),
    })
}

fn map_recommendation(
    value: SlottingRecommendationReadModel,
) -> V1Result<SlottingRecommendationResponse> {
    Ok(SlottingRecommendationResponse {
        slotting_recommendation_id: value.slotting_recommendation_id.get(),
        slotting_run_id: value.slotting_run_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        facility_id: value.facility_id.get(),
        source_inventory_balance_id: value.source_inventory_balance_id.get(),
        item_id: value.item_id,
        item_description: value.item_description,
        uom: value.uom,
        source_location_id: value.source_location_id.get(),
        source_location_label: value.source_location_label,
        source_zone_code: value.source_zone_code,
        destination_location_id: value.destination_location_id.get(),
        destination_location_label: value.destination_location_label,
        destination_zone_code: value.destination_zone_code,
        recommended_quantity: value.recommended_quantity,
        reason: map_reason_to_api(value.reason),
        score: SlottingScoreResponse {
            demand_component: value.score.demand_component,
            travel_component: value.score.travel_component,
            activity_component: value.score.activity_component,
            total: value.score.total,
        },
        evidence: SlottingScoreEvidenceResponse {
            outstanding_demand_quantity: value.evidence.outstanding_demand_quantity,
            historical_pick_quantity: value.evidence.historical_pick_quantity,
            historical_pick_count: value.evidence.historical_pick_count,
            source_travel_sequence: value.evidence.source_travel_sequence,
            destination_travel_sequence: value.evidence.destination_travel_sequence,
            source_on_hand: value.evidence.source_on_hand,
            source_movable_quantity: value.evidence.source_movable_quantity,
            destination_on_hand: value.evidence.destination_on_hand,
            destination_inbound_planned_quantity: value
                .evidence
                .destination_inbound_planned_quantity,
            destination_capacity: value.evidence.destination_capacity,
            recommended_quantity: value.evidence.recommended_quantity,
        },
        item_storage_policy_id: value.item_storage_policy_id,
        item_storage_policy_revision: value.item_storage_policy_revision,
        status: map_status_to_api(value.status),
        revision: Revision::new(value.revision).map_err(invalid_result)?,
        decided_by: value.decided_by.map(|id| id.get()),
        decided_at: value.decided_at.map(|at| at.to_rfc3339()),
        dismissal_reason: value.dismissal_reason.map(map_dismissal_to_api),
        dismissal_note: value.dismissal_note,
        inventory_relocation_task_id: value.inventory_relocation_task_id,
        created_at: value.created_at.to_rfc3339(),
    })
}

const fn map_mode(value: ApiMode) -> SlottingAdvisoryMode {
    match value {
        ApiMode::Enabled => SlottingAdvisoryMode::Enabled,
        ApiMode::Disabled => SlottingAdvisoryMode::Disabled,
    }
}

const fn map_mode_to_api(value: SlottingAdvisoryMode) -> ApiMode {
    match value {
        SlottingAdvisoryMode::Enabled => ApiMode::Enabled,
        SlottingAdvisoryMode::Disabled => ApiMode::Disabled,
    }
}

const fn map_reason_to_api(value: SlottingRecommendationReason) -> ApiReason {
    match value {
        SlottingRecommendationReason::ForwardPickDemand => ApiReason::ForwardPickDemand,
        SlottingRecommendationReason::TravelReduction => ApiReason::TravelReduction,
        SlottingRecommendationReason::CapacityRebalance => ApiReason::CapacityRebalance,
    }
}

const fn map_status(value: ApiStatus) -> SlottingRecommendationStatus {
    match value {
        ApiStatus::Pending => SlottingRecommendationStatus::Pending,
        ApiStatus::Accepted => SlottingRecommendationStatus::Accepted,
        ApiStatus::Dismissed => SlottingRecommendationStatus::Dismissed,
    }
}

const fn map_status_to_api(value: SlottingRecommendationStatus) -> ApiStatus {
    match value {
        SlottingRecommendationStatus::Pending => ApiStatus::Pending,
        SlottingRecommendationStatus::Accepted => ApiStatus::Accepted,
        SlottingRecommendationStatus::Dismissed => ApiStatus::Dismissed,
    }
}

const fn map_dismissal(value: ApiDismissal) -> SlottingDismissalReason {
    match value {
        ApiDismissal::CapacityChanged => SlottingDismissalReason::CapacityChanged,
        ApiDismissal::OperationalConstraint => SlottingDismissalReason::OperationalConstraint,
        ApiDismissal::ItemStrategy => SlottingDismissalReason::ItemStrategy,
        ApiDismissal::StaleEvidence => SlottingDismissalReason::StaleEvidence,
        ApiDismissal::DuplicateWork => SlottingDismissalReason::DuplicateWork,
        ApiDismissal::Other => SlottingDismissalReason::Other,
    }
}

const fn map_dismissal_to_api(value: SlottingDismissalReason) -> ApiDismissal {
    match value {
        SlottingDismissalReason::CapacityChanged => ApiDismissal::CapacityChanged,
        SlottingDismissalReason::OperationalConstraint => ApiDismissal::OperationalConstraint,
        SlottingDismissalReason::ItemStrategy => ApiDismissal::ItemStrategy,
        SlottingDismissalReason::StaleEvidence => ApiDismissal::StaleEvidence,
        SlottingDismissalReason::DuplicateWork => ApiDismissal::DuplicateWork,
        SlottingDismissalReason::Other => ApiDismissal::Other,
    }
}

fn profile_filter(request: &SlottingProfilePageRequest) -> String {
    format!(
        "{:016x}.{:016x}.{}",
        request.inventory_owner_id.unwrap_or_default(),
        request.facility_id.unwrap_or_default(),
        u8::from(request.include_history)
    )
}

fn encode_profile_cursor(
    cursor: SlottingProfileCursor,
    request: &SlottingProfilePageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{PROFILE_CURSOR_PREFIX}{}.{:016x}.{:016x}",
        profile_filter(request),
        cursor.after_configured_at.timestamp_micros(),
        cursor.after_slotting_profile_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid slotting profile cursor"))
}

fn decode_profile_cursor(
    cursor: &OpaqueCursor,
    request: &SlottingProfilePageRequest,
) -> V1Result<SlottingProfileCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(PROFILE_CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("slotting profile"))?;
    let mut parts = encoded.rsplitn(3, '.');
    let id = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("slotting profile"))?;
    let micros = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("slotting profile"))?;
    let filter = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("slotting profile"))?;
    if filter != profile_filter(request) {
        return Err(V1Error::invalid_cursor_for("slotting profile"));
    }
    let micros = i64::from_str_radix(micros, 16)
        .map_err(|_| V1Error::invalid_cursor_for("slotting profile"))?;
    let after_configured_at = chrono::DateTime::from_timestamp_micros(micros)
        .ok_or_else(|| V1Error::invalid_cursor_for("slotting profile"))?;
    Ok(SlottingProfileCursor {
        after_configured_at,
        after_slotting_profile_id: SlottingProfileId::new(
            i64::from_str_radix(id, 16)
                .map_err(|_| V1Error::invalid_cursor_for("slotting profile"))?,
        )
        .map_err(|_| V1Error::invalid_cursor_for("slotting profile"))?,
    })
}

fn recommendation_filter(request: &SlottingRecommendationPageRequest) -> String {
    format!(
        "{:016x}.{:016x}.{:016x}.{}",
        request.inventory_owner_id.unwrap_or_default(),
        request.facility_id.unwrap_or_default(),
        request.slotting_run_id.unwrap_or_default(),
        request.status.map_or("all", |status| match status {
            ApiStatus::Pending => "pending",
            ApiStatus::Accepted => "accepted",
            ApiStatus::Dismissed => "dismissed",
        })
    )
}

fn encode_recommendation_cursor(
    cursor: SlottingRecommendationCursor,
    request: &SlottingRecommendationPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{RECOMMENDATION_CURSOR_PREFIX}{}.{:016x}.{:016x}",
        recommendation_filter(request),
        cursor.after_score,
        cursor.after_slotting_recommendation_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid slotting recommendation cursor"))
}

fn decode_recommendation_cursor(
    cursor: &OpaqueCursor,
    request: &SlottingRecommendationPageRequest,
) -> V1Result<SlottingRecommendationCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(RECOMMENDATION_CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("slotting recommendation"))?;
    let mut parts = encoded.rsplitn(3, '.');
    let id = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("slotting recommendation"))?;
    let score = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("slotting recommendation"))?;
    let filter = parts
        .next()
        .ok_or_else(|| V1Error::invalid_cursor_for("slotting recommendation"))?;
    if filter != recommendation_filter(request) {
        return Err(V1Error::invalid_cursor_for("slotting recommendation"));
    }
    Ok(SlottingRecommendationCursor {
        after_score: i64::from_str_radix(score, 16)
            .map_err(|_| V1Error::invalid_cursor_for("slotting recommendation"))?,
        after_slotting_recommendation_id: SlottingRecommendationId::new(
            i64::from_str_radix(id, 16)
                .map_err(|_| V1Error::invalid_cursor_for("slotting recommendation"))?,
        )
        .map_err(|_| V1Error::invalid_cursor_for("slotting recommendation"))?,
    })
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    V1Error::internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::PageLimit;

    #[test]
    fn recommendation_cursor_round_trips_and_is_filter_bound() {
        let request = SlottingRecommendationPageRequest {
            inventory_owner_id: Some(2),
            facility_id: Some(3),
            slotting_run_id: None,
            status: Some(ApiStatus::Pending),
            cursor: None,
            limit: PageLimit::default(),
        };
        let cursor = SlottingRecommendationCursor {
            after_score: 42,
            after_slotting_recommendation_id: SlottingRecommendationId::new(9).unwrap(),
        };
        let encoded = encode_recommendation_cursor(cursor, &request).unwrap();
        assert_eq!(
            decode_recommendation_cursor(&encoded, &request).unwrap(),
            cursor
        );
        let mut changed = request;
        changed.status = Some(ApiStatus::Dismissed);
        assert!(decode_recommendation_cursor(&encoded, &changed).is_err());
    }
}
