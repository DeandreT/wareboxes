use axum::extract::{Path, Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    BillableEventType as ApiBillableEventType, BillingUnit as ApiBillingUnit,
    ConfigurationLifecycleRequest, ConfigurationPage as ApiConfigurationPage,
    ConfigurationPageRequest, ConfigurationResponse, ConfigurationScope as ApiConfigurationScope,
    ConfigurationSimulationResponse, ConfigurationStatus as ApiConfigurationStatus,
    CreateConfigurationRequest, DecisionRule as ApiDecisionRule,
    DecisionRuleKind as ApiDecisionRuleKind, InventoryRotation as ApiInventoryRotation,
    OpaqueCursor, Revision, RollbackConfigurationRequest, SimulateConfigurationRequest,
};
use wareboxes_application::configuration::{
    ActivateConfigurationCommand, ConfigurationCursor, ConfigurationLifecycleCommand,
    ConfigurationPageQuery, ConfigurationReadModel, CreateConfigurationCommand,
    RollbackConfigurationCommand, SimulateConfigurationQuery,
};
use wareboxes_domain::{
    BillableEventType, BillingUnit, ConfigurationEffectiveWindow, ConfigurationScope,
    ConfigurationStatus, ConfigurationVersionId, DecisionRuleDefinition, DecisionRuleKind,
    FacilityId, InventoryOwnerId, InventoryRotation,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;

const PERMISSION: &str = "admin";
const CURSOR_PREFIX: &str = "cfg1.";

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<ConfigurationPageRequest>,
) -> V1Result<Json<ApiConfigurationPage>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let inventory_owner_id = request
        .inventory_owner_id
        .map(InventoryOwnerId::new)
        .transpose()
        .map_err(validation)?;
    let facility_id = request
        .facility_id
        .map(FacilityId::new)
        .transpose()
        .map_err(validation)?;
    let cursor = request
        .cursor
        .as_ref()
        .map(|cursor| decode_cursor(cursor, &request))
        .transpose()?;
    let page = repo::configuration::configuration_page(
        &state.db,
        &user.tenant,
        ConfigurationPageQuery {
            kind: request.kind.map(map_kind),
            status: request.status.map(map_status),
            inventory_owner_id,
            facility_id,
            cursor,
            limit: request.limit.get(),
        },
    )
    .await?;
    let next_cursor = page
        .next_cursor
        .map(|cursor| encode_cursor(cursor, &request))
        .transpose()?;
    Ok(Json(ApiConfigurationPage {
        items: page
            .items
            .into_iter()
            .map(map_response)
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor,
    }))
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Json(body): Json<CreateConfigurationRequest>,
) -> V1Result<Json<ConfigurationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let scope = map_scope(body.scope)?;
    let effective_window = map_window(body.effective_from, body.effective_until)?;
    let definition = map_rule(body.rule);
    definition.validate().map_err(validation)?;
    let command = CreateConfigurationCommand {
        scope,
        effective_window,
        definition,
        expected_revision: body.expected_revision.map(Revision::get),
    };
    let result = repo::configuration::create_configuration(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn submit(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(configuration_id): Path<i64>,
    Json(body): Json<ConfigurationLifecycleRequest>,
) -> V1Result<Json<ConfigurationResponse>> {
    lifecycle(
        state,
        user,
        idempotency_key,
        configuration_id,
        body,
        Lifecycle::Submit,
    )
    .await
}

pub async fn approve(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(configuration_id): Path<i64>,
    Json(body): Json<ConfigurationLifecycleRequest>,
) -> V1Result<Json<ConfigurationResponse>> {
    lifecycle(
        state,
        user,
        idempotency_key,
        configuration_id,
        body,
        Lifecycle::Approve,
    )
    .await
}

pub async fn activate(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(configuration_id): Path<i64>,
    Json(body): Json<ConfigurationLifecycleRequest>,
) -> V1Result<Json<ConfigurationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ActivateConfigurationCommand {
        configuration_id: ConfigurationVersionId::new(configuration_id).map_err(validation)?,
        expected_revision: body.expected_revision.get(),
    };
    let result = repo::configuration::activate_configuration(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn retire(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(configuration_id): Path<i64>,
    Json(body): Json<ConfigurationLifecycleRequest>,
) -> V1Result<Json<ConfigurationResponse>> {
    lifecycle(
        state,
        user,
        idempotency_key,
        configuration_id,
        body,
        Lifecycle::Retire,
    )
    .await
}

pub async fn rollback(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(configuration_id): Path<i64>,
    Json(body): Json<RollbackConfigurationRequest>,
) -> V1Result<Json<ConfigurationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = RollbackConfigurationCommand {
        source_configuration_id: ConfigurationVersionId::new(configuration_id)
            .map_err(validation)?,
        expected_source_revision: body.expected_source_revision.get(),
        effective_window: map_window(body.effective_from, body.effective_until)?,
    };
    let result = repo::configuration::rollback_configuration(
        &state.db,
        &user.tenant,
        &user.command_context(&idempotency_key),
        &command,
    )
    .await?;
    Ok(Json(map_response(result)?))
}

pub async fn simulate(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<SimulateConfigurationRequest>,
) -> V1Result<Json<ConfigurationSimulationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let inventory_owner_id = InventoryOwnerId::new(body.inventory_owner_id).map_err(validation)?;
    let facility_id = FacilityId::new(body.facility_id).map_err(validation)?;
    let effective_at = body.effective_at.parse().map_err(validation)?;
    let result = repo::configuration::simulate_configuration(
        &state.db,
        &user.tenant,
        SimulateConfigurationQuery {
            kind: map_kind(body.kind),
            inventory_owner_id,
            facility_id,
            effective_at,
        },
    )
    .await?;
    Ok(Json(ConfigurationSimulationResponse {
        kind: map_kind_to_api(result.kind),
        inventory_owner_id: result.inventory_owner_id.get(),
        facility_id: result.facility_id.get(),
        effective_at: result.effective_at.to_rfc3339(),
        matched_configuration: result.matched_configuration.map(map_response).transpose()?,
        evaluated_candidate_count: result.evaluated_candidate_count,
    }))
}

#[derive(Clone, Copy)]
enum Lifecycle {
    Submit,
    Approve,
    Retire,
}

async fn lifecycle(
    state: AppState,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    configuration_id: i64,
    body: ConfigurationLifecycleRequest,
    lifecycle: Lifecycle,
) -> V1Result<Json<ConfigurationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    let command = ConfigurationLifecycleCommand {
        configuration_id: ConfigurationVersionId::new(configuration_id).map_err(validation)?,
        expected_revision: body.expected_revision.get(),
    };
    let context = user.command_context(&idempotency_key);
    let result = match lifecycle {
        Lifecycle::Submit => {
            repo::configuration::submit_configuration(&state.db, &user.tenant, &context, &command)
                .await?
        }
        Lifecycle::Approve => {
            repo::configuration::approve_configuration(&state.db, &user.tenant, &context, &command)
                .await?
        }
        Lifecycle::Retire => {
            repo::configuration::retire_configuration(&state.db, &user.tenant, &context, &command)
                .await?
        }
    };
    Ok(Json(map_response(result)?))
}

fn map_scope(value: ApiConfigurationScope) -> V1Result<ConfigurationScope> {
    Ok(match value {
        ApiConfigurationScope::Tenant => ConfigurationScope::Tenant,
        ApiConfigurationScope::InventoryOwner { inventory_owner_id } => {
            ConfigurationScope::InventoryOwner {
                inventory_owner_id: InventoryOwnerId::new(inventory_owner_id)
                    .map_err(validation)?,
            }
        }
        ApiConfigurationScope::Facility { facility_id } => ConfigurationScope::Facility {
            facility_id: FacilityId::new(facility_id).map_err(validation)?,
        },
        ApiConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        } => ConfigurationScope::OwnerFacility {
            inventory_owner_id: InventoryOwnerId::new(inventory_owner_id).map_err(validation)?,
            facility_id: FacilityId::new(facility_id).map_err(validation)?,
        },
    })
}

fn map_scope_to_api(value: ConfigurationScope) -> ApiConfigurationScope {
    match value {
        ConfigurationScope::Tenant => ApiConfigurationScope::Tenant,
        ConfigurationScope::InventoryOwner { inventory_owner_id } => {
            ApiConfigurationScope::InventoryOwner {
                inventory_owner_id: inventory_owner_id.get(),
            }
        }
        ConfigurationScope::Facility { facility_id } => ApiConfigurationScope::Facility {
            facility_id: facility_id.get(),
        },
        ConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        } => ApiConfigurationScope::OwnerFacility {
            inventory_owner_id: inventory_owner_id.get(),
            facility_id: facility_id.get(),
        },
    }
}

fn map_window(
    effective_from: String,
    effective_until: Option<String>,
) -> V1Result<ConfigurationEffectiveWindow> {
    ConfigurationEffectiveWindow::new(
        effective_from.parse().map_err(validation)?,
        effective_until
            .map(|value| value.parse().map_err(validation))
            .transpose()?,
    )
    .map_err(validation)
}

const fn map_kind(value: ApiDecisionRuleKind) -> DecisionRuleKind {
    match value {
        ApiDecisionRuleKind::Receipt => DecisionRuleKind::Receipt,
        ApiDecisionRuleKind::Putaway => DecisionRuleKind::Putaway,
        ApiDecisionRuleKind::Allocation => DecisionRuleKind::Allocation,
        ApiDecisionRuleKind::Replenishment => DecisionRuleKind::Replenishment,
        ApiDecisionRuleKind::Wave => DecisionRuleKind::Wave,
        ApiDecisionRuleKind::Pick => DecisionRuleKind::Pick,
        ApiDecisionRuleKind::Pack => DecisionRuleKind::Pack,
        ApiDecisionRuleKind::Count => DecisionRuleKind::Count,
        ApiDecisionRuleKind::Document => DecisionRuleKind::Document,
        ApiDecisionRuleKind::Billing => DecisionRuleKind::Billing,
    }
}

const fn map_kind_to_api(value: DecisionRuleKind) -> ApiDecisionRuleKind {
    match value {
        DecisionRuleKind::Receipt => ApiDecisionRuleKind::Receipt,
        DecisionRuleKind::Putaway => ApiDecisionRuleKind::Putaway,
        DecisionRuleKind::Allocation => ApiDecisionRuleKind::Allocation,
        DecisionRuleKind::Replenishment => ApiDecisionRuleKind::Replenishment,
        DecisionRuleKind::Wave => ApiDecisionRuleKind::Wave,
        DecisionRuleKind::Pick => ApiDecisionRuleKind::Pick,
        DecisionRuleKind::Pack => ApiDecisionRuleKind::Pack,
        DecisionRuleKind::Count => ApiDecisionRuleKind::Count,
        DecisionRuleKind::Document => ApiDecisionRuleKind::Document,
        DecisionRuleKind::Billing => ApiDecisionRuleKind::Billing,
    }
}

const fn map_status(value: ApiConfigurationStatus) -> ConfigurationStatus {
    match value {
        ApiConfigurationStatus::Draft => ConfigurationStatus::Draft,
        ApiConfigurationStatus::PendingApproval => ConfigurationStatus::PendingApproval,
        ApiConfigurationStatus::Approved => ConfigurationStatus::Approved,
        ApiConfigurationStatus::Active => ConfigurationStatus::Active,
        ApiConfigurationStatus::Retired => ConfigurationStatus::Retired,
    }
}

const fn map_status_to_api(value: ConfigurationStatus) -> ApiConfigurationStatus {
    match value {
        ConfigurationStatus::Draft => ApiConfigurationStatus::Draft,
        ConfigurationStatus::PendingApproval => ApiConfigurationStatus::PendingApproval,
        ConfigurationStatus::Approved => ApiConfigurationStatus::Approved,
        ConfigurationStatus::Active => ApiConfigurationStatus::Active,
        ConfigurationStatus::Retired => ApiConfigurationStatus::Retired,
    }
}

fn map_rule(value: ApiDecisionRule) -> DecisionRuleDefinition {
    match value {
        ApiDecisionRule::Receipt {
            allow_unexpected,
            quarantine_unmapped_items,
            over_receipt_tolerance_basis_points,
        } => DecisionRuleDefinition::Receipt {
            allow_unexpected,
            quarantine_unmapped_items,
            over_receipt_tolerance_basis_points,
        },
        ApiDecisionRule::Putaway {
            require_zone_compatibility,
            enforce_location_capacity,
            allow_mixed_lots,
        } => DecisionRuleDefinition::Putaway {
            require_zone_compatibility,
            enforce_location_capacity,
            allow_mixed_lots,
        },
        ApiDecisionRule::Allocation {
            rotation,
            allow_partial,
            require_complete_line,
        } => DecisionRuleDefinition::Allocation {
            rotation: match rotation {
                ApiInventoryRotation::Fifo => InventoryRotation::Fifo,
                ApiInventoryRotation::Fefo => InventoryRotation::Fefo,
            },
            allow_partial,
            require_complete_line,
        },
        ApiDecisionRule::Replenishment {
            minimum_percent,
            target_percent,
            include_inbound_projection,
        } => DecisionRuleDefinition::Replenishment {
            minimum_percent,
            target_percent,
            include_inbound_projection,
        },
        ApiDecisionRule::Wave {
            max_orders,
            require_complete_allocation,
        } => DecisionRuleDefinition::Wave {
            max_orders,
            require_complete_allocation,
        },
        ApiDecisionRule::Pick {
            require_source_location_scan,
            require_item_scan,
            require_destination_container_scan,
        } => DecisionRuleDefinition::Pick {
            require_source_location_scan,
            require_item_scan,
            require_destination_container_scan,
        },
        ApiDecisionRule::Pack {
            require_station_scan,
            require_weight,
            allow_mixed_orders,
        } => DecisionRuleDefinition::Pack {
            require_station_scan,
            require_weight,
            allow_mixed_orders,
        },
        ApiDecisionRule::Count {
            absolute_tolerance,
            percentage_tolerance_basis_points,
            approval_threshold,
        } => DecisionRuleDefinition::Count {
            absolute_tolerance,
            percentage_tolerance_basis_points,
            approval_threshold,
        },
        ApiDecisionRule::Document {
            generate_packing_slip,
            generate_carton_label,
            require_tracking_barcode,
        } => DecisionRuleDefinition::Document {
            generate_packing_slip,
            generate_carton_label,
            require_tracking_barcode,
        },
        ApiDecisionRule::Billing {
            event_type,
            unit,
            currency,
            rate_minor,
            minimum_charge_minor,
        } => DecisionRuleDefinition::Billing {
            event_type: map_billable_event(event_type),
            unit: map_billing_unit(unit),
            currency,
            rate_minor,
            minimum_charge_minor,
        },
    }
}

fn map_rule_to_api(value: DecisionRuleDefinition) -> ApiDecisionRule {
    match value {
        DecisionRuleDefinition::Receipt {
            allow_unexpected,
            quarantine_unmapped_items,
            over_receipt_tolerance_basis_points,
        } => ApiDecisionRule::Receipt {
            allow_unexpected,
            quarantine_unmapped_items,
            over_receipt_tolerance_basis_points,
        },
        DecisionRuleDefinition::Putaway {
            require_zone_compatibility,
            enforce_location_capacity,
            allow_mixed_lots,
        } => ApiDecisionRule::Putaway {
            require_zone_compatibility,
            enforce_location_capacity,
            allow_mixed_lots,
        },
        DecisionRuleDefinition::Allocation {
            rotation,
            allow_partial,
            require_complete_line,
        } => ApiDecisionRule::Allocation {
            rotation: match rotation {
                InventoryRotation::Fifo => ApiInventoryRotation::Fifo,
                InventoryRotation::Fefo => ApiInventoryRotation::Fefo,
            },
            allow_partial,
            require_complete_line,
        },
        DecisionRuleDefinition::Replenishment {
            minimum_percent,
            target_percent,
            include_inbound_projection,
        } => ApiDecisionRule::Replenishment {
            minimum_percent,
            target_percent,
            include_inbound_projection,
        },
        DecisionRuleDefinition::Wave {
            max_orders,
            require_complete_allocation,
        } => ApiDecisionRule::Wave {
            max_orders,
            require_complete_allocation,
        },
        DecisionRuleDefinition::Pick {
            require_source_location_scan,
            require_item_scan,
            require_destination_container_scan,
        } => ApiDecisionRule::Pick {
            require_source_location_scan,
            require_item_scan,
            require_destination_container_scan,
        },
        DecisionRuleDefinition::Pack {
            require_station_scan,
            require_weight,
            allow_mixed_orders,
        } => ApiDecisionRule::Pack {
            require_station_scan,
            require_weight,
            allow_mixed_orders,
        },
        DecisionRuleDefinition::Count {
            absolute_tolerance,
            percentage_tolerance_basis_points,
            approval_threshold,
        } => ApiDecisionRule::Count {
            absolute_tolerance,
            percentage_tolerance_basis_points,
            approval_threshold,
        },
        DecisionRuleDefinition::Document {
            generate_packing_slip,
            generate_carton_label,
            require_tracking_barcode,
        } => ApiDecisionRule::Document {
            generate_packing_slip,
            generate_carton_label,
            require_tracking_barcode,
        },
        DecisionRuleDefinition::Billing {
            event_type,
            unit,
            currency,
            rate_minor,
            minimum_charge_minor,
        } => ApiDecisionRule::Billing {
            event_type: map_billable_event_to_api(event_type),
            unit: map_billing_unit_to_api(unit),
            currency,
            rate_minor,
            minimum_charge_minor,
        },
    }
}

const fn map_billable_event(value: ApiBillableEventType) -> BillableEventType {
    match value {
        ApiBillableEventType::ReceiptLine => BillableEventType::ReceiptLine,
        ApiBillableEventType::ReceivedUnit => BillableEventType::ReceivedUnit,
        ApiBillableEventType::PalletDay => BillableEventType::PalletDay,
        ApiBillableEventType::PickLine => BillableEventType::PickLine,
        ApiBillableEventType::PickedUnit => BillableEventType::PickedUnit,
        ApiBillableEventType::PackedCarton => BillableEventType::PackedCarton,
        ApiBillableEventType::ShippedUnit => BillableEventType::ShippedUnit,
        ApiBillableEventType::ReturnUnit => BillableEventType::ReturnUnit,
        ApiBillableEventType::RelabelUnit => BillableEventType::RelabelUnit,
        ApiBillableEventType::RefurbishmentUnit => BillableEventType::RefurbishmentUnit,
        ApiBillableEventType::KitUnit => BillableEventType::KitUnit,
        ApiBillableEventType::AssemblyUnit => BillableEventType::AssemblyUnit,
        ApiBillableEventType::Accessorial => BillableEventType::Accessorial,
        ApiBillableEventType::DetentionHour => BillableEventType::DetentionHour,
        ApiBillableEventType::ValueAddedServiceUnit => BillableEventType::ValueAddedServiceUnit,
    }
}

const fn map_billable_event_to_api(value: BillableEventType) -> ApiBillableEventType {
    match value {
        BillableEventType::ReceiptLine => ApiBillableEventType::ReceiptLine,
        BillableEventType::ReceivedUnit => ApiBillableEventType::ReceivedUnit,
        BillableEventType::PalletDay => ApiBillableEventType::PalletDay,
        BillableEventType::PickLine => ApiBillableEventType::PickLine,
        BillableEventType::PickedUnit => ApiBillableEventType::PickedUnit,
        BillableEventType::PackedCarton => ApiBillableEventType::PackedCarton,
        BillableEventType::ShippedUnit => ApiBillableEventType::ShippedUnit,
        BillableEventType::ReturnUnit => ApiBillableEventType::ReturnUnit,
        BillableEventType::RelabelUnit => ApiBillableEventType::RelabelUnit,
        BillableEventType::RefurbishmentUnit => ApiBillableEventType::RefurbishmentUnit,
        BillableEventType::KitUnit => ApiBillableEventType::KitUnit,
        BillableEventType::AssemblyUnit => ApiBillableEventType::AssemblyUnit,
        BillableEventType::Accessorial => ApiBillableEventType::Accessorial,
        BillableEventType::DetentionHour => ApiBillableEventType::DetentionHour,
        BillableEventType::ValueAddedServiceUnit => ApiBillableEventType::ValueAddedServiceUnit,
    }
}

const fn map_billing_unit(value: ApiBillingUnit) -> BillingUnit {
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

const fn map_billing_unit_to_api(value: BillingUnit) -> ApiBillingUnit {
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

fn map_response(value: ConfigurationReadModel) -> V1Result<ConfigurationResponse> {
    Ok(ConfigurationResponse {
        configuration_id: value.configuration_id.get(),
        revision: Revision::new(value.revision).map_err(invalid_result)?,
        scope: map_scope_to_api(value.scope),
        status: map_status_to_api(value.status),
        effective_from: value.effective_window.effective_from.to_rfc3339(),
        effective_until: value
            .effective_window
            .effective_until
            .map(|timestamp| timestamp.to_rfc3339()),
        rule: map_rule_to_api(value.definition),
        created_by: value.created_by.get(),
        created_at: value.created_at.to_rfc3339(),
        submitted_by: value.submitted_by.map(|user| user.get()),
        submitted_at: value.submitted_at.map(|timestamp| timestamp.to_rfc3339()),
        approved_by: value.approved_by.map(|user| user.get()),
        approved_at: value.approved_at.map(|timestamp| timestamp.to_rfc3339()),
        activated_by: value.activated_by.map(|user| user.get()),
        activated_at: value.activated_at.map(|timestamp| timestamp.to_rfc3339()),
        retired_by: value.retired_by.map(|user| user.get()),
        retired_at: value.retired_at.map(|timestamp| timestamp.to_rfc3339()),
        rollback_of_configuration_id: value
            .rollback_of_configuration_id
            .map(|configuration| configuration.get()),
    })
}

fn cursor_filter(request: &ConfigurationPageRequest) -> String {
    format!(
        "{}.{}.{}.{}",
        request.kind.map_or("all", api_kind_name),
        request.status.map_or("all", api_status_name),
        request
            .inventory_owner_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        request
            .facility_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
    )
}

fn encode_cursor(
    cursor: ConfigurationCursor,
    request: &ConfigurationPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{CURSOR_PREFIX}{}.{:016x}",
        cursor_filter(request),
        cursor.after_configuration_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid configuration cursor"))
}

fn decode_cursor(
    cursor: &OpaqueCursor,
    request: &ConfigurationPageRequest,
) -> V1Result<ConfigurationCursor> {
    let encoded = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("configuration"))?;
    let (filter, id) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("configuration"))?;
    if filter != cursor_filter(request) || id.len() != 16 {
        return Err(V1Error::invalid_cursor_for("configuration"));
    }
    let id =
        i64::from_str_radix(id, 16).map_err(|_| V1Error::invalid_cursor_for("configuration"))?;
    Ok(ConfigurationCursor {
        after_configuration_id: ConfigurationVersionId::new(id)
            .map_err(|_| V1Error::invalid_cursor_for("configuration"))?,
    })
}

const fn api_kind_name(value: ApiDecisionRuleKind) -> &'static str {
    match value {
        ApiDecisionRuleKind::Receipt => "receipt",
        ApiDecisionRuleKind::Putaway => "putaway",
        ApiDecisionRuleKind::Allocation => "allocation",
        ApiDecisionRuleKind::Replenishment => "replenishment",
        ApiDecisionRuleKind::Wave => "wave",
        ApiDecisionRuleKind::Pick => "pick",
        ApiDecisionRuleKind::Pack => "pack",
        ApiDecisionRuleKind::Count => "count",
        ApiDecisionRuleKind::Document => "document",
        ApiDecisionRuleKind::Billing => "billing",
    }
}

const fn api_status_name(value: ApiConfigurationStatus) -> &'static str {
    match value {
        ApiConfigurationStatus::Draft => "draft",
        ApiConfigurationStatus::PendingApproval => "pending_approval",
        ApiConfigurationStatus::Approved => "approved",
        ApiConfigurationStatus::Active => "active",
        ApiConfigurationStatus::Retired => "retired",
    }
}

fn validation(error: impl std::fmt::Display) -> V1Error {
    AppError::bad_request(error.to_string()).into()
}

fn invalid_result(error: impl std::fmt::Display) -> V1Error {
    V1Error::internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use wareboxes_api_contract::v1::PageLimit;

    use super::*;

    #[test]
    fn cursor_is_bound_to_every_configuration_filter() {
        let request = ConfigurationPageRequest {
            kind: Some(ApiDecisionRuleKind::Billing),
            status: Some(ApiConfigurationStatus::Active),
            inventory_owner_id: Some(2),
            facility_id: Some(3),
            cursor: None,
            limit: PageLimit::default(),
        };
        let cursor = ConfigurationCursor {
            after_configuration_id: ConfigurationVersionId::new(11).unwrap(),
        };
        let encoded = encode_cursor(cursor, &request).unwrap();
        assert_eq!(decode_cursor(&encoded, &request).unwrap(), cursor);
        let mut changed = request;
        changed.status = Some(ApiConfigurationStatus::Retired);
        assert!(decode_cursor(&encoded, &changed).is_err());
    }

    #[test]
    fn api_mapping_preserves_every_billing_field() {
        let rule = ApiDecisionRule::Billing {
            event_type: ApiBillableEventType::PickedUnit,
            unit: ApiBillingUnit::Each,
            currency: "USD".into(),
            rate_minor: 25,
            minimum_charge_minor: 100,
        };
        assert_eq!(map_rule_to_api(map_rule(rule.clone())), rule);
    }
}
