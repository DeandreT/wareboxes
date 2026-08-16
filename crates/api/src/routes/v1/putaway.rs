use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use wareboxes_api_contract::v1::{
    ConfirmPutawayRequest, CreatePutawayTaskRequest, CreatePutawayTaskResponse, OpaqueCursor,
    PutawayCandidatePage, PutawayCandidatePageRequest, PutawayCandidateResponse,
    PutawayCandidateSort as ApiCandidateSort, PutawayConfirmationResponse, PutawayLocationResponse,
    PutawayPolicyExpectation as ApiPolicyExpectation, PutawayPolicyResponse as ApiPolicyResponse,
    PutawayPolicySource as ApiPolicySource, PutawaySortDirection as ApiSortDirection,
    PutawayWorkPage, PutawayWorkPageRequest, PutawayWorkResponse, PutawayWorkSort as ApiWorkSort,
    PutawayWorkStatus as ApiWorkStatus, PutawayWorkflow as ApiWorkflow,
};
use wareboxes_application::putaway::{
    PutawayCandidatePage as ApplicationCandidatePage, PutawayCandidateQuery,
    PutawayCandidateReadModel, PutawayCandidateSort, PutawayCursor, PutawayLocationReadModel,
    PutawaySortDirection, PutawayWorkPage as ApplicationWorkPage, PutawayWorkQuery,
    PutawayWorkReadModel, PutawayWorkSort, PutawayWorkStatus, PutawayWorkflow,
};
use wareboxes_application::putaway_policy::{
    PutawayPolicyExpectation, PutawayPolicyReadModel, PutawayPolicySource,
};
use wareboxes_domain::{ConfigurationScope, ConfigurationVersionId, FacilityId, InventoryOwnerId};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::{AppError, AppResult};
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "wms";
const MAX_BARCODE_LENGTH: usize = 200;
const MAX_INSTRUCTIONS_LENGTH: usize = 1_000;
const CANDIDATE_CURSOR_PREFIX: &str = "pc1.";
const WORK_CURSOR_PREFIX: &str = "pt1.";
const MAX_PAGE_LIMIT: u16 = 100;

pub async fn candidates(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(query): Query<PutawayCandidatePageRequest>,
) -> V1Result<Json<PutawayCandidatePage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_page_limit(query.limit.get())?;
    let facility_id = query
        .facility_id
        .map(|value| user.require_facility(value))
        .transpose()?;
    let inventory_owner_id = query
        .inventory_owner_id
        .map(|value| user.require_inventory_owner(value))
        .transpose()?;
    let workflow = query.workflow.map(map_workflow_to_application);
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
        workflow,
        sort,
        direction,
    };
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.filters != filters)
    {
        return Err(V1Error::invalid_cursor_for("putaway candidates"));
    }
    let page = repo::tasks::putaway_candidate_page(
        &state.db,
        &user.tenant,
        PutawayCandidateQuery {
            facility_id,
            inventory_owner_id,
            workflow,
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
    Query(query): Query<PutawayWorkPageRequest>,
) -> V1Result<Json<PutawayWorkPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_page_limit(query.limit.get())?;
    let facility_id = query
        .facility_id
        .map(|value| user.require_facility(value))
        .transpose()?;
    let inventory_owner_id = query
        .inventory_owner_id
        .map(|value| user.require_inventory_owner(value))
        .transpose()?;
    let workflow = query.workflow.map(map_workflow_to_application);
    let status = query.status.map(map_status_to_application);
    let sort = map_work_sort_to_application(query.sort);
    let direction = map_direction_to_application(query.direction);
    let cursor = query.cursor.as_ref().map(decode_work_cursor).transpose()?;
    let filters = WorkCursorFilters {
        facility_id,
        inventory_owner_id,
        workflow,
        status,
        sort,
        direction,
    };
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.filters != filters)
    {
        return Err(V1Error::invalid_cursor_for("putaway work"));
    }
    let page = repo::tasks::putaway_work_page(
        &state.db,
        &user.tenant,
        PutawayWorkQuery {
            facility_id,
            inventory_owner_id,
            workflow,
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

#[cfg_attr(not(feature = "ssr"), allow(dead_code))]
pub(crate) async fn pages_for_access(
    state: &AppState,
    access: &wareboxes_core::models::TenantAccess,
    limit: u16,
) -> AppResult<(PutawayCandidatePage, PutawayWorkPage)> {
    let candidate_filters = CandidateCursorFilters::default();
    let work_filters = WorkCursorFilters::default();
    let (candidates, work) = tokio::try_join!(
        repo::tasks::putaway_candidate_page(
            &state.db,
            access,
            PutawayCandidateQuery {
                facility_id: None,
                inventory_owner_id: None,
                workflow: None,
                sort: PutawayCandidateSort::default(),
                direction: PutawaySortDirection::default(),
                cursor: None,
                limit,
            },
        ),
        repo::tasks::putaway_work_page(
            &state.db,
            access,
            PutawayWorkQuery {
                facility_id: None,
                inventory_owner_id: None,
                workflow: None,
                status: None,
                sort: PutawayWorkSort::default(),
                direction: PutawaySortDirection::default(),
                cursor: None,
                limit,
            },
        ),
    )?;
    Ok((
        map_candidate_page(candidates, candidate_filters)?,
        map_work_page(work, work_filters)?,
    ))
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreatePutawayTaskRequest>,
) -> V1Result<Json<CreatePutawayTaskResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(
        body.source_inventory_balance_id,
        "source inventory balance ID",
    )?;
    require_positive(body.destination_location_id, "destination location ID")?;
    require_positive(body.quantity, "quantity")?;
    if body.priority.is_some_and(|priority| priority < 0) {
        return Err(invalid("priority cannot be negative"));
    }
    if let Some(assigned_user_id) = body.assigned_user_id {
        require_positive(assigned_user_id, "assigned user ID")?;
    }
    let scheduled_for = parse_timestamp(body.scheduled_for.as_deref(), "scheduled_for")?;
    let due_at = parse_timestamp(body.due_at.as_deref(), "due_at")?;
    if scheduled_for
        .as_ref()
        .zip(due_at.as_ref())
        .is_some_and(|(scheduled_for, due_at)| due_at < scheduled_for)
    {
        return Err(invalid("due_at cannot be earlier than scheduled_for"));
    }
    validate_instructions(body.instructions.as_deref())?;
    let expected_policy = map_policy_expectation(body.expected_policy)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::tasks::create_putaway_task_with_policy_in_scope(
        &state.db,
        &user.tenant,
        &context,
        body.source_inventory_balance_id,
        body.destination_location_id,
        body.quantity,
        body.priority.unwrap_or(50),
        body.assigned_user_id,
        scheduled_for,
        due_at,
        body.instructions.as_deref(),
        &expected_policy,
    )
    .await?;

    Ok(Json(CreatePutawayTaskResponse {
        task_id: result.task_id,
        putaway_policy: map_policy(result.putaway_policy),
    }))
}

pub async fn confirm(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(task_id): Path<i64>,
    Json(body): Json<ConfirmPutawayRequest>,
) -> V1Result<Json<PutawayConfirmationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(task_id, "task ID")?;
    validate_barcode(
        &body.destination_location_barcode,
        "destination_location_barcode",
    )?;
    let expected_policy = map_policy_expectation(body.expected_policy)?;
    let context = user.command_context(&idempotency_key);
    let outcome = repo::tasks::confirm_putaway_with_policy_in_scope(
        &state.db,
        &user.tenant,
        &context,
        task_id,
        &body.destination_location_barcode,
        &expected_policy,
    )
    .await?;
    let confirmation = outcome.confirmation;

    Ok(Json(PutawayConfirmationResponse {
        task_id: confirmation.task_id,
        inventory_owner_id: confirmation.inventory_owner_id.get(),
        facility_id: confirmation.facility_id,
        inventory_transaction_id: confirmation.inventory_transaction_id,
        source_inventory_balance_id: confirmation.source_inventory_balance_id,
        destination_inventory_balance_id: confirmation.destination_inventory_balance_id,
        source_location_id: confirmation.source_location_id,
        destination_location_id: confirmation.destination_location_id,
        destination_location_barcode: confirmation.destination_location_barcode,
        item_batch_id: confirmation.item_batch_id,
        item_id: confirmation.item_id,
        quantity: confirmation.quantity,
        inventory_status: confirmation.inventory_status.as_str().to_owned(),
        confirmed_by: confirmation.confirmed_by,
        confirmed_at: confirmation.confirmed_at.to_rfc3339(),
        putaway_policy: map_policy(outcome.putaway_policy),
    }))
}

fn require_positive(value: i64, label: &str) -> V1Result<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(invalid(format!("{label} must be positive")))
    }
}

fn parse_timestamp(value: Option<&str>, field: &str) -> V1Result<Option<DateTime<Utc>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim() != value || value.is_empty() {
        return Err(invalid(format!(
            "{field} must be a nonempty RFC3339 timestamp"
        )));
    }
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| Some(timestamp.with_timezone(&Utc)))
        .map_err(|_| invalid(format!("{field} must be an RFC3339 timestamp")))
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

fn validate_instructions(instructions: Option<&str>) -> V1Result<()> {
    let Some(instructions) = instructions else {
        return Ok(());
    };
    if instructions.trim() != instructions || instructions.is_empty() {
        return Err(invalid("instructions must be trimmed and nonempty"));
    }
    if instructions.chars().count() > MAX_INSTRUCTIONS_LENGTH {
        return Err(invalid(format!(
            "instructions cannot exceed {MAX_INSTRUCTIONS_LENGTH} characters"
        )));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CandidateCursorFilters {
    facility_id: Option<FacilityId>,
    inventory_owner_id: Option<InventoryOwnerId>,
    workflow: Option<PutawayWorkflow>,
    sort: PutawayCandidateSort,
    direction: PutawaySortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CandidateCursor {
    filters: CandidateCursorFilters,
    cursor: PutawayCursor,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct WorkCursorFilters {
    facility_id: Option<FacilityId>,
    inventory_owner_id: Option<InventoryOwnerId>,
    workflow: Option<PutawayWorkflow>,
    status: Option<PutawayWorkStatus>,
    sort: PutawayWorkSort,
    direction: PutawaySortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkCursor {
    filters: WorkCursorFilters,
    cursor: PutawayCursor,
}

fn map_candidate_page(
    page: ApplicationCandidatePage,
    filters: CandidateCursorFilters,
) -> AppResult<PutawayCandidatePage> {
    let items = page.items.into_iter().map(map_candidate).collect();
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_candidate_cursor(CandidateCursor { filters, cursor }))
        .transpose()?;
    Ok(PutawayCandidatePage::new(items, next_cursor))
}

fn map_work_page(
    page: ApplicationWorkPage,
    filters: WorkCursorFilters,
) -> AppResult<PutawayWorkPage> {
    let items = page.items.into_iter().map(map_work).collect();
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_work_cursor(WorkCursor { filters, cursor }))
        .transpose()?;
    Ok(PutawayWorkPage::new(items, next_cursor))
}

fn map_candidate(candidate: PutawayCandidateReadModel) -> PutawayCandidateResponse {
    PutawayCandidateResponse {
        workflow: map_workflow(candidate.workflow),
        inventory_owner_id: candidate.inventory_owner_id.get(),
        inventory_owner_name: candidate.inventory_owner_name,
        facility_id: candidate.facility_id.get(),
        facility_name: candidate.facility_name,
        source_inventory_balance_id: candidate.source_inventory_balance_id,
        license_plate_id: candidate.license_plate_id,
        license_plate_barcode: candidate.license_plate_barcode,
        source_location: map_location(candidate.source_location),
        item_count: candidate.item_count,
        balance_count: candidate.balance_count,
        item_id: candidate.item_id,
        item_description: candidate.item_description,
        primary_sku: candidate.primary_sku,
        uom: candidate.uom,
        lot: candidate.lot,
        serial: candidate.serial,
        available_quantity: candidate.available_quantity,
        received_at: candidate.received_at.to_rfc3339(),
        putaway_policy: map_policy(candidate.putaway_policy),
    }
}

fn map_work(work: PutawayWorkReadModel) -> PutawayWorkResponse {
    PutawayWorkResponse {
        task_id: work.task_id,
        workflow: map_workflow(work.workflow),
        status: map_status(work.status),
        inventory_owner_id: work.inventory_owner_id.get(),
        inventory_owner_name: work.inventory_owner_name,
        facility_id: work.facility_id.get(),
        facility_name: work.facility_name,
        source_inventory_balance_id: work.source_inventory_balance_id,
        license_plate_id: work.license_plate_id,
        license_plate_barcode: work.license_plate_barcode,
        source_location: map_location(work.source_location),
        destination_location: map_location(work.destination_location),
        item_count: work.item_count,
        balance_count: work.balance_count,
        item_id: work.item_id,
        item_description: work.item_description,
        primary_sku: work.primary_sku,
        uom: work.uom,
        planned_quantity: work.planned_quantity,
        priority: work.priority,
        instructions: work.instructions,
        assigned_user_id: work.assigned_user_id,
        lease_expires_at: work.lease_expires_at.map(|value| value.to_rfc3339()),
        due_at: work.due_at.map(|value| value.to_rfc3339()),
        created_at: work.created_at.to_rfc3339(),
        completed_at: work.completed_at.map(|value| value.to_rfc3339()),
        putaway_policy: map_policy(work.putaway_policy),
    }
}

pub(super) fn map_policy_expectation(
    value: ApiPolicyExpectation,
) -> V1Result<PutawayPolicyExpectation> {
    let expectation = PutawayPolicyExpectation {
        source: match value.source {
            ApiPolicySource::ProductDefault => PutawayPolicySource::ProductDefault,
            ApiPolicySource::Configuration => PutawayPolicySource::Configuration,
        },
        configuration_id: value
            .configuration_id
            .map(ConfigurationVersionId::new)
            .transpose()
            .map_err(|error| AppError::bad_request(error.to_string()))?,
        configuration_revision: value.configuration_revision,
        policy_hash: value.policy_hash,
    };
    if expectation.is_well_formed() {
        Ok(expectation)
    } else {
        Err(invalid("putaway policy expectation is invalid"))
    }
}

pub(super) fn map_policy(value: PutawayPolicyReadModel) -> ApiPolicyResponse {
    ApiPolicyResponse {
        source: match value.source {
            PutawayPolicySource::ProductDefault => ApiPolicySource::ProductDefault,
            PutawayPolicySource::Configuration => ApiPolicySource::Configuration,
        },
        configuration_id: value.configuration_id.map(|id| id.get()),
        configuration_revision: value.configuration_revision,
        configuration_scope: value.configuration_scope.map(|scope| match scope {
            ConfigurationScope::Tenant => wareboxes_api_contract::v1::ConfigurationScope::Tenant,
            ConfigurationScope::InventoryOwner { inventory_owner_id } => {
                wareboxes_api_contract::v1::ConfigurationScope::InventoryOwner {
                    inventory_owner_id: inventory_owner_id.get(),
                }
            }
            ConfigurationScope::Facility { facility_id } => {
                wareboxes_api_contract::v1::ConfigurationScope::Facility {
                    facility_id: facility_id.get(),
                }
            }
            ConfigurationScope::OwnerFacility {
                inventory_owner_id,
                facility_id,
            } => wareboxes_api_contract::v1::ConfigurationScope::OwnerFacility {
                inventory_owner_id: inventory_owner_id.get(),
                facility_id: facility_id.get(),
            },
        }),
        require_zone_compatibility: value.require_zone_compatibility,
        enforce_location_capacity: value.enforce_location_capacity,
        allow_mixed_lots: value.allow_mixed_lots,
        policy_hash: value.policy_hash,
    }
}

fn map_location(location: PutawayLocationReadModel) -> PutawayLocationResponse {
    PutawayLocationResponse {
        location_id: location.location_id,
        barcode: location.barcode,
        name: location.name,
    }
}

fn map_workflow(workflow: PutawayWorkflow) -> ApiWorkflow {
    match workflow {
        PutawayWorkflow::Loose => ApiWorkflow::Loose,
        PutawayWorkflow::LicensePlate => ApiWorkflow::LicensePlate,
    }
}

fn map_workflow_to_application(workflow: ApiWorkflow) -> PutawayWorkflow {
    match workflow {
        ApiWorkflow::Loose => PutawayWorkflow::Loose,
        ApiWorkflow::LicensePlate => PutawayWorkflow::LicensePlate,
    }
}

fn map_status(status: PutawayWorkStatus) -> ApiWorkStatus {
    match status {
        PutawayWorkStatus::Pending => ApiWorkStatus::Pending,
        PutawayWorkStatus::Claimed => ApiWorkStatus::Claimed,
        PutawayWorkStatus::Completed => ApiWorkStatus::Completed,
        PutawayWorkStatus::Cancelled => ApiWorkStatus::Cancelled,
    }
}

fn map_status_to_application(status: ApiWorkStatus) -> PutawayWorkStatus {
    match status {
        ApiWorkStatus::Pending => PutawayWorkStatus::Pending,
        ApiWorkStatus::Claimed => PutawayWorkStatus::Claimed,
        ApiWorkStatus::Completed => PutawayWorkStatus::Completed,
        ApiWorkStatus::Cancelled => PutawayWorkStatus::Cancelled,
    }
}

fn map_candidate_sort_to_application(sort: ApiCandidateSort) -> PutawayCandidateSort {
    match sort {
        ApiCandidateSort::ReceivedAt => PutawayCandidateSort::ReceivedAt,
        ApiCandidateSort::Client => PutawayCandidateSort::Client,
        ApiCandidateSort::Facility => PutawayCandidateSort::Facility,
        ApiCandidateSort::Source => PutawayCandidateSort::Source,
        ApiCandidateSort::Item => PutawayCandidateSort::Item,
        ApiCandidateSort::Quantity => PutawayCandidateSort::Quantity,
        ApiCandidateSort::Workflow => PutawayCandidateSort::Workflow,
    }
}

fn map_work_sort_to_application(sort: ApiWorkSort) -> PutawayWorkSort {
    match sort {
        ApiWorkSort::Priority => PutawayWorkSort::Priority,
        ApiWorkSort::CreatedAt => PutawayWorkSort::CreatedAt,
        ApiWorkSort::Client => PutawayWorkSort::Client,
        ApiWorkSort::Facility => PutawayWorkSort::Facility,
        ApiWorkSort::Source => PutawayWorkSort::Source,
        ApiWorkSort::Destination => PutawayWorkSort::Destination,
        ApiWorkSort::Quantity => PutawayWorkSort::Quantity,
        ApiWorkSort::Status => PutawayWorkSort::Status,
        ApiWorkSort::Workflow => PutawayWorkSort::Workflow,
    }
}

fn map_direction_to_application(direction: ApiSortDirection) -> PutawaySortDirection {
    match direction {
        ApiSortDirection::Asc => PutawaySortDirection::Asc,
        ApiSortDirection::Desc => PutawaySortDirection::Desc,
    }
}

fn decode_candidate_cursor(cursor: &OpaqueCursor) -> V1Result<CandidateCursor> {
    let parts = cursor_parts(cursor, CANDIDATE_CURSOR_PREFIX, 6, "putaway candidates")?;
    Ok(CandidateCursor {
        filters: CandidateCursorFilters {
            facility_id: parse_optional_facility(parts[0], "putaway candidates")?,
            inventory_owner_id: parse_optional_owner(parts[1], "putaway candidates")?,
            workflow: parse_workflow(parts[2], "putaway candidates")?,
            sort: parse_candidate_sort(parts[3])?,
            direction: parse_direction(parts[4], "putaway candidates")?,
        },
        cursor: PutawayCursor {
            offset: parse_hex_u64(parts[5], "putaway candidates")?,
        },
    })
}

fn encode_candidate_cursor(cursor: CandidateCursor) -> AppResult<OpaqueCursor> {
    let filters = cursor.filters;
    opaque_cursor(format!(
        "{CANDIDATE_CURSOR_PREFIX}{}.{}.{}.{}.{}.{:016x}",
        optional_id(filters.facility_id.map(FacilityId::get)),
        optional_id(filters.inventory_owner_id.map(InventoryOwnerId::get)),
        workflow_code(filters.workflow),
        filters.sort.as_str(),
        filters.direction.as_str(),
        cursor.cursor.offset,
    ))
}

fn decode_work_cursor(cursor: &OpaqueCursor) -> V1Result<WorkCursor> {
    let parts = cursor_parts(cursor, WORK_CURSOR_PREFIX, 7, "putaway work")?;
    Ok(WorkCursor {
        filters: WorkCursorFilters {
            facility_id: parse_optional_facility(parts[0], "putaway work")?,
            inventory_owner_id: parse_optional_owner(parts[1], "putaway work")?,
            workflow: parse_workflow(parts[2], "putaway work")?,
            status: parse_work_status(parts[3])?,
            sort: parse_work_sort(parts[4])?,
            direction: parse_direction(parts[5], "putaway work")?,
        },
        cursor: PutawayCursor {
            offset: parse_hex_u64(parts[6], "putaway work")?,
        },
    })
}

fn encode_work_cursor(cursor: WorkCursor) -> AppResult<OpaqueCursor> {
    let filters = cursor.filters;
    opaque_cursor(format!(
        "{WORK_CURSOR_PREFIX}{}.{}.{}.{}.{}.{}.{:016x}",
        optional_id(filters.facility_id.map(FacilityId::get)),
        optional_id(filters.inventory_owner_id.map(InventoryOwnerId::get)),
        workflow_code(filters.workflow),
        status_code(filters.status),
        filters.sort.as_str(),
        filters.direction.as_str(),
        cursor.cursor.offset,
    ))
}

fn cursor_parts<'a>(
    cursor: &'a OpaqueCursor,
    prefix: &str,
    expected: usize,
    label: &str,
) -> V1Result<Vec<&'a str>> {
    let parts = cursor
        .as_str()
        .strip_prefix(prefix)
        .map(|value| value.split('.').collect::<Vec<_>>())
        .filter(|parts| parts.len() == expected)
        .ok_or_else(|| V1Error::invalid_cursor_for(label))?;
    Ok(parts)
}

fn optional_id(value: Option<i64>) -> String {
    value.map_or_else(|| "a".into(), |value| format!("{value:016x}"))
}

fn parse_optional_facility(value: &str, label: &str) -> V1Result<Option<FacilityId>> {
    parse_optional_id(value, label)?
        .map(FacilityId::new)
        .transpose()
        .map_err(|_| V1Error::invalid_cursor_for(label))
}

fn parse_optional_owner(value: &str, label: &str) -> V1Result<Option<InventoryOwnerId>> {
    parse_optional_id(value, label)?
        .map(InventoryOwnerId::new)
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

fn workflow_code(value: Option<PutawayWorkflow>) -> &'static str {
    match value {
        None => "a",
        Some(PutawayWorkflow::Loose) => "l",
        Some(PutawayWorkflow::LicensePlate) => "p",
    }
}

fn status_code(value: Option<PutawayWorkStatus>) -> &'static str {
    match value {
        None => "a",
        Some(PutawayWorkStatus::Pending) => "p",
        Some(PutawayWorkStatus::Claimed) => "c",
        Some(PutawayWorkStatus::Completed) => "d",
        Some(PutawayWorkStatus::Cancelled) => "x",
    }
}

fn parse_workflow(value: &str, label: &str) -> V1Result<Option<PutawayWorkflow>> {
    match value {
        "a" => Ok(None),
        "l" => Ok(Some(PutawayWorkflow::Loose)),
        "p" => Ok(Some(PutawayWorkflow::LicensePlate)),
        _ => Err(V1Error::invalid_cursor_for(label)),
    }
}

fn parse_work_status(value: &str) -> V1Result<Option<PutawayWorkStatus>> {
    match value {
        "a" => Ok(None),
        "p" => Ok(Some(PutawayWorkStatus::Pending)),
        "c" => Ok(Some(PutawayWorkStatus::Claimed)),
        "d" => Ok(Some(PutawayWorkStatus::Completed)),
        "x" => Ok(Some(PutawayWorkStatus::Cancelled)),
        _ => Err(V1Error::invalid_cursor_for("putaway work")),
    }
}

fn parse_candidate_sort(value: &str) -> V1Result<PutawayCandidateSort> {
    match value {
        "received_at" => Ok(PutawayCandidateSort::ReceivedAt),
        "client" => Ok(PutawayCandidateSort::Client),
        "facility" => Ok(PutawayCandidateSort::Facility),
        "source" => Ok(PutawayCandidateSort::Source),
        "item" => Ok(PutawayCandidateSort::Item),
        "quantity" => Ok(PutawayCandidateSort::Quantity),
        "workflow" => Ok(PutawayCandidateSort::Workflow),
        _ => Err(V1Error::invalid_cursor_for("putaway candidates")),
    }
}

fn parse_work_sort(value: &str) -> V1Result<PutawayWorkSort> {
    match value {
        "priority" => Ok(PutawayWorkSort::Priority),
        "created_at" => Ok(PutawayWorkSort::CreatedAt),
        "client" => Ok(PutawayWorkSort::Client),
        "facility" => Ok(PutawayWorkSort::Facility),
        "source" => Ok(PutawayWorkSort::Source),
        "destination" => Ok(PutawayWorkSort::Destination),
        "quantity" => Ok(PutawayWorkSort::Quantity),
        "status" => Ok(PutawayWorkSort::Status),
        "workflow" => Ok(PutawayWorkSort::Workflow),
        _ => Err(V1Error::invalid_cursor_for("putaway work")),
    }
}

fn parse_direction(value: &str, label: &str) -> V1Result<PutawaySortDirection> {
    match value {
        "asc" => Ok(PutawaySortDirection::Asc),
        "desc" => Ok(PutawaySortDirection::Desc),
        _ => Err(V1Error::invalid_cursor_for(label)),
    }
}

fn opaque_cursor(value: String) -> AppResult<OpaqueCursor> {
    OpaqueCursor::new(value).map_err(|_| AppError::internal("generated an invalid putaway cursor"))
}

fn require_page_limit(limit: u16) -> V1Result<()> {
    if limit <= MAX_PAGE_LIMIT {
        Ok(())
    } else {
        Err(invalid("putaway page limit must be 1..=100"))
    }
}

#[cfg(test)]
mod manager_tests {
    use super::*;

    #[test]
    fn cursors_bind_sort_and_filters() {
        let filters = CandidateCursorFilters {
            facility_id: FacilityId::new(4).ok(),
            inventory_owner_id: InventoryOwnerId::new(7).ok(),
            workflow: Some(PutawayWorkflow::LicensePlate),
            sort: PutawayCandidateSort::Quantity,
            direction: PutawaySortDirection::Desc,
        };
        let encoded = encode_candidate_cursor(CandidateCursor {
            filters,
            cursor: PutawayCursor { offset: 100 },
        })
        .unwrap();
        assert_eq!(decode_candidate_cursor(&encoded).unwrap().filters, filters);
        assert_eq!(
            decode_candidate_cursor(&encoded).unwrap().cursor.offset,
            100
        );
    }

    #[test]
    fn malformed_cursor_is_rejected() {
        let cursor = OpaqueCursor::new("pc1.bad").unwrap();
        assert!(decode_candidate_cursor(&cursor).is_err());
    }
}
