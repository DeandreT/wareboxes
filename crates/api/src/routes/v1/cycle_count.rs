use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ClaimCycleCountByIdRequest, ClaimNextCycleCountRequest, ConfirmCycleCountRequest,
    CreateCycleCountTaskRequest, CreateCycleCountTaskResponse, CycleCountCandidatePage,
    CycleCountCandidatePageRequest, CycleCountCandidateResponse,
    CycleCountCandidateSort as ApiCandidateSort, CycleCountClaimHeartbeatResponse,
    CycleCountClaimReleaseReason, CycleCountClaimReleaseResponse, CycleCountClaimResponse,
    CycleCountConfirmationResponse, CycleCountDisposition as ApiDisposition, CycleCountItem,
    CycleCountLocation, CycleCountQuantityResponse, CycleCountSortDirection as ApiSortDirection,
    CycleCountStock, CycleCountWorkPage, CycleCountWorkPageRequest, CycleCountWorkResponse,
    CycleCountWorkSort as ApiWorkSort, CycleCountWorkStatus as ApiWorkStatus,
    HeartbeatCycleCountClaimRequest, InventoryBalanceStatus, OpaqueCursor,
    ReleaseCycleCountClaimRequest, Revision,
};
use wareboxes_application::cycle_count::{
    CycleCountCandidatePage as ApplicationCandidatePage, CycleCountCandidateQuery,
    CycleCountCandidateReadModel, CycleCountCandidateSort, CycleCountCursor,
    CycleCountLocationReadModel, CycleCountSortDirection, CycleCountStockReadModel,
    CycleCountWorkPage as ApplicationWorkPage, CycleCountWorkQuery, CycleCountWorkReadModel,
    CycleCountWorkSort, CycleCountWorkStatus,
};
use wareboxes_application::inventory::InventoryBalanceStatus as ApplicationInventoryStatus;
use wareboxes_core::models::{
    CycleCountClaim, CycleCountClaimReleaseReason as CoreReleaseReason, InventoryStatus,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
pub(super) const SUPERVISOR_PERMISSION: &str = "wms_supervisor";
const MAX_BARCODE_LENGTH: usize = 200;
const MAX_NOTE_LENGTH: usize = 1_000;
const MAX_PAGE_LIMIT: u16 = 100;
const CANDIDATE_CURSOR_PREFIX: &str = "cc1.";
const WORK_CURSOR_PREFIX: &str = "cw1.";

mod control;
#[cfg_attr(not(feature = "ssr"), allow(unused_imports))]
pub(crate) use control::pages_for_access as control_pages_for_access;
pub use control::{configure_policy, decide_variance, policies, variances};

pub async fn candidates(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<CycleCountCandidatePageRequest>,
) -> V1Result<Json<CycleCountCandidatePage>> {
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
    let inventory_status = query
        .inventory_status
        .map(map_inventory_status_to_application);
    let sort = map_candidate_sort_to_application(query.sort);
    let direction = map_direction_to_application(query.direction);
    let cursor = query
        .cursor
        .as_ref()
        .map(decode_candidate_cursor)
        .transpose()?;
    let filters = CandidateCursorFilters {
        facility_id,
        inventory_owner_id,
        inventory_status,
        sort,
        direction,
    };
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.filters != filters)
    {
        return Err(V1Error::invalid_cursor_for("cycle-count candidates"));
    }
    let page = repo::tasks::cycle_count_candidate_page(
        &state.db,
        &user.tenant,
        CycleCountCandidateQuery {
            facility_id,
            inventory_owner_id,
            inventory_status,
            sort,
            direction,
            cursor: cursor.map(|value| value.cursor),
            limit: query.limit.get(),
        },
    )
    .await?;
    Ok(Json(map_candidate_page(page, filters)?))
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<CycleCountWorkPageRequest>,
) -> V1Result<Json<CycleCountWorkPage>> {
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
    let status = query.status.map(map_work_status_to_application);
    let sort = map_work_sort_to_application(query.sort);
    let direction = map_direction_to_application(query.direction);
    let cursor = query.cursor.as_ref().map(decode_work_cursor).transpose()?;
    let filters = WorkCursorFilters {
        facility_id,
        inventory_owner_id,
        status,
        sort,
        direction,
    };
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.filters != filters)
    {
        return Err(V1Error::invalid_cursor_for("cycle-count work"));
    }
    let page = repo::tasks::cycle_count_work_page(
        &state.db,
        &user.tenant,
        CycleCountWorkQuery {
            facility_id,
            inventory_owner_id,
            status,
            sort,
            direction,
            cursor: cursor.map(|value| value.cursor),
            limit: query.limit.get(),
        },
    )
    .await?;
    Ok(Json(map_work_page(page, filters)?))
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateCycleCountTaskRequest>,
) -> V1Result<Json<CreateCycleCountTaskResponse>> {
    user.require_permission(&state.db, SUPERVISOR_PERMISSION)
        .await?;
    require_positive(body.inventory_balance_id, "inventory balance ID")?;
    validate_note(body.note.as_deref())?;
    let command = user.command_context(&idempotency_key);
    let task_id = repo::tasks::create_inventory_balance_cycle_count_task_in_scope(
        &state.db,
        &user.tenant,
        &command,
        body.inventory_balance_id,
        body.note.as_deref(),
    )
    .await?;
    Ok(Json(CreateCycleCountTaskResponse { task_id }))
}

#[cfg_attr(not(feature = "ssr"), allow(dead_code))]
pub(crate) async fn pages_for_access(
    state: &AppState,
    access: &wareboxes_core::models::TenantAccess,
    limit: u16,
) -> AppResult<(CycleCountCandidatePage, CycleCountWorkPage)> {
    let candidate_filters = CandidateCursorFilters::default();
    let work_filters = WorkCursorFilters::default();
    let (candidates, work) = tokio::try_join!(
        repo::tasks::cycle_count_candidate_page(
            &state.db,
            access,
            CycleCountCandidateQuery {
                facility_id: None,
                inventory_owner_id: None,
                inventory_status: None,
                sort: CycleCountCandidateSort::default(),
                direction: CycleCountSortDirection::default(),
                cursor: None,
                limit,
            }
        ),
        repo::tasks::cycle_count_work_page(
            &state.db,
            access,
            CycleCountWorkQuery {
                facility_id: None,
                inventory_owner_id: None,
                status: None,
                sort: CycleCountWorkSort::default(),
                direction: CycleCountSortDirection::default(),
                cursor: None,
                limit,
            }
        ),
    )?;
    Ok((
        map_candidate_page(candidates, candidate_filters)?,
        map_work_page(work, work_filters)?,
    ))
}

pub async fn claim_next(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(_body): Json<ClaimNextCycleCountRequest>,
) -> V1Result<Json<Option<CycleCountClaimResponse>>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = user.command_context(&idempotency_key);
    let claim =
        repo::tasks::claim_next_cycle_count_in_scope(&state.db, &user.tenant, &command).await?;
    Ok(Json(claim.map(map_claim)))
}

pub async fn claim_by_id(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(_body): Json<ClaimCycleCountByIdRequest>,
) -> V1Result<Json<CycleCountClaimResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    let command = user.command_context(&idempotency_key);
    let claim =
        repo::tasks::claim_cycle_count_by_id_in_scope(&state.db, &user.tenant, &command, task_id)
            .await?;
    Ok(Json(map_claim(claim)))
}

pub async fn current(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> V1Result<Json<Option<CycleCountClaimResponse>>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let claim = repo::tasks::get_current_cycle_count_claim_in_scope(
        &state.db,
        &user.tenant,
        user.tenant.user_id.get(),
    )
    .await?;
    Ok(Json(claim.map(map_claim)))
}

pub async fn heartbeat(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(_body): Json<HeartbeatCycleCountClaimRequest>,
) -> V1Result<Json<CycleCountClaimHeartbeatResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    let command = user.command_context(&idempotency_key);
    let heartbeat = repo::tasks::heartbeat_cycle_count_claim_in_scope(
        &state.db,
        &user.tenant,
        &command,
        task_id,
    )
    .await?;
    Ok(Json(CycleCountClaimHeartbeatResponse {
        task_id: heartbeat.task_id,
        heartbeat_at: heartbeat.heartbeat_at.to_rfc3339(),
        lease_expires_at: heartbeat.lease_expires_at.to_rfc3339(),
    }))
}

pub async fn release(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(body): Json<ReleaseCycleCountClaimRequest>,
) -> V1Result<Json<CycleCountClaimReleaseResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    let command = user.command_context(&idempotency_key);
    let reason = map_release_reason(body.reason);
    let release = repo::tasks::release_cycle_count_claim_in_scope(
        &state.db,
        &user.tenant,
        &command,
        task_id,
        reason,
        body.note.as_deref(),
    )
    .await?;
    Ok(Json(CycleCountClaimReleaseResponse {
        task_id: release.task_id,
        released_at: release.released_at.to_rfc3339(),
        release_count: release.release_count,
        reason: body.reason,
        note: release.note,
    }))
}

pub async fn confirm(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(body): Json<ConfirmCycleCountRequest>,
) -> V1Result<Json<CycleCountConfirmationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    validate_barcode(&body.location_barcode, "location_barcode")?;
    validate_barcode(&body.item_barcode, "item_barcode")?;
    if let Some(barcode) = body.license_plate_barcode.as_deref() {
        validate_barcode(barcode, "license_plate_barcode")?;
    }
    if body.counted_quantity < 0 {
        return Err(invalid("counted_quantity cannot be negative"));
    }
    validate_note(body.note.as_deref())?;
    let command = user.command_context(&idempotency_key);
    let confirmation = repo::tasks::confirm_scanned_item_location_cycle_count_in_scope(
        &state.db,
        &user.tenant,
        &command,
        task_id,
        &body.location_barcode,
        &body.item_barcode,
        body.license_plate_barcode.as_deref(),
        body.counted_quantity,
        body.note.as_deref(),
    )
    .await?;
    Ok(Json(CycleCountConfirmationResponse {
        task_id: confirmation.task_id,
        inventory_owner_id: confirmation.inventory_owner_id.get(),
        facility_id: confirmation.facility_id,
        location_id: confirmation.location_id,
        inventory_balance_id: confirmation.inventory_balance_id,
        counted_quantity: confirmation.counted_quantity,
        variance_quantity: confirmation.variance_quantity,
        inventory_transaction_id: confirmation.inventory_transaction_id,
        disposition: match confirmation.disposition {
            wareboxes_domain::CycleCountDisposition::Posted => ApiDisposition::Posted,
            wareboxes_domain::CycleCountDisposition::RecountRequired => {
                ApiDisposition::RecountRequired
            }
            wareboxes_domain::CycleCountDisposition::ApprovalRequired => {
                ApiDisposition::ApprovalRequired
            }
        },
        variance_id: confirmation.variance_id.map(|id| id.get()),
        variance_revision: confirmation
            .variance_revision
            .map(|revision| {
                wareboxes_api_contract::v1::Revision::new(revision.get()).map_err(|_| {
                    V1Error::internal("cycle count produced an invalid variance revision")
                })
            })
            .transpose()?,
        next_recount_task_id: confirmation.next_recount_task_id,
        confirmed_by: confirmation.confirmed_by,
        confirmed_at: confirmation.confirmed_at.to_rfc3339(),
    }))
}

fn map_claim(claim: CycleCountClaim) -> CycleCountClaimResponse {
    CycleCountClaimResponse {
        task_id: claim.task_id,
        inventory_owner_id: claim.inventory_owner_id.get(),
        facility_id: claim.facility_id,
        priority: claim.priority,
        instructions: claim.instructions,
        due_at: claim.due_at.map(|timestamp| timestamp.to_rfc3339()),
        lease_expires_at: claim.lease_expires_at.to_rfc3339(),
        location: CycleCountLocation {
            location_id: claim.location.location_id,
            barcode: claim.location.barcode,
            name: claim.location.name,
        },
        item: CycleCountItem {
            item_id: claim.item.item_id,
            description: claim.item.description,
            barcodes: claim.item.barcodes,
        },
        stock: CycleCountStock {
            inventory_balance_id: claim.stock.inventory_balance_id,
            license_plate_barcode: claim.stock.license_plate_barcode,
            uom: claim.stock.uom,
            lot: claim.stock.lot,
            expiration: claim
                .stock
                .expiration
                .map(|timestamp| timestamp.to_rfc3339()),
            serial: claim.stock.serial,
            inventory_status: map_inventory_status(claim.stock.inventory_status),
        },
    }
}

const fn map_release_reason(reason: CycleCountClaimReleaseReason) -> CoreReleaseReason {
    match reason {
        CycleCountClaimReleaseReason::WorkInterrupted => CoreReleaseReason::WorkInterrupted,
        CycleCountClaimReleaseReason::EquipmentUnavailable => {
            CoreReleaseReason::EquipmentUnavailable
        }
        CycleCountClaimReleaseReason::SafetyIssue => CoreReleaseReason::SafetyIssue,
        CycleCountClaimReleaseReason::Other => CoreReleaseReason::Other,
    }
}

const fn map_inventory_status(status: InventoryStatus) -> InventoryBalanceStatus {
    match status {
        InventoryStatus::Available => InventoryBalanceStatus::Available,
        InventoryStatus::Hold => InventoryBalanceStatus::Hold,
        InventoryStatus::Quarantine => InventoryBalanceStatus::Quarantine,
        InventoryStatus::Damaged => InventoryBalanceStatus::Damaged,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CandidateCursorFilters {
    facility_id: Option<wareboxes_domain::FacilityId>,
    inventory_owner_id: Option<wareboxes_domain::InventoryOwnerId>,
    inventory_status: Option<ApplicationInventoryStatus>,
    sort: CycleCountCandidateSort,
    direction: CycleCountSortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CandidateCursor {
    filters: CandidateCursorFilters,
    cursor: CycleCountCursor,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct WorkCursorFilters {
    facility_id: Option<wareboxes_domain::FacilityId>,
    inventory_owner_id: Option<wareboxes_domain::InventoryOwnerId>,
    status: Option<CycleCountWorkStatus>,
    sort: CycleCountWorkSort,
    direction: CycleCountSortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkCursor {
    filters: WorkCursorFilters,
    cursor: CycleCountCursor,
}

fn map_candidate_page(
    page: ApplicationCandidatePage,
    filters: CandidateCursorFilters,
) -> AppResult<CycleCountCandidatePage> {
    let items = page.items.into_iter().map(map_candidate).collect();
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_candidate_cursor(CandidateCursor { filters, cursor }))
        .transpose()?;
    Ok(CycleCountCandidatePage::new(items, next_cursor))
}

fn map_work_page(
    page: ApplicationWorkPage,
    filters: WorkCursorFilters,
) -> AppResult<CycleCountWorkPage> {
    let items = page.items.into_iter().map(map_work).collect();
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_work_cursor(WorkCursor { filters, cursor }))
        .transpose()?;
    Ok(CycleCountWorkPage::new(items, next_cursor))
}

fn map_candidate(candidate: CycleCountCandidateReadModel) -> CycleCountCandidateResponse {
    CycleCountCandidateResponse {
        inventory_owner_id: candidate.inventory_owner_id.get(),
        inventory_owner_name: candidate.inventory_owner_name,
        facility_id: candidate.facility_id.get(),
        facility_name: candidate.facility_name,
        location: map_location(candidate.location),
        item: map_item(&candidate.stock),
        stock: map_stock(candidate.stock),
        quantity: CycleCountQuantityResponse {
            on_hand: candidate.quantity_on_hand,
            reserved: candidate.quantity_reserved,
            held: candidate.quantity_held,
        },
        last_counted_at: candidate
            .last_counted_at
            .map(|timestamp| timestamp.to_rfc3339()),
        last_variance_quantity: candidate.last_variance_quantity,
    }
}

fn map_work(work: CycleCountWorkReadModel) -> CycleCountWorkResponse {
    let current_quantity =
        work.current_quantity_on_hand
            .map(|on_hand| CycleCountQuantityResponse {
                on_hand,
                reserved: work.current_quantity_reserved.unwrap_or_default(),
                held: work.current_quantity_held.unwrap_or_default(),
            });
    let system_quantity = work
        .system_quantity_on_hand
        .map(|on_hand| CycleCountQuantityResponse {
            on_hand,
            reserved: work.system_quantity_reserved.unwrap_or_default(),
            held: work.system_quantity_held.unwrap_or_default(),
        });
    CycleCountWorkResponse {
        task_id: work.task_id,
        status: map_work_status(work.status),
        inventory_owner_id: work.inventory_owner_id.get(),
        inventory_owner_name: work.inventory_owner_name,
        facility_id: work.facility_id.get(),
        facility_name: work.facility_name,
        location: map_location(work.location),
        item: map_item(&work.stock),
        stock: map_stock(work.stock),
        current_quantity,
        system_quantity,
        counted_quantity: work.counted_quantity,
        variance_quantity: work.variance_quantity,
        inventory_transaction_id: work.inventory_transaction_id,
        priority: work.priority,
        note: work.note,
        assigned_user_id: work.assigned_user_id,
        lease_expires_at: work
            .lease_expires_at
            .map(|timestamp| timestamp.to_rfc3339()),
        due_at: work.due_at.map(|timestamp| timestamp.to_rfc3339()),
        created_at: work.created_at.to_rfc3339(),
        completed_at: work.completed_at.map(|timestamp| timestamp.to_rfc3339()),
        confirmed_by: work.confirmed_by,
        confirmed_at: work.confirmed_at.map(|timestamp| timestamp.to_rfc3339()),
    }
}

fn map_location(location: CycleCountLocationReadModel) -> CycleCountLocation {
    CycleCountLocation {
        location_id: location.location_id,
        barcode: location.barcode,
        name: location.name,
    }
}

fn map_item(stock: &CycleCountStockReadModel) -> CycleCountItem {
    CycleCountItem {
        item_id: stock.item_id,
        description: stock.item_description.clone(),
        barcodes: stock.primary_sku.iter().cloned().collect(),
    }
}

fn map_stock(stock: CycleCountStockReadModel) -> CycleCountStock {
    CycleCountStock {
        inventory_balance_id: stock.inventory_balance_id,
        license_plate_barcode: stock.license_plate_barcode,
        uom: stock.uom,
        lot: stock.lot,
        expiration: stock.expiration.map(|timestamp| timestamp.to_rfc3339()),
        serial: stock.serial,
        inventory_status: map_application_inventory_status(stock.inventory_status),
    }
}

const fn map_work_status(status: CycleCountWorkStatus) -> ApiWorkStatus {
    match status {
        CycleCountWorkStatus::Pending => ApiWorkStatus::Pending,
        CycleCountWorkStatus::Claimed => ApiWorkStatus::Claimed,
        CycleCountWorkStatus::Completed => ApiWorkStatus::Completed,
        CycleCountWorkStatus::Cancelled => ApiWorkStatus::Cancelled,
    }
}

const fn map_work_status_to_application(status: ApiWorkStatus) -> CycleCountWorkStatus {
    match status {
        ApiWorkStatus::Pending => CycleCountWorkStatus::Pending,
        ApiWorkStatus::Claimed => CycleCountWorkStatus::Claimed,
        ApiWorkStatus::Completed => CycleCountWorkStatus::Completed,
        ApiWorkStatus::Cancelled => CycleCountWorkStatus::Cancelled,
    }
}

const fn map_application_inventory_status(
    status: ApplicationInventoryStatus,
) -> InventoryBalanceStatus {
    match status {
        ApplicationInventoryStatus::Available => InventoryBalanceStatus::Available,
        ApplicationInventoryStatus::Hold => InventoryBalanceStatus::Hold,
        ApplicationInventoryStatus::Damaged => InventoryBalanceStatus::Damaged,
        ApplicationInventoryStatus::Quarantine => InventoryBalanceStatus::Quarantine,
    }
}

const fn map_inventory_status_to_application(
    status: InventoryBalanceStatus,
) -> ApplicationInventoryStatus {
    match status {
        InventoryBalanceStatus::Available => ApplicationInventoryStatus::Available,
        InventoryBalanceStatus::Hold => ApplicationInventoryStatus::Hold,
        InventoryBalanceStatus::Damaged => ApplicationInventoryStatus::Damaged,
        InventoryBalanceStatus::Quarantine => ApplicationInventoryStatus::Quarantine,
    }
}

const fn map_candidate_sort_to_application(sort: ApiCandidateSort) -> CycleCountCandidateSort {
    match sort {
        ApiCandidateSort::LastCounted => CycleCountCandidateSort::LastCounted,
        ApiCandidateSort::Client => CycleCountCandidateSort::Client,
        ApiCandidateSort::Facility => CycleCountCandidateSort::Facility,
        ApiCandidateSort::Location => CycleCountCandidateSort::Location,
        ApiCandidateSort::Item => CycleCountCandidateSort::Item,
        ApiCandidateSort::Quantity => CycleCountCandidateSort::Quantity,
        ApiCandidateSort::InventoryStatus => CycleCountCandidateSort::InventoryStatus,
    }
}

const fn map_work_sort_to_application(sort: ApiWorkSort) -> CycleCountWorkSort {
    match sort {
        ApiWorkSort::Priority => CycleCountWorkSort::Priority,
        ApiWorkSort::CreatedAt => CycleCountWorkSort::CreatedAt,
        ApiWorkSort::Client => CycleCountWorkSort::Client,
        ApiWorkSort::Facility => CycleCountWorkSort::Facility,
        ApiWorkSort::Location => CycleCountWorkSort::Location,
        ApiWorkSort::Item => CycleCountWorkSort::Item,
        ApiWorkSort::Quantity => CycleCountWorkSort::Quantity,
        ApiWorkSort::Variance => CycleCountWorkSort::Variance,
        ApiWorkSort::Status => CycleCountWorkSort::Status,
    }
}

const fn map_direction_to_application(direction: ApiSortDirection) -> CycleCountSortDirection {
    match direction {
        ApiSortDirection::Asc => CycleCountSortDirection::Asc,
        ApiSortDirection::Desc => CycleCountSortDirection::Desc,
    }
}

fn encode_candidate_cursor(cursor: CandidateCursor) -> AppResult<OpaqueCursor> {
    let filters = cursor.filters;
    opaque_cursor(format!(
        "{CANDIDATE_CURSOR_PREFIX}{}.{}.{}.{}.{}.{:016x}",
        optional_id(filters.facility_id.map(wareboxes_domain::FacilityId::get)),
        optional_id(
            filters
                .inventory_owner_id
                .map(wareboxes_domain::InventoryOwnerId::get)
        ),
        inventory_status_code(filters.inventory_status),
        filters.sort.as_str(),
        filters.direction.as_str(),
        cursor.cursor.offset,
    ))
}

fn decode_candidate_cursor(cursor: &OpaqueCursor) -> V1Result<CandidateCursor> {
    let parts = cursor_parts(cursor, CANDIDATE_CURSOR_PREFIX, 6, "cycle-count candidates")?;
    Ok(CandidateCursor {
        filters: CandidateCursorFilters {
            facility_id: parse_optional_facility(parts[0], "cycle-count candidates")?,
            inventory_owner_id: parse_optional_owner(parts[1], "cycle-count candidates")?,
            inventory_status: parse_inventory_status_filter(parts[2])?,
            sort: parse_candidate_sort(parts[3])?,
            direction: parse_direction(parts[4], "cycle-count candidates")?,
        },
        cursor: CycleCountCursor {
            offset: parse_hex_u64(parts[5], "cycle-count candidates")?,
        },
    })
}

fn encode_work_cursor(cursor: WorkCursor) -> AppResult<OpaqueCursor> {
    let filters = cursor.filters;
    opaque_cursor(format!(
        "{WORK_CURSOR_PREFIX}{}.{}.{}.{}.{}.{:016x}",
        optional_id(filters.facility_id.map(wareboxes_domain::FacilityId::get)),
        optional_id(
            filters
                .inventory_owner_id
                .map(wareboxes_domain::InventoryOwnerId::get)
        ),
        work_status_code(filters.status),
        filters.sort.as_str(),
        filters.direction.as_str(),
        cursor.cursor.offset,
    ))
}

fn decode_work_cursor(cursor: &OpaqueCursor) -> V1Result<WorkCursor> {
    let parts = cursor_parts(cursor, WORK_CURSOR_PREFIX, 6, "cycle-count work")?;
    Ok(WorkCursor {
        filters: WorkCursorFilters {
            facility_id: parse_optional_facility(parts[0], "cycle-count work")?,
            inventory_owner_id: parse_optional_owner(parts[1], "cycle-count work")?,
            status: parse_work_status_filter(parts[2])?,
            sort: parse_work_sort(parts[3])?,
            direction: parse_direction(parts[4], "cycle-count work")?,
        },
        cursor: CycleCountCursor {
            offset: parse_hex_u64(parts[5], "cycle-count work")?,
        },
    })
}

pub(super) fn cursor_parts<'a>(
    cursor: &'a OpaqueCursor,
    prefix: &str,
    expected: usize,
    label: &str,
) -> V1Result<Vec<&'a str>> {
    cursor
        .as_str()
        .strip_prefix(prefix)
        .map(|value| value.split('.').collect::<Vec<_>>())
        .filter(|parts| parts.len() == expected)
        .ok_or_else(|| V1Error::invalid_cursor_for(label))
}

pub(super) fn optional_id(value: Option<i64>) -> String {
    value.map_or_else(|| "a".into(), |value| format!("{value:016x}"))
}

pub(super) fn parse_optional_facility(
    value: &str,
    label: &str,
) -> V1Result<Option<wareboxes_domain::FacilityId>> {
    parse_optional_id(value, label)?
        .map(wareboxes_domain::FacilityId::new)
        .transpose()
        .map_err(|_| V1Error::invalid_cursor_for(label))
}

pub(super) fn parse_optional_owner(
    value: &str,
    label: &str,
) -> V1Result<Option<wareboxes_domain::InventoryOwnerId>> {
    parse_optional_id(value, label)?
        .map(wareboxes_domain::InventoryOwnerId::new)
        .transpose()
        .map_err(|_| V1Error::invalid_cursor_for(label))
}

fn parse_optional_id(value: &str, label: &str) -> V1Result<Option<i64>> {
    if value == "a" {
        Ok(None)
    } else if value.len() == 16 {
        i64::from_str_radix(value, 16)
            .ok()
            .filter(|value| *value > 0)
            .map(Some)
            .ok_or_else(|| V1Error::invalid_cursor_for(label))
    } else {
        Err(V1Error::invalid_cursor_for(label))
    }
}

fn parse_hex_u64(value: &str, label: &str) -> V1Result<u64> {
    if value.len() != 16 {
        return Err(V1Error::invalid_cursor_for(label));
    }
    u64::from_str_radix(value, 16).map_err(|_| V1Error::invalid_cursor_for(label))
}

pub(super) fn parse_hex_i64(value: &str, label: &str) -> V1Result<i64> {
    let value = parse_hex_u64(value, label)?;
    i64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| V1Error::invalid_cursor_for(label))
}

const fn inventory_status_code(value: Option<ApplicationInventoryStatus>) -> &'static str {
    match value {
        None => "a",
        Some(ApplicationInventoryStatus::Available) => "v",
        Some(ApplicationInventoryStatus::Hold) => "h",
        Some(ApplicationInventoryStatus::Damaged) => "d",
        Some(ApplicationInventoryStatus::Quarantine) => "q",
    }
}

fn parse_inventory_status_filter(value: &str) -> V1Result<Option<ApplicationInventoryStatus>> {
    match value {
        "a" => Ok(None),
        "v" => Ok(Some(ApplicationInventoryStatus::Available)),
        "h" => Ok(Some(ApplicationInventoryStatus::Hold)),
        "d" => Ok(Some(ApplicationInventoryStatus::Damaged)),
        "q" => Ok(Some(ApplicationInventoryStatus::Quarantine)),
        _ => Err(V1Error::invalid_cursor_for("cycle-count candidates")),
    }
}

const fn work_status_code(value: Option<CycleCountWorkStatus>) -> &'static str {
    match value {
        None => "a",
        Some(CycleCountWorkStatus::Pending) => "p",
        Some(CycleCountWorkStatus::Claimed) => "c",
        Some(CycleCountWorkStatus::Completed) => "d",
        Some(CycleCountWorkStatus::Cancelled) => "x",
    }
}

fn parse_work_status_filter(value: &str) -> V1Result<Option<CycleCountWorkStatus>> {
    match value {
        "a" => Ok(None),
        "p" => Ok(Some(CycleCountWorkStatus::Pending)),
        "c" => Ok(Some(CycleCountWorkStatus::Claimed)),
        "d" => Ok(Some(CycleCountWorkStatus::Completed)),
        "x" => Ok(Some(CycleCountWorkStatus::Cancelled)),
        _ => Err(V1Error::invalid_cursor_for("cycle-count work")),
    }
}

fn parse_candidate_sort(value: &str) -> V1Result<CycleCountCandidateSort> {
    match value {
        "last_counted" => Ok(CycleCountCandidateSort::LastCounted),
        "client" => Ok(CycleCountCandidateSort::Client),
        "facility" => Ok(CycleCountCandidateSort::Facility),
        "location" => Ok(CycleCountCandidateSort::Location),
        "item" => Ok(CycleCountCandidateSort::Item),
        "quantity" => Ok(CycleCountCandidateSort::Quantity),
        "inventory_status" => Ok(CycleCountCandidateSort::InventoryStatus),
        _ => Err(V1Error::invalid_cursor_for("cycle-count candidates")),
    }
}

fn parse_work_sort(value: &str) -> V1Result<CycleCountWorkSort> {
    match value {
        "priority" => Ok(CycleCountWorkSort::Priority),
        "created_at" => Ok(CycleCountWorkSort::CreatedAt),
        "client" => Ok(CycleCountWorkSort::Client),
        "facility" => Ok(CycleCountWorkSort::Facility),
        "location" => Ok(CycleCountWorkSort::Location),
        "item" => Ok(CycleCountWorkSort::Item),
        "quantity" => Ok(CycleCountWorkSort::Quantity),
        "variance" => Ok(CycleCountWorkSort::Variance),
        "status" => Ok(CycleCountWorkSort::Status),
        _ => Err(V1Error::invalid_cursor_for("cycle-count work")),
    }
}

fn parse_direction(value: &str, label: &str) -> V1Result<CycleCountSortDirection> {
    match value {
        "asc" => Ok(CycleCountSortDirection::Asc),
        "desc" => Ok(CycleCountSortDirection::Desc),
        _ => Err(V1Error::invalid_cursor_for(label)),
    }
}

pub(super) fn opaque_cursor(value: String) -> AppResult<OpaqueCursor> {
    OpaqueCursor::new(value)
        .map_err(|_| AppError::internal("generated an invalid cycle-count cursor"))
}

pub(super) fn require_page_limit(limit: u16) -> V1Result<()> {
    if limit <= MAX_PAGE_LIMIT {
        Ok(())
    } else {
        Err(invalid("cycle-count page limit must be 1..=100"))
    }
}

fn require_positive(value: i64, label: &str) -> V1Result<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(invalid(format!("{label} must be positive")))
    }
}

fn validate_barcode(value: &str, field: &str) -> V1Result<()> {
    if value.trim() != value || value.is_empty() {
        return Err(invalid(format!("{field} must be trimmed and nonempty")));
    }
    if value.chars().count() > MAX_BARCODE_LENGTH {
        return Err(invalid(format!(
            "{field} cannot exceed {MAX_BARCODE_LENGTH} characters"
        )));
    }
    Ok(())
}

fn validate_note(note: Option<&str>) -> V1Result<()> {
    let Some(note) = note else {
        return Ok(());
    };
    if note.trim() != note || note.is_empty() {
        return Err(invalid("note must be trimmed and nonempty"));
    }
    if note.chars().count() > MAX_NOTE_LENGTH {
        return Err(invalid(format!(
            "note cannot exceed {MAX_NOTE_LENGTH} characters"
        )));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}

pub(super) fn domain_validation(error: impl std::fmt::Display) -> V1Error {
    invalid(error.to_string())
}

pub(super) fn revision_to_api(value: i64) -> AppResult<Revision> {
    Revision::new(value).map_err(|_| AppError::internal("cycle count produced an invalid revision"))
}

#[cfg(test)]
mod manager_tests {
    use super::*;

    #[test]
    fn candidate_cursor_binds_scope_sort_and_status() {
        let filters = CandidateCursorFilters {
            facility_id: wareboxes_domain::FacilityId::new(4).ok(),
            inventory_owner_id: wareboxes_domain::InventoryOwnerId::new(7).ok(),
            inventory_status: Some(ApplicationInventoryStatus::Quarantine),
            sort: CycleCountCandidateSort::Quantity,
            direction: CycleCountSortDirection::Desc,
        };
        let cursor = encode_candidate_cursor(CandidateCursor {
            filters,
            cursor: CycleCountCursor { offset: 100 },
        })
        .unwrap();
        let decoded = decode_candidate_cursor(&cursor).unwrap();
        assert_eq!(decoded.filters, filters);
        assert_eq!(decoded.cursor.offset, 100);
    }

    #[test]
    fn malformed_work_cursor_is_rejected() {
        let cursor = OpaqueCursor::new("cw1.bad").unwrap();
        assert!(decode_work_cursor(&cursor).is_err());
    }
}
