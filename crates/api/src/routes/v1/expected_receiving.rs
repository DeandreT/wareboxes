use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use wareboxes_api_contract::v1::{
    ConfigurationScope as ApiConfigurationScope, ConfirmExpectedReceiptRequest,
    ConfirmUnexpectedReceiptRequest, ExpectedReceiptConfirmationResponse,
    ExpectedReceiptDisposition, ExpectedReceiptExceptionReason, ExpectedReceiptLine,
    ExpectedReceiptLineStatus, ExpectedReceiptQuarantineReason, ExpectedReceivingLoadStatus,
    ExpectedReceivingLocation, ExpectedReceivingSessionResponse, InventoryBalanceStatus,
    ReceiptPolicyExpectation as ApiReceiptPolicyExpectation,
    ReceiptPolicyResponse as ApiReceiptPolicyResponse,
    ReceiptPolicySource as ApiReceiptPolicySource, UnexpectedReceiptConfirmationResponse,
    UnexpectedReceiptReason as ApiUnexpectedReceiptReason,
};
use wareboxes_application::receipt_policy::{
    ReceiptPolicyExpectation, ReceiptPolicyReadModel, ReceiptPolicySource,
};
use wareboxes_core::models::{
    InboundReceiptExceptionReason, InboundReceiptQuarantineReason, InventoryStatus, LoadLineStatus,
    LoadStatus, ReceiveExpectedInventoryResult, UnexpectedReceiptReason,
};
use wareboxes_domain::{ConfigurationScope, ConfigurationVersionId};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::request_context::IdempotencyKey;
use crate::state::AppState;
use crate::{permissions, repo};

const PERMISSION: &str = "wms";
const MAX_BARCODE_LENGTH: usize = 200;
const MAX_DIMENSION_LENGTH: usize = 200;
const MAX_NOTE_LENGTH: usize = 1_000;

pub async fn get_session_by_execution_barcode(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(execution_barcode): Path<String>,
) -> V1Result<Json<ExpectedReceivingSessionResponse>> {
    if !permissions::user_has_permission(&state.db, user.tenant.tenant_id, user.user.id, PERMISSION)
        .await?
    {
        return Err(AppError::not_found("expected receiving load").into());
    }
    let session = repo::expected_receiving::get_expected_receiving_session_by_execution_barcode(
        &state.db,
        &user.tenant,
        &execution_barcode,
    )
    .await?;

    Ok(Json(map_session(session)))
}

pub async fn get_session(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(load_id): Path<i64>,
) -> V1Result<Json<ExpectedReceivingSessionResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(load_id, "load ID")?;
    let session =
        repo::expected_receiving::get_expected_receiving_session(&state.db, &user.tenant, load_id)
            .await?;

    Ok(Json(map_session(session)))
}

pub async fn confirm(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(load_line_id): Path<i64>,
    Json(body): Json<ConfirmExpectedReceiptRequest>,
) -> V1Result<Json<ExpectedReceiptConfirmationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(load_line_id, "load line ID")?;
    validate_confirmation(&body)?;
    let mapped = map_confirmation(&body)?;
    let context = user.command_context(&idempotency_key);
    let result = repo::inbound_receipt::confirm_expected_receipt(
        &state.db,
        &user.tenant,
        &context,
        load_line_id,
        &mapped.command,
    )
    .await?;

    Ok(Json(map_confirmation_result(
        result,
        mapped.disposition,
        mapped.quantity,
    )?))
}

pub async fn confirm_unexpected(
    State(state): State<AppState>,
    user: CurrentTenant,
    idempotency_key: IdempotencyKey,
    Path(load_id): Path<i64>,
    Json(body): Json<ConfirmUnexpectedReceiptRequest>,
) -> V1Result<Json<UnexpectedReceiptConfirmationResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(load_id, "load ID")?;
    validate_unexpected_confirmation(&body)?;
    let expiration = parse_timestamp(body.expiration.as_deref(), "expiration")?;
    let expected_policy = map_receipt_policy_expectation(body.expected_policy.clone())?;
    let context = user.command_context(&idempotency_key);
    let result = repo::unexpected_receipt::confirm_unexpected_receipt(
        &state.db,
        &user.tenant,
        &context,
        load_id,
        &repo::unexpected_receipt::ConfirmUnexpectedReceiptCommand {
            item_barcode: &body.item_barcode,
            receiving_location_barcode: &body.receiving_location_barcode,
            quantity: body.quantity,
            license_plate_barcode: body.license_plate_barcode.as_deref(),
            lot: body.lot.as_deref(),
            serial: body.serial.as_deref(),
            expiration,
            reason: map_unexpected_reason(body.reason),
            note: body.note.as_deref(),
            expected_policy: &expected_policy,
        },
    )
    .await?;
    Ok(Json(map_unexpected_result(result)?))
}

struct MappedConfirmation<'a> {
    command: repo::inbound_receipt::ConfirmExpectedReceiptCommand<'a>,
    disposition: ExpectedReceiptDisposition,
    quantity: i64,
}

fn map_confirmation(body: &ConfirmExpectedReceiptRequest) -> V1Result<MappedConfirmation<'_>> {
    Ok(match body {
        ConfirmExpectedReceiptRequest::Received {
            item_barcode,
            receiving_location_barcode,
            quantity,
            license_plate_barcode,
            lot,
            serial,
            expiration,
        } => MappedConfirmation {
            command: repo::inbound_receipt::ConfirmExpectedReceiptCommand::Received {
                item_barcode,
                receiving_location_barcode,
                quantity: *quantity,
                license_plate_barcode: license_plate_barcode.as_deref(),
                lot: lot.as_deref(),
                serial: serial.as_deref(),
                expiration: parse_timestamp(expiration.as_deref(), "expiration")?,
            },
            disposition: ExpectedReceiptDisposition::Received,
            quantity: *quantity,
        },
        ConfirmExpectedReceiptRequest::Quarantined {
            item_barcode,
            receiving_location_barcode,
            quantity,
            license_plate_barcode,
            lot,
            serial,
            expiration,
            reason,
            note,
        } => MappedConfirmation {
            command: repo::inbound_receipt::ConfirmExpectedReceiptCommand::Quarantined {
                item_barcode,
                receiving_location_barcode,
                quantity: *quantity,
                license_plate_barcode: license_plate_barcode.as_deref(),
                lot: lot.as_deref(),
                serial: serial.as_deref(),
                expiration: parse_timestamp(expiration.as_deref(), "expiration")?,
                reason: map_quarantine_reason(*reason),
                note: note.as_deref(),
            },
            disposition: ExpectedReceiptDisposition::Quarantined,
            quantity: *quantity,
        },
        ConfirmExpectedReceiptRequest::Rejected {
            item_barcode,
            quantity,
            reason,
            note,
        } => MappedConfirmation {
            command: repo::inbound_receipt::ConfirmExpectedReceiptCommand::Rejected {
                item_barcode,
                quantity: *quantity,
                reason: map_exception_reason(*reason),
                note: note.as_deref(),
            },
            disposition: ExpectedReceiptDisposition::Rejected,
            quantity: *quantity,
        },
        ConfirmExpectedReceiptRequest::Missing {
            quantity,
            reason,
            note,
        } => MappedConfirmation {
            command: repo::inbound_receipt::ConfirmExpectedReceiptCommand::Missing {
                quantity: *quantity,
                reason: map_exception_reason(*reason),
                note: note.as_deref(),
            },
            disposition: ExpectedReceiptDisposition::Missing,
            quantity: *quantity,
        },
    })
}

fn map_confirmation_result(
    result: ReceiveExpectedInventoryResult,
    disposition: ExpectedReceiptDisposition,
    quantity: i64,
) -> V1Result<ExpectedReceiptConfirmationResponse> {
    Ok(ExpectedReceiptConfirmationResponse {
        load_id: result.load_id,
        load_line_id: result.load_line_id,
        disposition,
        quantity,
        inventory_transaction_id: result.inventory_transaction_id,
        inventory_balance_id: result.inventory_balance_id,
        item_batch_id: result.item_batch_id,
        license_plate_id: result.license_plate_id,
        inventory_hold_id: result.inventory_hold_id,
        inventory_status: result.inventory_status.map(map_inventory_status),
        line_status: map_line_status(result.line_status),
        load_status: map_load_status(result.load_status)?,
        cumulative_received_quantity: result.cumulative_received_qty,
        cumulative_rejected_quantity: result.cumulative_rejected_qty,
        cumulative_missing_quantity: result.cumulative_missing_qty,
        remaining_quantity: result.remaining_quantity,
        receive_completed: result.receive_completed,
    })
}

fn validate_confirmation(body: &ConfirmExpectedReceiptRequest) -> V1Result<()> {
    let quantity = match body {
        ConfirmExpectedReceiptRequest::Received {
            item_barcode,
            receiving_location_barcode,
            quantity,
            license_plate_barcode,
            lot,
            serial,
            ..
        } => {
            validate_required_text(item_barcode, "item_barcode", MAX_BARCODE_LENGTH)?;
            validate_required_text(
                receiving_location_barcode,
                "receiving_location_barcode",
                MAX_BARCODE_LENGTH,
            )?;
            validate_optional_text(
                license_plate_barcode.as_deref(),
                "license_plate_barcode",
                MAX_BARCODE_LENGTH,
            )?;
            validate_optional_text(lot.as_deref(), "lot", MAX_DIMENSION_LENGTH)?;
            validate_optional_text(serial.as_deref(), "serial", MAX_DIMENSION_LENGTH)?;
            *quantity
        }
        ConfirmExpectedReceiptRequest::Quarantined {
            item_barcode,
            receiving_location_barcode,
            quantity,
            license_plate_barcode,
            lot,
            serial,
            reason,
            note,
            ..
        } => {
            validate_required_text(item_barcode, "item_barcode", MAX_BARCODE_LENGTH)?;
            validate_required_text(
                receiving_location_barcode,
                "receiving_location_barcode",
                MAX_BARCODE_LENGTH,
            )?;
            validate_optional_text(
                license_plate_barcode.as_deref(),
                "license_plate_barcode",
                MAX_BARCODE_LENGTH,
            )?;
            validate_optional_text(lot.as_deref(), "lot", MAX_DIMENSION_LENGTH)?;
            validate_optional_text(serial.as_deref(), "serial", MAX_DIMENSION_LENGTH)?;
            validate_optional_text(note.as_deref(), "note", MAX_NOTE_LENGTH)?;
            if *reason == ExpectedReceiptQuarantineReason::Other && note.is_none() {
                return Err(invalid("note is required when quarantine reason is other"));
            }
            *quantity
        }
        ConfirmExpectedReceiptRequest::Rejected {
            item_barcode,
            quantity,
            reason,
            note,
        } => {
            validate_required_text(item_barcode, "item_barcode", MAX_BARCODE_LENGTH)?;
            validate_exception(*reason, note.as_deref())?;
            *quantity
        }
        ConfirmExpectedReceiptRequest::Missing {
            quantity,
            reason,
            note,
        } => {
            validate_exception(*reason, note.as_deref())?;
            *quantity
        }
    };
    require_positive(quantity, "quantity")
}

fn map_quarantine_reason(
    reason: ExpectedReceiptQuarantineReason,
) -> InboundReceiptQuarantineReason {
    match reason {
        ExpectedReceiptQuarantineReason::Damaged => InboundReceiptQuarantineReason::Damaged,
        ExpectedReceiptQuarantineReason::QualityInspection => {
            InboundReceiptQuarantineReason::QualityInspection
        }
        ExpectedReceiptQuarantineReason::CountDiscrepancy => {
            InboundReceiptQuarantineReason::CountDiscrepancy
        }
        ExpectedReceiptQuarantineReason::WrongItem => InboundReceiptQuarantineReason::WrongItem,
        ExpectedReceiptQuarantineReason::Other => InboundReceiptQuarantineReason::Other,
    }
}

fn map_inventory_status(status: InventoryStatus) -> InventoryBalanceStatus {
    match status {
        InventoryStatus::Available => InventoryBalanceStatus::Available,
        InventoryStatus::Hold => InventoryBalanceStatus::Hold,
        InventoryStatus::Damaged => InventoryBalanceStatus::Damaged,
        InventoryStatus::Quarantine => InventoryBalanceStatus::Quarantine,
    }
}

fn map_unexpected_reason(reason: ApiUnexpectedReceiptReason) -> UnexpectedReceiptReason {
    match reason {
        ApiUnexpectedReceiptReason::Excess => UnexpectedReceiptReason::Excess,
        ApiUnexpectedReceiptReason::UnexpectedItem => UnexpectedReceiptReason::UnexpectedItem,
        ApiUnexpectedReceiptReason::BlindReceipt => UnexpectedReceiptReason::BlindReceipt,
        ApiUnexpectedReceiptReason::MisShipped => UnexpectedReceiptReason::MisShipped,
        ApiUnexpectedReceiptReason::Other => UnexpectedReceiptReason::Other,
    }
}

fn map_unexpected_result(
    outcome: repo::unexpected_receipt::ConfirmUnexpectedReceiptOutcome,
) -> V1Result<UnexpectedReceiptConfirmationResponse> {
    let result = outcome.result;
    let reason = match result.reason {
        UnexpectedReceiptReason::Excess => ApiUnexpectedReceiptReason::Excess,
        UnexpectedReceiptReason::UnexpectedItem => ApiUnexpectedReceiptReason::UnexpectedItem,
        UnexpectedReceiptReason::BlindReceipt => ApiUnexpectedReceiptReason::BlindReceipt,
        UnexpectedReceiptReason::MisShipped => ApiUnexpectedReceiptReason::MisShipped,
        UnexpectedReceiptReason::Other => ApiUnexpectedReceiptReason::Other,
    };
    Ok(UnexpectedReceiptConfirmationResponse {
        unexpected_receipt_id: result.unexpected_receipt_id,
        load_id: result.load_id,
        inventory_owner_id: result.inventory_owner_id,
        facility_id: result.facility_id,
        item_id: result.item_id,
        uom: result.uom,
        quantity: result.quantity,
        receiving_location_id: result.receiving_location_id,
        observed_item_barcode: result.observed_item_barcode,
        observed_receiving_location_barcode: result.observed_receiving_location_barcode,
        inventory_transaction_id: result.inventory_transaction_id,
        inventory_balance_id: result.inventory_balance_id,
        item_batch_id: result.item_batch_id,
        license_plate_id: result.license_plate_id,
        license_plate_barcode: result.license_plate_barcode,
        lot: result.lot,
        serial: result.serial,
        expiration: result.expiration.map(|value| value.to_rfc3339()),
        inventory_hold_id: result.inventory_hold_id,
        inventory_status: map_inventory_status(result.inventory_status),
        reason,
        note: result.note,
        load_status: map_load_status(result.load_status)?,
        confirmed_by_user_id: result.confirmed_by_user_id,
        confirmed_at: result.confirmed_at.to_rfc3339(),
        receipt_policy: map_receipt_policy(outcome.receipt_policy),
    })
}

fn map_receipt_policy_expectation(
    value: ApiReceiptPolicyExpectation,
) -> V1Result<ReceiptPolicyExpectation> {
    let expectation = ReceiptPolicyExpectation {
        source: match value.source {
            ApiReceiptPolicySource::ProductDefault => ReceiptPolicySource::ProductDefault,
            ApiReceiptPolicySource::Configuration => ReceiptPolicySource::Configuration,
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
        Err(AppError::bad_request("receipt policy expectation is invalid").into())
    }
}

fn map_receipt_policy(value: ReceiptPolicyReadModel) -> ApiReceiptPolicyResponse {
    ApiReceiptPolicyResponse {
        source: match value.source {
            ReceiptPolicySource::ProductDefault => ApiReceiptPolicySource::ProductDefault,
            ReceiptPolicySource::Configuration => ApiReceiptPolicySource::Configuration,
        },
        configuration_id: value.configuration_id.map(|id| id.get()),
        configuration_revision: value.configuration_revision,
        configuration_scope: value.configuration_scope.map(|scope| match scope {
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
        }),
        allow_unexpected: value.allow_unexpected,
        quarantine_unmapped_items: value.quarantine_unmapped_items,
        over_receipt_tolerance_basis_points: value.over_receipt_tolerance_basis_points,
        policy_hash: value.policy_hash,
    }
}

fn validate_unexpected_confirmation(body: &ConfirmUnexpectedReceiptRequest) -> V1Result<()> {
    validate_required_text(&body.item_barcode, "item_barcode", MAX_BARCODE_LENGTH)?;
    validate_required_text(
        &body.receiving_location_barcode,
        "receiving_location_barcode",
        MAX_BARCODE_LENGTH,
    )?;
    validate_optional_text(
        body.license_plate_barcode.as_deref(),
        "license_plate_barcode",
        MAX_BARCODE_LENGTH,
    )?;
    validate_optional_text(body.lot.as_deref(), "lot", MAX_DIMENSION_LENGTH)?;
    validate_optional_text(body.serial.as_deref(), "serial", MAX_DIMENSION_LENGTH)?;
    validate_optional_text(body.note.as_deref(), "note", MAX_NOTE_LENGTH)?;
    if body.reason == ApiUnexpectedReceiptReason::Other && body.note.is_none() {
        return Err(invalid(
            "note is required when unexpected receipt reason is other",
        ));
    }
    require_positive(body.quantity, "quantity")
}

fn validate_exception(reason: ExpectedReceiptExceptionReason, note: Option<&str>) -> V1Result<()> {
    validate_optional_text(note, "note", MAX_NOTE_LENGTH)?;
    if reason == ExpectedReceiptExceptionReason::Other && note.is_none() {
        return Err(invalid("note is required when reason is other"));
    }
    Ok(())
}

fn validate_required_text(value: &str, field: &str, maximum: usize) -> V1Result<()> {
    if value.trim() != value || value.is_empty() {
        return Err(invalid(format!("{field} must be trimmed and nonempty")));
    }
    if value.chars().count() > maximum {
        return Err(invalid(format!(
            "{field} cannot exceed {maximum} characters"
        )));
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, field: &str, maximum: usize) -> V1Result<()> {
    if let Some(value) = value {
        validate_required_text(value, field, maximum)?;
    }
    Ok(())
}

fn parse_timestamp(value: Option<&str>, field: &str) -> V1Result<Option<DateTime<Utc>>> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .map(|timestamp| timestamp.with_timezone(&Utc))
                .map_err(|_| invalid(format!("{field} must be an RFC 3339 timestamp")))
        })
        .transpose()
}

fn map_exception_reason(reason: ExpectedReceiptExceptionReason) -> InboundReceiptExceptionReason {
    match reason {
        ExpectedReceiptExceptionReason::Damaged => InboundReceiptExceptionReason::Damaged,
        ExpectedReceiptExceptionReason::QualityRejected => {
            InboundReceiptExceptionReason::QualityRejected
        }
        ExpectedReceiptExceptionReason::ShortShipment => {
            InboundReceiptExceptionReason::ShortShipment
        }
        ExpectedReceiptExceptionReason::CountDiscrepancy => {
            InboundReceiptExceptionReason::CountDiscrepancy
        }
        ExpectedReceiptExceptionReason::WrongItem => InboundReceiptExceptionReason::WrongItem,
        ExpectedReceiptExceptionReason::Other => InboundReceiptExceptionReason::Other,
    }
}

fn map_line_status(status: LoadLineStatus) -> ExpectedReceiptLineStatus {
    match status {
        LoadLineStatus::Pending => ExpectedReceiptLineStatus::Pending,
        LoadLineStatus::Partial => ExpectedReceiptLineStatus::Partial,
        LoadLineStatus::Received => ExpectedReceiptLineStatus::Received,
        LoadLineStatus::Rejected => ExpectedReceiptLineStatus::Rejected,
        LoadLineStatus::Missing => ExpectedReceiptLineStatus::Missing,
    }
}

fn map_load_status(status: LoadStatus) -> V1Result<ExpectedReceivingLoadStatus> {
    match status {
        LoadStatus::Arrived => Ok(ExpectedReceivingLoadStatus::Arrived),
        LoadStatus::Receiving => Ok(ExpectedReceivingLoadStatus::Receiving),
        LoadStatus::Received => Ok(ExpectedReceivingLoadStatus::Received),
        _ => Err(V1Error::internal(
            "expected receipt returned an invalid load status",
        )),
    }
}

fn map_session(
    session: repo::expected_receiving::ExpectedReceivingSession,
) -> ExpectedReceivingSessionResponse {
    ExpectedReceivingSessionResponse {
        load_id: session.load_id,
        inventory_owner_id: session.inventory_owner_id,
        facility_id: session.facility_id,
        reference_number: session.reference_number,
        status: match session.status {
            repo::expected_receiving::ExpectedReceivingLoadStatus::Arrived => {
                ExpectedReceivingLoadStatus::Arrived
            }
            repo::expected_receiving::ExpectedReceivingLoadStatus::Receiving => {
                ExpectedReceivingLoadStatus::Receiving
            }
            repo::expected_receiving::ExpectedReceivingLoadStatus::Received => {
                ExpectedReceivingLoadStatus::Received
            }
        },
        expected_seal: session.expected_seal,
        receiving_location: ExpectedReceivingLocation {
            location_id: session.receiving_location.location_id,
            barcode: session.receiving_location.barcode,
            name: session.receiving_location.name,
        },
        receipt_policy: map_receipt_policy(session.receipt_policy),
        lines: session.lines.into_iter().map(map_line).collect(),
    }
}

fn map_line(line: repo::expected_receiving::ExpectedReceiptLine) -> ExpectedReceiptLine {
    ExpectedReceiptLine {
        load_line_id: line.load_line_id,
        item_id: line.item_id,
        item_description: line.item_description,
        uom: line.uom,
        item_barcodes: line.item_barcodes,
        expected_quantity: line.expected_quantity,
        received_quantity: line.received_quantity,
        rejected_quantity: line.rejected_quantity,
        missing_quantity: line.missing_quantity,
        remaining_quantity: line.remaining_quantity,
        lot: line.lot,
        serial: line.serial,
        expiration: line.expiration.map(|timestamp| timestamp.to_rfc3339()),
    }
}

fn require_positive(value: i64, label: &str) -> V1Result<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(invalid(format!("{label} must be positive")))
    }
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}
