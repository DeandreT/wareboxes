//! Owner-scoped 3PL billing ledger and reconciliation.

mod commands;
mod decision_policy;
mod models;
mod query;
mod reconciliation_policy;
mod review_export;

pub use commands::{
    activate_contract, capture_billable_event, capture_storage_snapshot, close_contract,
    configure_rate, create_contract, generate_run,
};
pub use query::workspace;
pub use review_export::{export_run, review_run};

use serde::Serialize;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    BillableEventType, BillingUnit, FacilityId, InventoryOwnerId, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;
use crate::repo::orders::next_outbox_sequence_tx;

const PERMISSION: &str = "admin";

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

const fn event_name(value: BillableEventType) -> &'static str {
    match value {
        BillableEventType::ReceiptLine => "receipt_line",
        BillableEventType::ReceivedUnit => "received_unit",
        BillableEventType::PalletDay => "pallet_day",
        BillableEventType::PickLine => "pick_line",
        BillableEventType::PickedUnit => "picked_unit",
        BillableEventType::PackedCarton => "packed_carton",
        BillableEventType::ShippedUnit => "shipped_unit",
        BillableEventType::ReturnUnit => "return_unit",
        BillableEventType::RelabelUnit => "relabel_unit",
        BillableEventType::RefurbishmentUnit => "refurbishment_unit",
        BillableEventType::KitUnit => "kit_unit",
        BillableEventType::AssemblyUnit => "assembly_unit",
        BillableEventType::Accessorial => "accessorial",
        BillableEventType::DetentionHour => "detention_hour",
        BillableEventType::ValueAddedServiceUnit => "value_added_service_unit",
    }
}

fn parse_event(value: &str) -> AppResult<BillableEventType> {
    match value {
        "receipt_line" => Ok(BillableEventType::ReceiptLine),
        "received_unit" => Ok(BillableEventType::ReceivedUnit),
        "pallet_day" => Ok(BillableEventType::PalletDay),
        "pick_line" => Ok(BillableEventType::PickLine),
        "picked_unit" => Ok(BillableEventType::PickedUnit),
        "packed_carton" => Ok(BillableEventType::PackedCarton),
        "shipped_unit" => Ok(BillableEventType::ShippedUnit),
        "return_unit" => Ok(BillableEventType::ReturnUnit),
        "relabel_unit" => Ok(BillableEventType::RelabelUnit),
        "refurbishment_unit" => Ok(BillableEventType::RefurbishmentUnit),
        "kit_unit" => Ok(BillableEventType::KitUnit),
        "assembly_unit" => Ok(BillableEventType::AssemblyUnit),
        "accessorial" => Ok(BillableEventType::Accessorial),
        "detention_hour" => Ok(BillableEventType::DetentionHour),
        "value_added_service_unit" => Ok(BillableEventType::ValueAddedServiceUnit),
        _ => Err(AppError::internal("invalid stored billable event type")),
    }
}

const fn unit_name(value: BillingUnit) -> &'static str {
    match value {
        BillingUnit::Event => "event",
        BillingUnit::Each => "each",
        BillingUnit::Case => "case",
        BillingUnit::Pallet => "pallet",
        BillingUnit::Carton => "carton",
        BillingUnit::Hour => "hour",
        BillingUnit::Day => "day",
    }
}

fn parse_unit(value: &str) -> AppResult<BillingUnit> {
    match value {
        "event" => Ok(BillingUnit::Event),
        "each" => Ok(BillingUnit::Each),
        "case" => Ok(BillingUnit::Case),
        "pallet" => Ok(BillingUnit::Pallet),
        "carton" => Ok(BillingUnit::Carton),
        "hour" => Ok(BillingUnit::Hour),
        "day" => Ok(BillingUnit::Day),
        _ => Err(AppError::internal("invalid stored billing unit")),
    }
}

fn require_owner(scope: &ScopeBindings, owner_id: InventoryOwnerId) -> AppResult<()> {
    if scope.includes_inventory_owner(owner_id.get()) {
        Ok(())
    } else {
        Err(AppError::not_found("billing record"))
    }
}

fn require_facility(scope: &ScopeBindings, facility_id: FacilityId) -> AppResult<()> {
    if scope.includes_facility(facility_id.get()) {
        Ok(())
    } else {
        Err(AppError::not_found("billing record"))
    }
}

fn require_record_scope(
    scope: &ScopeBindings,
    owner_id: InventoryOwnerId,
    facility_id: Option<FacilityId>,
) -> AppResult<()> {
    require_owner(scope, owner_id)?;
    if let Some(facility_id) = facility_id {
        require_facility(scope, facility_id)?;
    }
    Ok(())
}

struct BillingOutboxEvent<'a> {
    tenant_id: TenantId,
    actor_id: UserId,
    owner_id: InventoryOwnerId,
    facility_id: Option<FacilityId>,
    aggregate_type: &'a str,
    aggregate_id: i64,
    transition: &'a str,
    occurred_at: Timestamp,
}

async fn enqueue_event_tx<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: BillingOutboxEvent<'_>,
    payload: &T,
) -> AppResult<()> {
    let BillingOutboxEvent {
        tenant_id,
        actor_id,
        owner_id,
        facility_id,
        aggregate_type,
        aggregate_id,
        transition,
        occurred_at,
    } = event;
    let ordering_key = format!("{aggregate_type}:{aggregate_id}");
    let event_key = format!("{ordering_key}:{transition}");
    let event_type = format!("billing.{aggregate_type}.{transition}");
    let aggregate_id = aggregate_id.to_string();
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let payload = serde_json::to_value(payload).map_err(internal)?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(owner_id),
            facility_id,
            actor_user_id: Some(actor_id.get()),
            event_key: &event_key,
            aggregate_type,
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: &event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

fn require_access_actor(
    access: &TenantAccess,
    context: &wareboxes_application::CommandContext,
) -> AppResult<()> {
    context.require_actor(access.tenant_id, access.user_id)?;
    Ok(())
}
