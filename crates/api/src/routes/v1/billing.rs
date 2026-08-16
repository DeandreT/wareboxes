use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    BillableEventResponse, BillableEventType as ApiEventType, BillingChargeResponse,
    BillingContractResponse, BillingContractStatus as ApiContractStatus,
    BillingDecisionPolicyResponse, BillingDecisionPolicySource as ApiBillingPolicySource,
    BillingFinancialExportResponse, BillingLifecycleRequest, BillingPageRequest,
    BillingRateResponse, BillingReviewDecision as ApiReviewDecision, BillingRunResponse,
    BillingRunStatus as ApiRunStatus, BillingStorageSnapshotResponse,
    BillingUnit as ApiBillingUnit, BillingWorkspaceResponse, CaptureBillableEventRequest,
    CaptureBillingStorageSnapshotRequest, ConfigurationScope as ApiConfigurationScope,
    ConfigureBillingRateRequest, CreateBillingContractRequest, ExportBillingRunRequest,
    GenerateBillingRunRequest, OpaqueCursor, ReviewBillingRunRequest, Revision,
};
use wareboxes_application::billing::{
    BillableEventReadModel, BillingChargeReadModel, BillingContractLifecycleCommand,
    BillingContractReadModel, BillingFinancialExportReadModel, BillingRateReadModel,
    BillingReviewDecision, BillingRunReadModel, BillingStorageSnapshotReadModel,
    CaptureBillableEventCommand, CaptureStorageSnapshotCommand, ConfigureBillingRateCommand,
    CreateBillingContractCommand, ExportBillingRunCommand, GenerateBillingRunCommand,
    ReviewBillingRunCommand,
};
use wareboxes_application::billing_decision_policy::{
    BillingDecisionPolicyReadModel, BillingDecisionPolicySource,
};
use wareboxes_domain::{
    BillableEventType, BillingContractId, BillingContractNumber, BillingContractStatus,
    BillingEffectiveWindow, BillingQuantity, BillingRateDefinition, BillingReconciliationRunId,
    BillingRunStatus, BillingUnit, CurrencyCode, FacilityId, InventoryOwnerId, Timestamp,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "admin";
const CURSOR_PREFIX: &str = "bill1.";

pub async fn workspace(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<BillingPageRequest>,
) -> V1Result<Json<BillingWorkspaceResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let owner_id = request
        .inventory_owner_id
        .map(InventoryOwnerId::new)
        .transpose()
        .map_err(validation)?;
    let contract_id = request
        .contract_id
        .map(BillingContractId::new)
        .transpose()
        .map_err(validation)?;
    let after_run_id = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, &request))
        .transpose()?;
    let result = repo::billing::workspace(
        &state.db,
        &user.tenant,
        owner_id,
        contract_id,
        after_run_id,
        request.limit.get(),
    )
    .await?;
    let next_cursor = result
        .next_run_id
        .map(|run_id| encode_cursor(run_id, &request))
        .transpose()?;
    Ok(Json(BillingWorkspaceResponse {
        contracts: result
            .contracts
            .into_iter()
            .map(map_contract)
            .collect::<V1Result<Vec<_>>>()?,
        rates: result
            .rates
            .into_iter()
            .map(map_rate)
            .collect::<V1Result<Vec<_>>>()?,
        events: result
            .events
            .into_iter()
            .map(map_event_response)
            .collect::<V1Result<Vec<_>>>()?,
        runs: result
            .runs
            .into_iter()
            .map(map_run)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    }))
}

pub async fn create_contract(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateBillingContractRequest>,
) -> V1Result<Json<BillingContractResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = CreateBillingContractCommand {
        inventory_owner_id: InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?,
        contract_number: BillingContractNumber::new(body.contract_number).map_err(validation)?,
        currency: CurrencyCode::new(body.currency).map_err(validation)?,
        effective_window: parse_window(&body.effective_from, body.effective_until.as_deref())?,
    };
    let result = repo::billing::create_contract(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_contract(result)?))
}

pub async fn activate_contract(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(contract_id): Path<i64>,
    Json(body): Json<BillingLifecycleRequest>,
) -> V1Result<Json<BillingContractResponse>> {
    lifecycle_contract(state, user, idempotency_key, contract_id, body, true).await
}

pub async fn close_contract(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(contract_id): Path<i64>,
    Json(body): Json<BillingLifecycleRequest>,
) -> V1Result<Json<BillingContractResponse>> {
    lifecycle_contract(state, user, idempotency_key, contract_id, body, false).await
}

async fn lifecycle_contract(
    state: AppState,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    contract_id: i64,
    body: BillingLifecycleRequest,
    activate: bool,
) -> V1Result<Json<BillingContractResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = BillingContractLifecycleCommand {
        contract_id: BillingContractId::new(contract_id).map_err(validation)?,
        expected_revision: body.expected_revision.get(),
    };
    let context = user.command_context(&idempotency_key);
    let result = if activate {
        repo::billing::activate_contract(&state.db, &user.tenant, &context, &command).await?
    } else {
        repo::billing::close_contract(&state.db, &user.tenant, &context, &command).await?
    };
    Ok(Json(map_contract(result)?))
}

pub async fn configure_rate(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(contract_id): Path<i64>,
    Json(body): Json<ConfigureBillingRateRequest>,
) -> V1Result<Json<BillingRateResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ConfigureBillingRateCommand {
        contract_id: BillingContractId::new(contract_id).map_err(validation)?,
        definition: BillingRateDefinition::new(
            event_from_api(body.event_type),
            map_unit(body.unit),
            CurrencyCode::new(body.currency).map_err(validation)?,
            body.rate_minor,
            body.minimum_charge_minor,
        )
        .map_err(validation)?,
        effective_window: parse_window(&body.effective_from, body.effective_until.as_deref())?,
        expected_revision: body.expected_revision.map(Revision::get),
    };
    let result = repo::billing::configure_rate(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_rate(result)?))
}

pub async fn capture_event(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(contract_id): Path<i64>,
    Json(body): Json<CaptureBillableEventRequest>,
) -> V1Result<Json<BillableEventResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = CaptureBillableEventCommand {
        contract_id: BillingContractId::new(contract_id).map_err(validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        event_type: event_from_api(body.event_type),
        unit: map_unit(body.unit),
        quantity: BillingQuantity::new(body.quantity).map_err(validation)?,
        source_reference: body.source_reference,
        description: body.description,
        occurred_at: parse_timestamp(&body.occurred_at, "occurred_at")?,
    };
    let result = repo::billing::capture_billable_event(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_event_response(result)?))
}

pub async fn capture_snapshot(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(contract_id): Path<i64>,
    Json(body): Json<CaptureBillingStorageSnapshotRequest>,
) -> V1Result<Json<BillingStorageSnapshotResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = CaptureStorageSnapshotCommand {
        contract_id: BillingContractId::new(contract_id).map_err(validation)?,
        facility_id: FacilityId::new(body.facility_id).map_err(validation)?,
        snapshot_date: chrono::NaiveDate::parse_from_str(&body.snapshot_date, "%Y-%m-%d")
            .map_err(|_| validation("snapshot_date must use YYYY-MM-DD"))?,
    };
    let result = repo::billing::capture_storage_snapshot(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_snapshot(result)))
}

pub async fn generate_run(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(contract_id): Path<i64>,
    Json(body): Json<GenerateBillingRunRequest>,
) -> V1Result<Json<BillingRunResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = GenerateBillingRunCommand {
        contract_id: BillingContractId::new(contract_id).map_err(validation)?,
        facility_id: body
            .facility_id
            .map(FacilityId::new)
            .transpose()
            .map_err(validation)?,
        period_from: parse_timestamp(&body.period_from, "period_from")?,
        period_until: parse_timestamp(&body.period_until, "period_until")?,
    };
    let result = repo::billing::generate_run(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_run(result)?))
}

pub async fn review_run(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(run_id): Path<i64>,
    Json(body): Json<ReviewBillingRunRequest>,
) -> V1Result<Json<BillingRunResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ReviewBillingRunCommand {
        run_id: BillingReconciliationRunId::new(run_id).map_err(validation)?,
        expected_revision: body.expected_revision.get(),
        decision: match body.decision {
            ApiReviewDecision::Approve => BillingReviewDecision::Approve,
            ApiReviewDecision::Reject => BillingReviewDecision::Reject,
        },
        note: body.note,
    };
    let result = repo::billing::review_run(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_run(result)?))
}

pub async fn export_run(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(run_id): Path<i64>,
    Json(body): Json<ExportBillingRunRequest>,
) -> V1Result<Json<BillingFinancialExportResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ExportBillingRunCommand {
        run_id: BillingReconciliationRunId::new(run_id).map_err(validation)?,
        expected_revision: body.expected_revision.get(),
        external_batch_key: body.external_batch_key,
    };
    let result = repo::billing::export_run(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_export(result)?))
}

fn map_contract(value: BillingContractReadModel) -> V1Result<BillingContractResponse> {
    Ok(BillingContractResponse {
        contract_id: value.contract_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        contract_number: value.contract_number,
        currency: value.currency,
        effective_from: value.effective_window.effective_from.to_rfc3339(),
        effective_until: value
            .effective_window
            .effective_until
            .map(|timestamp| timestamp.to_rfc3339()),
        status: match value.status {
            BillingContractStatus::Draft => ApiContractStatus::Draft,
            BillingContractStatus::Active => ApiContractStatus::Active,
            BillingContractStatus::Closed => ApiContractStatus::Closed,
        },
        revision: Revision::new(value.revision).map_err(invalid_result)?,
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
        activated_by: value.activated_by.map(|user| user.get()),
        activated_at: value.activated_at.map(|timestamp| timestamp.to_rfc3339()),
        closed_by: value.closed_by.map(|user| user.get()),
        closed_at: value.closed_at.map(|timestamp| timestamp.to_rfc3339()),
    })
}

fn map_rate(value: BillingRateReadModel) -> V1Result<BillingRateResponse> {
    Ok(BillingRateResponse {
        rate_id: value.rate_id.get(),
        contract_id: value.contract_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        event_type: map_event_to_api(value.definition.event_type),
        unit: map_unit_to_api(value.definition.unit),
        currency: value.definition.currency.as_str().to_owned(),
        rate_minor: value.definition.rate_minor,
        minimum_charge_minor: value.definition.minimum_charge_minor,
        effective_from: value.effective_window.effective_from.to_rfc3339(),
        effective_until: value
            .effective_window
            .effective_until
            .map(|timestamp| timestamp.to_rfc3339()),
        revision: Revision::new(value.revision).map_err(invalid_result)?,
        active: value.active,
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
    })
}

fn map_event_response(value: BillableEventReadModel) -> V1Result<BillableEventResponse> {
    Ok(BillableEventResponse {
        event_id: value.event_id.get(),
        contract_id: value.contract_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        facility_id: value.facility_id.get(),
        event_type: map_event_to_api(value.event_type),
        unit: map_unit_to_api(value.unit),
        quantity: value.quantity,
        source_type: value.source_type,
        source_reference: value.source_reference,
        description: value.description,
        occurred_at: value.occurred_at.to_rfc3339(),
        captured_at: value.captured_at.to_rfc3339(),
    })
}

fn map_snapshot(value: BillingStorageSnapshotReadModel) -> BillingStorageSnapshotResponse {
    BillingStorageSnapshotResponse {
        snapshot_id: value.snapshot_id.get(),
        contract_id: value.contract_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        facility_id: value.facility_id.get(),
        snapshot_date: value.snapshot_date.format("%Y-%m-%d").to_string(),
        pallet_count: value.pallet_count,
        unit_count: value.unit_count,
        captured_at: value.captured_at.to_rfc3339(),
    }
}

fn map_charge(value: BillingChargeReadModel) -> V1Result<BillingChargeResponse> {
    Ok(BillingChargeResponse {
        charge_id: value.charge_id.get(),
        event_id: value.event_id.get(),
        rate_id: value.rate_id.map(|rate| rate.get()),
        decision_policy: map_billing_decision_policy(value.decision_policy)?,
        event_type: map_event_to_api(value.event_type),
        unit: map_unit_to_api(value.unit),
        quantity: value.quantity,
        rate_minor: value.rate_minor,
        minimum_charge_minor: value.minimum_charge_minor,
        gross_minor: value.gross_minor,
        amount_minor: value.amount_minor,
        currency: value.currency,
        source_type: value.source_type,
        source_reference: value.source_reference,
        occurred_at: value.occurred_at.to_rfc3339(),
    })
}

fn map_billing_decision_policy(
    policy: BillingDecisionPolicyReadModel,
) -> V1Result<BillingDecisionPolicyResponse> {
    Ok(BillingDecisionPolicyResponse {
        source: match policy.source {
            BillingDecisionPolicySource::ContractRate => ApiBillingPolicySource::ContractRate,
            BillingDecisionPolicySource::Configuration => ApiBillingPolicySource::Configuration,
        },
        contract_rate_id: policy.contract_rate_id.map(|rate| rate.get()),
        contract_rate_revision: policy
            .contract_rate_revision
            .map(Revision::new)
            .transpose()
            .map_err(invalid_result)?,
        configuration_id: policy
            .configuration_id
            .map(|configuration| configuration.get()),
        configuration_revision: policy
            .configuration_revision
            .map(Revision::new)
            .transpose()
            .map_err(invalid_result)?,
        configuration_scope: policy.configuration_scope.map(map_configuration_scope),
        event_type: map_event_to_api(policy.event_type),
        unit: map_unit_to_api(policy.unit),
        currency: policy.currency,
        rate_minor: policy.rate_minor,
        minimum_charge_minor: policy.minimum_charge_minor,
        policy_hash: policy.policy_hash,
    })
}

const fn map_configuration_scope(
    scope: wareboxes_domain::ConfigurationScope,
) -> ApiConfigurationScope {
    match scope {
        wareboxes_domain::ConfigurationScope::Tenant => ApiConfigurationScope::Tenant,
        wareboxes_domain::ConfigurationScope::InventoryOwner { inventory_owner_id } => {
            ApiConfigurationScope::InventoryOwner {
                inventory_owner_id: inventory_owner_id.get(),
            }
        }
        wareboxes_domain::ConfigurationScope::Facility { facility_id } => {
            ApiConfigurationScope::Facility {
                facility_id: facility_id.get(),
            }
        }
        wareboxes_domain::ConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        } => ApiConfigurationScope::OwnerFacility {
            inventory_owner_id: inventory_owner_id.get(),
            facility_id: facility_id.get(),
        },
    }
}

fn map_run(value: BillingRunReadModel) -> V1Result<BillingRunResponse> {
    Ok(BillingRunResponse {
        run_id: value.run_id.get(),
        contract_id: value.contract_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        inventory_owner_name: value.inventory_owner_name,
        contract_number: value.contract_number,
        facility_id: value.facility_id.map(|facility| facility.get()),
        attempt: value.attempt,
        supersedes_run_id: value.supersedes_run_id.map(|run| run.get()),
        period_from: value.period_from.to_rfc3339(),
        period_until: value.period_until.to_rfc3339(),
        status: match value.status {
            BillingRunStatus::PendingReview => ApiRunStatus::PendingReview,
            BillingRunStatus::Approved => ApiRunStatus::Approved,
            BillingRunStatus::Rejected => ApiRunStatus::Rejected,
            BillingRunStatus::Exported => ApiRunStatus::Exported,
        },
        revision: Revision::new(value.revision).map_err(invalid_result)?,
        event_count: value.event_count,
        charge_count: value.charge_count,
        unmatched_event_count: value.unmatched_event_count,
        total_minor: value.total_minor,
        currency: value.currency,
        generated_by: value.generated_by.get(),
        generated_at: value.generated_at.to_rfc3339(),
        reviewed_by: value.reviewed_by.map(|user| user.get()),
        reviewed_at: value.reviewed_at.map(|timestamp| timestamp.to_rfc3339()),
        review_note: value.review_note,
        exported_at: value.exported_at.map(|timestamp| timestamp.to_rfc3339()),
        charges: value
            .charges
            .into_iter()
            .map(map_charge)
            .collect::<V1Result<Vec<_>>>()?,
    })
}

fn map_export(value: BillingFinancialExportReadModel) -> V1Result<BillingFinancialExportResponse> {
    Ok(BillingFinancialExportResponse {
        export_id: value.export_id.get(),
        run_id: value.run_id.get(),
        inventory_owner_id: value.inventory_owner_id.get(),
        external_batch_key: value.external_batch_key,
        content_sha256: value.content_sha256,
        line_count: value.line_count,
        total_minor: value.total_minor,
        currency: value.currency,
        csv_content: value.csv_content,
        exported_by: value.exported_by.get(),
        exported_at: value.exported_at.to_rfc3339(),
        resulting_revision: Revision::new(value.resulting_revision).map_err(invalid_result)?,
    })
}

const fn event_from_api(value: ApiEventType) -> BillableEventType {
    match value {
        ApiEventType::ReceiptLine => BillableEventType::ReceiptLine,
        ApiEventType::ReceivedUnit => BillableEventType::ReceivedUnit,
        ApiEventType::PalletDay => BillableEventType::PalletDay,
        ApiEventType::PickLine => BillableEventType::PickLine,
        ApiEventType::PickedUnit => BillableEventType::PickedUnit,
        ApiEventType::PackedCarton => BillableEventType::PackedCarton,
        ApiEventType::ShippedUnit => BillableEventType::ShippedUnit,
        ApiEventType::ReturnUnit => BillableEventType::ReturnUnit,
        ApiEventType::RelabelUnit => BillableEventType::RelabelUnit,
        ApiEventType::RefurbishmentUnit => BillableEventType::RefurbishmentUnit,
        ApiEventType::KitUnit => BillableEventType::KitUnit,
        ApiEventType::AssemblyUnit => BillableEventType::AssemblyUnit,
        ApiEventType::Accessorial => BillableEventType::Accessorial,
        ApiEventType::DetentionHour => BillableEventType::DetentionHour,
        ApiEventType::ValueAddedServiceUnit => BillableEventType::ValueAddedServiceUnit,
    }
}

const fn map_event_to_api(value: BillableEventType) -> ApiEventType {
    match value {
        BillableEventType::ReceiptLine => ApiEventType::ReceiptLine,
        BillableEventType::ReceivedUnit => ApiEventType::ReceivedUnit,
        BillableEventType::PalletDay => ApiEventType::PalletDay,
        BillableEventType::PickLine => ApiEventType::PickLine,
        BillableEventType::PickedUnit => ApiEventType::PickedUnit,
        BillableEventType::PackedCarton => ApiEventType::PackedCarton,
        BillableEventType::ShippedUnit => ApiEventType::ShippedUnit,
        BillableEventType::ReturnUnit => ApiEventType::ReturnUnit,
        BillableEventType::RelabelUnit => ApiEventType::RelabelUnit,
        BillableEventType::RefurbishmentUnit => ApiEventType::RefurbishmentUnit,
        BillableEventType::KitUnit => ApiEventType::KitUnit,
        BillableEventType::AssemblyUnit => ApiEventType::AssemblyUnit,
        BillableEventType::Accessorial => ApiEventType::Accessorial,
        BillableEventType::DetentionHour => ApiEventType::DetentionHour,
        BillableEventType::ValueAddedServiceUnit => ApiEventType::ValueAddedServiceUnit,
    }
}

const fn map_unit(value: ApiBillingUnit) -> BillingUnit {
    match value {
        ApiBillingUnit::Event => BillingUnit::Event,
        ApiBillingUnit::Each => BillingUnit::Each,
        ApiBillingUnit::Case => BillingUnit::Case,
        ApiBillingUnit::Pallet => BillingUnit::Pallet,
        ApiBillingUnit::Carton => BillingUnit::Carton,
        ApiBillingUnit::Hour => BillingUnit::Hour,
        ApiBillingUnit::Day => BillingUnit::Day,
    }
}

const fn map_unit_to_api(value: BillingUnit) -> ApiBillingUnit {
    match value {
        BillingUnit::Event => ApiBillingUnit::Event,
        BillingUnit::Each => ApiBillingUnit::Each,
        BillingUnit::Case => ApiBillingUnit::Case,
        BillingUnit::Pallet => ApiBillingUnit::Pallet,
        BillingUnit::Carton => ApiBillingUnit::Carton,
        BillingUnit::Hour => ApiBillingUnit::Hour,
        BillingUnit::Day => ApiBillingUnit::Day,
    }
}

fn parse_window(from: &str, until: Option<&str>) -> V1Result<BillingEffectiveWindow> {
    BillingEffectiveWindow::new(
        parse_timestamp(from, "effective_from")?,
        until
            .map(|value| parse_timestamp(value, "effective_until"))
            .transpose()?,
    )
    .map_err(validation)
}

fn parse_timestamp(value: &str, field: &str) -> V1Result<Timestamp> {
    value
        .parse::<Timestamp>()
        .map_err(|error| AppError::bad_request(format!("{field} is invalid: {error}")).into())
}

fn cursor_filter(request: &BillingPageRequest) -> String {
    format!(
        "{}.{}",
        request
            .inventory_owner_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        request
            .contract_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}"))
    )
}

fn encode_cursor(
    run_id: BillingReconciliationRunId,
    request: &BillingPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{:016x}",
        cursor_filter(request),
        run_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid billing cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    request: &BillingPageRequest,
) -> V1Result<BillingReconciliationRunId> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("billing workspace"))?;
    let (filter, run_id) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("billing workspace"))?;
    if filter != cursor_filter(request) || run_id.len() != 16 {
        return Err(V1Error::invalid_cursor_for("billing workspace"));
    }
    let run_id = i64::from_str_radix(run_id, 16)
        .map_err(|_| V1Error::invalid_cursor_for("billing workspace"))?;
    BillingReconciliationRunId::new(run_id)
        .map_err(|_| V1Error::invalid_cursor_for("billing workspace"))
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    V1Error::internal(error.to_string())
}
