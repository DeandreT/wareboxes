use sqlx::Row;
use wareboxes_application::automation::{
    AutomationCommandReadModel, AutomationCommandStatus, AutomationDeviceReadModel,
    EnqueueAutomationCommand, ShippingDocumentPrintContext,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::shipping::{
    CancelShipmentDocumentPrintCommand, CANCEL_SHIPMENT_DOCUMENT_PRINT_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    AutomationCommandId, AutomationDeviceCommand, AutomationDeviceId, AutomationPrintFormat,
    AutomationPrinterCommand, AutomationRecoveryPolicy, ShipmentDocumentId, ShipmentId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{current_scope_tx, require_permission_tx};
use crate::repo::automation::{self, mapping};

const MAX_PRINT_COPIES: u16 = 100;
const MAX_PRINT_HISTORY_PAGE: u16 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShipmentDocumentPrintPage {
    pub items: Vec<AutomationCommandReadModel>,
    pub next_command_id: Option<AutomationCommandId>,
}

struct PrintableDocument {
    inventory_owner_id: wareboxes_domain::InventoryOwnerId,
    shipment_id: ShipmentId,
    content: String,
    content_sha256: String,
}

pub async fn available_printers(
    db: &Db,
    access: &TenantAccess,
    document_id: ShipmentDocumentId,
) -> AppResult<Vec<AutomationDeviceReadModel>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let rows = sqlx::query(&format!(
        r#"SELECT {} FROM automation_devices
           WHERE automation_devices.tenant_id=$1
             AND automation_devices.device_class='printer'
             AND automation_devices.control_mode='automatic'
             AND automation_devices.health IN ('healthy','degraded')
             AND automation_devices.last_heartbeat_at>=CURRENT_TIMESTAMP-INTERVAL '2 minutes'
             AND EXISTS(SELECT 1 FROM shipment_documents document
               INNER JOIN shipments shipment
                 ON shipment.tenant_id=document.tenant_id
                AND shipment.inventory_owner_id=document.inventory_owner_id
                AND shipment.facility_id=document.facility_id
                AND shipment.id=document.shipment_id
               WHERE document.tenant_id=automation_devices.tenant_id
                 AND document.facility_id=automation_devices.facility_id
                 AND document.id=$2
                 AND shipment.state IN ('awaiting manifest','manifested')
                 AND (document.document_type='packing_slip' OR shipment.state='manifested')
                 AND ($3 OR document.facility_id=ANY($4))
                 AND ($5 OR document.inventory_owner_id=ANY($6)))
             AND EXISTS(SELECT 1 FROM automation_heartbeats heartbeat
               WHERE heartbeat.tenant_id=automation_devices.tenant_id
                 AND heartbeat.device_id=automation_devices.id
                 AND heartbeat.observed_at=automation_devices.last_heartbeat_at
                 AND heartbeat.control_mode='automatic')
           ORDER BY lower(automation_devices.display_name),automation_devices.id"#,
        mapping::DEVICE_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(document_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_all(&mut *tx)
    .await?;
    if rows.is_empty() {
        require_document_printable(&mut tx, access, document_id, &scope).await?;
    }
    let printers = rows
        .iter()
        .map(mapping::device)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(printers)
}

pub async fn print_document(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    document_id: ShipmentDocumentId,
    device_id: AutomationDeviceId,
    copies: u16,
    expected_content_sha256: &str,
) -> AppResult<AutomationCommandReadModel> {
    if !(1..=MAX_PRINT_COPIES).contains(&copies) {
        return Err(AppError::bad_request(
            "shipment document copies must be between 1 and 100",
        ));
    }
    if !is_sha256(expected_content_sha256) {
        return Err(AppError::bad_request(
            "expected shipment document hash must be lowercase SHA-256 hex",
        ));
    }
    let document = printable_document(db, access, document_id).await?;
    if document.content_sha256 != expected_content_sha256 {
        return Err(AppError::conflict(
            "shipment document changed before print dispatch",
        ));
    }
    let idempotency_key = context
        .idempotency_key
        .as_deref()
        .ok_or_else(|| AppError::bad_request("shipment document print requires idempotency"))?;
    let command = EnqueueAutomationCommand {
        device_id,
        correlation_id: format!(
            "shipping-document:{}:{}",
            document_id.get(),
            idempotency_key
        ),
        recovery_policy: AutomationRecoveryPolicy::DeviceDeduplicatedReplay,
        command: AutomationDeviceCommand::Printer(AutomationPrinterCommand::PrintDocument {
            document_id: document_id.get().to_string(),
            format: AutomationPrintFormat::Html,
            content: document.content,
            copies,
        }),
        packing_scale_context: None,
        shipping_document_print_context: Some(ShippingDocumentPrintContext {
            inventory_owner_id: document.inventory_owner_id,
            shipment_id: document.shipment_id,
            document_id,
            content_sha256: document.content_sha256,
        }),
    };
    automation::enqueue_command(db, access, context, &command).await
}

pub async fn print_jobs(
    db: &Db,
    access: &TenantAccess,
    document_id: ShipmentDocumentId,
    before_command_id: Option<AutomationCommandId>,
    limit: u16,
) -> AppResult<ShipmentDocumentPrintPage> {
    if limit == 0 || limit > MAX_PRINT_HISTORY_PAGE {
        return Err(AppError::bad_request(
            "shipment print history page size is outside the supported range",
        ));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let rows = sqlx::query(&format!(
        r#"SELECT {} FROM automation_commands command
           INNER JOIN automation_devices device
             ON device.tenant_id=command.tenant_id AND device.id=command.device_id
           INNER JOIN shipment_documents document
             ON document.tenant_id=command.tenant_id
            AND document.inventory_owner_id=command.shipping_inventory_owner_id
            AND document.facility_id=command.facility_id
            AND document.shipment_id=command.shipping_shipment_id
            AND document.id=command.shipping_document_id
            AND document.content_sha256=command.shipping_document_content_sha256
           WHERE command.tenant_id=$1 AND document.id=$2
             AND ($3::bigint IS NULL OR command.id<$3)
             AND ($4 OR document.facility_id=ANY($5))
             AND ($6 OR document.inventory_owner_id=ANY($7))
           ORDER BY command.id DESC LIMIT $8"#,
        mapping::COMMAND_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(document_id.get())
    .bind(before_command_id.map(|id| id.get()))
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(i64::from(limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = rows
        .iter()
        .map(mapping::command)
        .collect::<AppResult<Vec<_>>>()?;
    let has_more = items.len() > usize::from(limit);
    if has_more {
        items.pop();
    }
    let next_command_id = has_more
        .then(|| items.last().map(|item| item.command_id))
        .flatten();
    if items.is_empty() {
        require_document_visible(&mut tx, access, document_id, &scope).await?;
    }
    tx.commit().await?;
    Ok(ShipmentDocumentPrintPage {
        items,
        next_command_id,
    })
}

pub async fn print_job(
    db: &Db,
    access: &TenantAccess,
    document_id: ShipmentDocumentId,
    command_id: AutomationCommandId,
) -> AppResult<AutomationCommandReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let row = sqlx::query(&format!(
        r#"SELECT {} FROM automation_commands command
           INNER JOIN automation_devices device
             ON device.tenant_id=command.tenant_id AND device.id=command.device_id
           INNER JOIN shipment_documents document
             ON document.tenant_id=command.tenant_id
            AND document.inventory_owner_id=command.shipping_inventory_owner_id
            AND document.facility_id=command.facility_id
            AND document.shipment_id=command.shipping_shipment_id
            AND document.id=command.shipping_document_id
            AND document.content_sha256=command.shipping_document_content_sha256
           WHERE command.tenant_id=$1 AND document.id=$2 AND command.id=$3
             AND ($4 OR document.facility_id=ANY($5))
             AND ($6 OR document.inventory_owner_id=ANY($7))"#,
        mapping::COMMAND_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(document_id.get())
    .bind(command_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("shipment document print job"))?;
    let result = mapping::command(&row)?;
    tx.commit().await?;
    Ok(result)
}

pub async fn cancel_print_job(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CancelShipmentDocumentPrintCommand,
) -> AppResult<AutomationCommandReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    if command.expected_revision == 0 {
        return Err(AppError::bad_request("expected revision must be positive"));
    }
    let prepared =
        PreparedCommand::new_v1(context, CANCEL_SHIPMENT_DOCUMENT_PRINT_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    automation::bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared
        .replayed::<AutomationCommandReadModel>(&mut tx)
        .await?
    {
        require_print_context(&result, command.document_id, command.command_id)?;
        require_document_visible(&mut tx, access, command.document_id, &scope).await?;
        tx.commit().await?;
        return Ok(result);
    }
    let row = sqlx::query(&format!(
        r#"SELECT {} FROM automation_commands command
           INNER JOIN automation_devices device
             ON device.tenant_id=command.tenant_id AND device.id=command.device_id
           INNER JOIN shipment_documents document
             ON document.tenant_id=command.tenant_id
            AND document.inventory_owner_id=command.shipping_inventory_owner_id
            AND document.facility_id=command.facility_id
            AND document.shipment_id=command.shipping_shipment_id
            AND document.id=command.shipping_document_id
            AND document.content_sha256=command.shipping_document_content_sha256
           WHERE command.tenant_id=$1 AND command.id=$2 AND document.id=$3
             AND ($4 OR document.facility_id=ANY($5))
             AND ($6 OR document.inventory_owner_id=ANY($7))
           FOR UPDATE OF command"#,
        mapping::COMMAND_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(command.command_id.get())
    .bind(command.document_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("shipment document print job"))?;
    let current = mapping::command(&row)?;
    require_print_context(&current, command.document_id, command.command_id)?;
    if current.status != AutomationCommandStatus::Queued
        || current.revision != command.expected_revision
    {
        return Err(AppError::conflict(
            "shipment document print is no longer queued at the expected revision",
        ));
    }
    let now = now_iso();
    sqlx::query(
        r#"UPDATE automation_commands SET status='cancelled',revision=revision+1,completed_at=$3
           WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.command_id.get())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    automation::insert_command_history_tx(
        &mut tx,
        automation::CommandHistoryEvent {
            tenant_id: access.tenant_id,
            command_id: command.command_id,
            transition: "cancelled",
            actor_user_id: context.actor_id.get(),
            service_account_id: None,
            occurred_at: now,
            evidence: serde_json::json!({
                "document_id": command.document_id,
                "reason": "shipment_document_print_cancelled_before_delivery",
            }),
        },
    )
    .await?;
    let updated = sqlx::query(&format!(
        r#"SELECT {} FROM automation_commands command
           INNER JOIN automation_devices device
             ON device.tenant_id=command.tenant_id AND device.id=command.device_id
           WHERE command.tenant_id=$1 AND command.id=$2"#,
        mapping::COMMAND_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(command.command_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let result = mapping::command(&updated)?;
    let payload =
        serde_json::to_value(&result).map_err(|error| AppError::internal(error.to_string()))?;
    automation::insert_outbox_tx(
        &mut tx,
        automation::AutomationEvent {
            tenant_id: access.tenant_id,
            facility_id: result.facility_id,
            actor_user_id: context.actor_id.get(),
            aggregate_type: "automation_command",
            aggregate_id: result.command_id.get().to_string(),
            event_type: "automation.command.cancelled",
            event_key: format!(
                "automation-command:{}:{}:cancelled",
                result.command_id.get(),
                result.revision
            ),
            payload: &payload,
            occurred_at: now,
        },
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn printable_document(
    db: &Db,
    access: &TenantAccess,
    document_id: ShipmentDocumentId,
) -> AppResult<PrintableDocument> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    let row = sqlx::query(
        r#"SELECT document.inventory_owner_id,document.shipment_id,document.content,
                  encode(document.content_sha256,'hex') AS content_sha256,
                  document.document_type,shipment.state AS shipment_state
           FROM shipment_documents document
           INNER JOIN shipments shipment
             ON shipment.tenant_id=document.tenant_id
            AND shipment.inventory_owner_id=document.inventory_owner_id
            AND shipment.facility_id=document.facility_id
            AND shipment.id=document.shipment_id
           WHERE document.tenant_id=$1 AND document.id=$2
             AND ($3 OR document.facility_id=ANY($4))
             AND ($5 OR document.inventory_owner_id=ANY($6))"#,
    )
    .bind(access.tenant_id.get())
    .bind(document_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("shipment document"))?;
    let document_type: String = row.try_get("document_type")?;
    let shipment_state: String = row.try_get("shipment_state")?;
    if !matches!(shipment_state.as_str(), "awaiting manifest" | "manifested")
        || (document_type == "carton_label_set" && shipment_state != "manifested")
    {
        return Err(AppError::conflict(
            "shipment document can only print before departure",
        ));
    }
    let document = PrintableDocument {
        inventory_owner_id: wareboxes_domain::InventoryOwnerId::new(
            row.try_get("inventory_owner_id")?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        shipment_id: ShipmentId::new(row.try_get("shipment_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        content: row.try_get("content")?,
        content_sha256: row.try_get("content_sha256")?,
    };
    tx.commit().await?;
    Ok(document)
}

async fn require_document_visible(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    document_id: ShipmentDocumentId,
    scope: &crate::repo::access::ScopeBindings,
) -> AppResult<()> {
    let visible: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM shipment_documents
           WHERE tenant_id=$1 AND id=$2 AND ($3 OR facility_id=ANY($4))
             AND ($5 OR inventory_owner_id=ANY($6)))"#,
    )
    .bind(access.tenant_id.get())
    .bind(document_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("shipment document"))
    }
}

async fn require_document_printable(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    document_id: ShipmentDocumentId,
    scope: &crate::repo::access::ScopeBindings,
) -> AppResult<()> {
    require_document_visible(tx, access, document_id, scope).await?;
    let printable: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM shipment_documents document
           INNER JOIN shipments shipment
             ON shipment.tenant_id=document.tenant_id
            AND shipment.inventory_owner_id=document.inventory_owner_id
            AND shipment.facility_id=document.facility_id
            AND shipment.id=document.shipment_id
           WHERE document.tenant_id=$1 AND document.id=$2
             AND shipment.state IN ('awaiting manifest','manifested')
             AND (document.document_type='packing_slip' OR shipment.state='manifested'))"#,
    )
    .bind(access.tenant_id.get())
    .bind(document_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if printable {
        Ok(())
    } else {
        Err(AppError::conflict(
            "shipment document can only print before departure",
        ))
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn require_print_context(
    command: &AutomationCommandReadModel,
    document_id: ShipmentDocumentId,
    command_id: AutomationCommandId,
) -> AppResult<()> {
    if command.command_id == command_id
        && command
            .shipping_document_print_context
            .as_ref()
            .is_some_and(|context| context.document_id == document_id)
    {
        Ok(())
    } else {
        Err(AppError::not_found("shipment document print job"))
    }
}
