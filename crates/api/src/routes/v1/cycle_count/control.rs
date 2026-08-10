//! Supervisor cycle-count policy and variance-review transport.

use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ConfigureCycleCountPolicyRequest, ConfigureCycleCountPolicyResponse,
    CycleCountPolicyPage as ApiPolicyPage, CycleCountPolicyPageRequest, CycleCountPolicyResponse,
    CycleCountVarianceDecision as ApiVarianceDecision, CycleCountVariancePage as ApiVariancePage,
    CycleCountVariancePageRequest, CycleCountVarianceReason as ApiVarianceReason,
    CycleCountVarianceResponse, CycleCountVarianceStatus as ApiVarianceStatus,
    CycleCountVarianceStockResponse, DecideCycleCountVarianceRequest,
    DecideCycleCountVarianceResponse, InventoryBalanceStatus, OpaqueCursor,
};
use wareboxes_application::cycle_count_control::{
    ConfigureCycleCountPolicyCommand, ConfigureCycleCountPolicyResult,
    CycleCountPolicyPage as ApplicationPolicyPage, CycleCountPolicyPageQuery,
    CycleCountPolicyReadModel, CycleCountVariancePage as ApplicationVariancePage,
    CycleCountVariancePageQuery, CycleCountVarianceReadModel, DecideCycleCountVarianceCommand,
    DecideCycleCountVarianceResult,
};
use wareboxes_domain::{
    CycleCountDisposition, CycleCountPolicyId, CycleCountPolicyRevision, CycleCountTolerancePolicy,
    CycleCountVarianceDecision, CycleCountVarianceDecisionDetails, CycleCountVarianceId,
    CycleCountVarianceNote, CycleCountVarianceReason, CycleCountVarianceRevision,
    CycleCountVarianceStatus, FacilityId, InventoryOwnerId,
};

use super::{
    cursor_parts, domain_validation, opaque_cursor, optional_id, parse_hex_i64,
    parse_optional_facility, parse_optional_owner, require_page_limit, revision_to_api,
    SUPERVISOR_PERMISSION,
};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::routes::v1::error::{V1Error, V1Result};
use crate::state::AppState;

const POLICY_CURSOR_PREFIX: &str = "cp1.";
const VARIANCE_CURSOR_PREFIX: &str = "cv1.";

pub async fn configure_policy(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<ConfigureCycleCountPolicyRequest>,
) -> V1Result<Json<ConfigureCycleCountPolicyResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let inventory_owner_id = user.require_inventory_owner(body.inventory_owner_id)?;
    let facility_id = user.require_facility(body.facility_id)?;
    let policy = CycleCountTolerancePolicy::new(
        body.absolute_tolerance_quantity,
        body.percentage_tolerance_basis_points,
        body.automatic_recount_limit,
    )
    .map_err(domain_validation)?;
    let command = ConfigureCycleCountPolicyCommand {
        inventory_owner_id,
        facility_id,
        policy,
        expected_revision: body
            .expected_revision
            .map(|revision| CycleCountPolicyRevision::new(revision.get()))
            .transpose()
            .map_err(domain_validation)?,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::tasks::configure_cycle_count_policy_in_scope(
        &state.db,
        &user.tenant,
        &context,
        &command,
    )
    .await?;
    Ok(Json(map_configured_policy(result)?))
}

pub async fn policies(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<CycleCountPolicyPageRequest>,
) -> V1Result<Json<ApiPolicyPage>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    require_page_limit(query.limit.get())?;
    let facility_id = query
        .facility_id
        .map(|value| user.require_facility(value))
        .transpose()?;
    let inventory_owner_id = query
        .inventory_owner_id
        .map(|value| user.require_inventory_owner(value))
        .transpose()?;
    let filters = PolicyCursorFilters {
        facility_id,
        inventory_owner_id,
    };
    let cursor = query
        .cursor
        .as_ref()
        .map(decode_policy_cursor)
        .transpose()?;
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.filters != filters)
    {
        return Err(V1Error::invalid_cursor_for("cycle-count policies"));
    }
    let page = repo::tasks::cycle_count_policy_page(
        &state.db,
        &user.tenant,
        CycleCountPolicyPageQuery {
            facility_id,
            inventory_owner_id,
            after_policy_id: cursor.map(|cursor| cursor.after_policy_id),
            limit: query.limit.get(),
        },
    )
    .await?;
    Ok(Json(map_policy_page(page, filters)?))
}

pub async fn variances(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<CycleCountVariancePageRequest>,
) -> V1Result<Json<ApiVariancePage>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    require_page_limit(query.limit.get())?;
    let facility_id = query
        .facility_id
        .map(|value| user.require_facility(value))
        .transpose()?;
    let inventory_owner_id = query
        .inventory_owner_id
        .map(|value| user.require_inventory_owner(value))
        .transpose()?;
    let status = query.status.map(map_variance_status_to_domain);
    let filters = VarianceCursorFilters {
        facility_id,
        inventory_owner_id,
        status,
    };
    let cursor = query
        .cursor
        .as_ref()
        .map(decode_variance_cursor)
        .transpose()?;
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.filters != filters)
    {
        return Err(V1Error::invalid_cursor_for("cycle-count variances"));
    }
    let page = repo::tasks::cycle_count_variance_page(
        &state.db,
        &user.tenant,
        CycleCountVariancePageQuery {
            facility_id,
            inventory_owner_id,
            status,
            after_variance_id: cursor.map(|cursor| cursor.after_variance_id),
            limit: query.limit.get(),
        },
    )
    .await?;
    Ok(Json(map_variance_page(page, filters)?))
}

pub async fn decide_variance(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(variance_id): Path<i64>,
    Json(body): Json<DecideCycleCountVarianceRequest>,
) -> V1Result<Json<DecideCycleCountVarianceResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    let variance_id = CycleCountVarianceId::new(variance_id).map_err(domain_validation)?;
    let note = body
        .note
        .map(CycleCountVarianceNote::new)
        .transpose()
        .map_err(domain_validation)?;
    let details = CycleCountVarianceDecisionDetails::new(
        map_variance_decision_to_domain(body.decision),
        map_variance_reason_to_domain(body.reason),
        note,
    )
    .map_err(domain_validation)?;
    let command = DecideCycleCountVarianceCommand {
        variance_id,
        expected_revision: CycleCountVarianceRevision::new(body.expected_revision.get())
            .map_err(domain_validation)?,
        details,
    };
    let context = user.command_context(&idempotency_key);
    let result = repo::tasks::decide_cycle_count_variance_in_scope(
        &state.db,
        &user.tenant,
        &context,
        &command,
    )
    .await?;
    Ok(Json(map_variance_decision(result)?))
}

#[cfg_attr(not(feature = "ssr"), allow(dead_code))]
pub(crate) async fn pages_for_access(
    state: &AppState,
    access: &wareboxes_core::models::TenantAccess,
    limit: u16,
) -> AppResult<(ApiPolicyPage, ApiVariancePage)> {
    let policy_filters = PolicyCursorFilters::default();
    let variance_filters = VarianceCursorFilters::default();
    let (policies, variances) = tokio::try_join!(
        repo::tasks::cycle_count_policy_page(
            &state.db,
            access,
            CycleCountPolicyPageQuery {
                facility_id: None,
                inventory_owner_id: None,
                after_policy_id: None,
                limit,
            },
        ),
        repo::tasks::cycle_count_variance_page(
            &state.db,
            access,
            CycleCountVariancePageQuery {
                facility_id: None,
                inventory_owner_id: None,
                status: None,
                after_variance_id: None,
                limit,
            },
        ),
    )?;
    Ok((
        map_policy_page(policies, policy_filters)?,
        map_variance_page(variances, variance_filters)?,
    ))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PolicyCursorFilters {
    facility_id: Option<FacilityId>,
    inventory_owner_id: Option<InventoryOwnerId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PolicyCursor {
    filters: PolicyCursorFilters,
    after_policy_id: CycleCountPolicyId,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct VarianceCursorFilters {
    facility_id: Option<FacilityId>,
    inventory_owner_id: Option<InventoryOwnerId>,
    status: Option<CycleCountVarianceStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VarianceCursor {
    filters: VarianceCursorFilters,
    after_variance_id: CycleCountVarianceId,
}

fn map_policy_page(
    page: ApplicationPolicyPage,
    filters: PolicyCursorFilters,
) -> AppResult<ApiPolicyPage> {
    let items = page
        .items
        .into_iter()
        .map(map_policy)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = page
        .next_after_policy_id
        .map(|after_policy_id| {
            encode_policy_cursor(PolicyCursor {
                filters,
                after_policy_id,
            })
        })
        .transpose()?;
    Ok(ApiPolicyPage::new(items, next_cursor))
}

fn map_policy(policy: CycleCountPolicyReadModel) -> AppResult<CycleCountPolicyResponse> {
    Ok(CycleCountPolicyResponse {
        policy_id: policy.policy_id.get(),
        inventory_owner_id: policy.inventory_owner_id.get(),
        inventory_owner_name: policy.inventory_owner_name,
        facility_id: policy.facility_id.get(),
        facility_name: policy.facility_name,
        absolute_tolerance_quantity: policy.policy.absolute_tolerance_quantity(),
        percentage_tolerance_basis_points: policy.policy.percentage_tolerance_basis_points(),
        automatic_recount_limit: policy.policy.automatic_recount_limit(),
        revision: revision_to_api(policy.revision.get())?,
        configured_by: policy.configured_by.get(),
        configured_at: policy.configured_at.to_rfc3339(),
    })
}

fn map_configured_policy(
    result: ConfigureCycleCountPolicyResult,
) -> V1Result<ConfigureCycleCountPolicyResponse> {
    Ok(ConfigureCycleCountPolicyResponse {
        policy_id: result.policy_id.get(),
        inventory_owner_id: result.inventory_owner_id.get(),
        facility_id: result.facility_id.get(),
        absolute_tolerance_quantity: result.policy.absolute_tolerance_quantity(),
        percentage_tolerance_basis_points: result.policy.percentage_tolerance_basis_points(),
        automatic_recount_limit: result.policy.automatic_recount_limit(),
        previous_revision: result
            .previous_revision
            .map(|revision| revision_to_api(revision.get()))
            .transpose()?,
        revision: revision_to_api(result.revision.get())?,
        configured_by: result.configured_by.get(),
        configured_at: result.configured_at.to_rfc3339(),
    })
}

fn map_variance_page(
    page: ApplicationVariancePage,
    filters: VarianceCursorFilters,
) -> AppResult<ApiVariancePage> {
    let items = page
        .items
        .into_iter()
        .map(map_variance)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = page
        .next_after_variance_id
        .map(|after_variance_id| {
            encode_variance_cursor(VarianceCursor {
                filters,
                after_variance_id,
            })
        })
        .transpose()?;
    Ok(ApiVariancePage::new(items, next_cursor))
}

fn map_variance(variance: CycleCountVarianceReadModel) -> AppResult<CycleCountVarianceResponse> {
    Ok(CycleCountVarianceResponse {
        variance_id: variance.variance_id.get(),
        revision: revision_to_api(variance.revision.get())?,
        status: map_variance_status_to_api(variance.status),
        inventory_owner_id: variance.inventory_owner_id.get(),
        inventory_owner_name: variance.inventory_owner_name,
        facility_id: variance.facility_id.get(),
        facility_name: variance.facility_name,
        stock: CycleCountVarianceStockResponse {
            inventory_balance_id: variance.stock.inventory_balance_id.get(),
            location_id: variance.stock.location_id.get(),
            location_barcode: variance.stock.location_barcode,
            location_name: variance.stock.location_name,
            item_id: variance.stock.item_id.get(),
            item_description: variance.stock.item_description,
            primary_sku: variance.stock.primary_sku,
            license_plate_barcode: variance.stock.license_plate_barcode,
            uom: variance.stock.uom,
            lot: variance.stock.lot,
            serial: variance.stock.serial,
            inventory_status: parse_api_inventory_status(&variance.stock.inventory_status)?,
        },
        policy_id: variance.policy_id.get(),
        policy_revision: revision_to_api(variance.policy_revision.get())?,
        absolute_tolerance_quantity: variance.policy.absolute_tolerance_quantity(),
        percentage_tolerance_basis_points: variance.policy.percentage_tolerance_basis_points(),
        automatic_recount_limit: variance.policy.automatic_recount_limit(),
        latest_task_id: variance.latest_task_id,
        latest_attempt_sequence: variance.latest_attempt_sequence,
        automatic_recounts_used: variance.automatic_recounts_used,
        system_quantity: variance.system_quantity,
        counted_quantity: variance.counted_quantity,
        variance_quantity: variance.variance_quantity,
        allowed_variance_quantity: variance.allowed_variance_quantity,
        inventory_transaction_id: variance.inventory_transaction_id,
        created_at: variance.created_at.to_rfc3339(),
        modified_at: variance.modified_at.to_rfc3339(),
    })
}

fn map_variance_decision(
    result: DecideCycleCountVarianceResult,
) -> V1Result<DecideCycleCountVarianceResponse> {
    Ok(DecideCycleCountVarianceResponse {
        decision_id: result.decision_id.get(),
        variance_id: result.variance_id.get(),
        previous_status: map_variance_status_to_api(result.previous_status),
        status: map_variance_status_to_api(result.status),
        previous_revision: revision_to_api(result.previous_revision.get())?,
        revision: revision_to_api(result.revision.get())?,
        disposition: map_disposition_to_api(result.disposition),
        next_task_id: result.next_task_id,
        inventory_transaction_id: result.inventory_transaction_id,
        decided_by: result.decided_by.get(),
        decided_at: result.decided_at.to_rfc3339(),
    })
}

const fn map_variance_status_to_domain(status: ApiVarianceStatus) -> CycleCountVarianceStatus {
    match status {
        ApiVarianceStatus::AwaitingRecount => CycleCountVarianceStatus::AwaitingRecount,
        ApiVarianceStatus::AwaitingApproval => CycleCountVarianceStatus::AwaitingApproval,
        ApiVarianceStatus::Posted => CycleCountVarianceStatus::Posted,
    }
}

const fn map_variance_status_to_api(status: CycleCountVarianceStatus) -> ApiVarianceStatus {
    match status {
        CycleCountVarianceStatus::AwaitingRecount => ApiVarianceStatus::AwaitingRecount,
        CycleCountVarianceStatus::AwaitingApproval => ApiVarianceStatus::AwaitingApproval,
        CycleCountVarianceStatus::Posted => ApiVarianceStatus::Posted,
    }
}

const fn map_variance_decision_to_domain(
    decision: ApiVarianceDecision,
) -> CycleCountVarianceDecision {
    match decision {
        ApiVarianceDecision::ApproveAdjustment => CycleCountVarianceDecision::ApproveAdjustment,
        ApiVarianceDecision::RequestRecount => CycleCountVarianceDecision::RequestRecount,
    }
}

const fn map_variance_reason_to_domain(reason: ApiVarianceReason) -> CycleCountVarianceReason {
    match reason {
        ApiVarianceReason::VerifiedPhysicalCount => CycleCountVarianceReason::VerifiedPhysicalCount,
        ApiVarianceReason::PackagingOrUomIssue => CycleCountVarianceReason::PackagingOrUomIssue,
        ApiVarianceReason::ReceivingOrShippingTiming => {
            CycleCountVarianceReason::ReceivingOrShippingTiming
        }
        ApiVarianceReason::SuspectedMiscount => CycleCountVarianceReason::SuspectedMiscount,
        ApiVarianceReason::Other => CycleCountVarianceReason::Other,
    }
}

const fn map_disposition_to_api(
    disposition: CycleCountDisposition,
) -> wareboxes_api_contract::v1::CycleCountDisposition {
    match disposition {
        CycleCountDisposition::Posted => wareboxes_api_contract::v1::CycleCountDisposition::Posted,
        CycleCountDisposition::RecountRequired => {
            wareboxes_api_contract::v1::CycleCountDisposition::RecountRequired
        }
        CycleCountDisposition::ApprovalRequired => {
            wareboxes_api_contract::v1::CycleCountDisposition::ApprovalRequired
        }
    }
}

fn parse_api_inventory_status(value: &str) -> AppResult<InventoryBalanceStatus> {
    match value {
        "available" => Ok(InventoryBalanceStatus::Available),
        "hold" => Ok(InventoryBalanceStatus::Hold),
        "damaged" => Ok(InventoryBalanceStatus::Damaged),
        "quarantine" => Ok(InventoryBalanceStatus::Quarantine),
        _ => Err(AppError::internal(format!(
            "invalid cycle count inventory status in database: {value}"
        ))),
    }
}

fn encode_policy_cursor(cursor: PolicyCursor) -> AppResult<OpaqueCursor> {
    opaque_cursor(format!(
        "{POLICY_CURSOR_PREFIX}{}.{}.{:016x}",
        optional_id(cursor.filters.facility_id.map(FacilityId::get)),
        optional_id(cursor.filters.inventory_owner_id.map(InventoryOwnerId::get)),
        cursor.after_policy_id.get(),
    ))
}

fn decode_policy_cursor(cursor: &OpaqueCursor) -> V1Result<PolicyCursor> {
    let parts = cursor_parts(cursor, POLICY_CURSOR_PREFIX, 3, "cycle-count policies")?;
    Ok(PolicyCursor {
        filters: PolicyCursorFilters {
            facility_id: parse_optional_facility(parts[0], "cycle-count policies")?,
            inventory_owner_id: parse_optional_owner(parts[1], "cycle-count policies")?,
        },
        after_policy_id: CycleCountPolicyId::new(parse_hex_i64(parts[2], "cycle-count policies")?)
            .map_err(|_| V1Error::invalid_cursor_for("cycle-count policies"))?,
    })
}

fn encode_variance_cursor(cursor: VarianceCursor) -> AppResult<OpaqueCursor> {
    opaque_cursor(format!(
        "{VARIANCE_CURSOR_PREFIX}{}.{}.{}.{:016x}",
        optional_id(cursor.filters.facility_id.map(FacilityId::get)),
        optional_id(cursor.filters.inventory_owner_id.map(InventoryOwnerId::get)),
        variance_status_code(cursor.filters.status),
        cursor.after_variance_id.get(),
    ))
}

fn decode_variance_cursor(cursor: &OpaqueCursor) -> V1Result<VarianceCursor> {
    let parts = cursor_parts(cursor, VARIANCE_CURSOR_PREFIX, 4, "cycle-count variances")?;
    Ok(VarianceCursor {
        filters: VarianceCursorFilters {
            facility_id: parse_optional_facility(parts[0], "cycle-count variances")?,
            inventory_owner_id: parse_optional_owner(parts[1], "cycle-count variances")?,
            status: parse_variance_status(parts[2])?,
        },
        after_variance_id: CycleCountVarianceId::new(parse_hex_i64(
            parts[3],
            "cycle-count variances",
        )?)
        .map_err(|_| V1Error::invalid_cursor_for("cycle-count variances"))?,
    })
}

const fn variance_status_code(status: Option<CycleCountVarianceStatus>) -> &'static str {
    match status {
        None => "a",
        Some(CycleCountVarianceStatus::AwaitingRecount) => "r",
        Some(CycleCountVarianceStatus::AwaitingApproval) => "v",
        Some(CycleCountVarianceStatus::Posted) => "p",
    }
}

fn parse_variance_status(value: &str) -> V1Result<Option<CycleCountVarianceStatus>> {
    match value {
        "a" => Ok(None),
        "r" => Ok(Some(CycleCountVarianceStatus::AwaitingRecount)),
        "v" => Ok(Some(CycleCountVarianceStatus::AwaitingApproval)),
        "p" => Ok(Some(CycleCountVarianceStatus::Posted)),
        _ => Err(V1Error::invalid_cursor_for("cycle-count variances")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_cursor_binds_facility_and_owner_filters() {
        let filters = PolicyCursorFilters {
            facility_id: FacilityId::new(11).ok(),
            inventory_owner_id: InventoryOwnerId::new(17).ok(),
        };
        let cursor = encode_policy_cursor(PolicyCursor {
            filters,
            after_policy_id: CycleCountPolicyId::new(29).unwrap(),
        })
        .unwrap();
        let decoded = decode_policy_cursor(&cursor).unwrap();
        assert_eq!(decoded.filters, filters);
        assert_eq!(decoded.after_policy_id.get(), 29);
    }

    #[test]
    fn variance_cursor_binds_scope_and_status_filters() {
        let filters = VarianceCursorFilters {
            facility_id: FacilityId::new(3).ok(),
            inventory_owner_id: InventoryOwnerId::new(5).ok(),
            status: Some(CycleCountVarianceStatus::AwaitingApproval),
        };
        let cursor = encode_variance_cursor(VarianceCursor {
            filters,
            after_variance_id: CycleCountVarianceId::new(31).unwrap(),
        })
        .unwrap();
        let decoded = decode_variance_cursor(&cursor).unwrap();
        assert_eq!(decoded.filters, filters);
        assert_eq!(decoded.after_variance_id.get(), 31);
    }
}
