use sha2::{Digest, Sha256};
use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::shipping::{
    DocumentPolicyReadModel, DocumentPolicySource, GenerateCartonLabelSetCommand,
    GenerateCartonLabelSetResult, GeneratePackingSlipCommand, GeneratePackingSlipResult,
    ShipmentDocumentContentQuery, ShipmentDocumentContentReadModel, ShipmentDocumentListQuery,
    ShipmentDocumentListReadModel, ShipmentDocumentReadModel, GENERATE_CARTON_LABEL_SET_OPERATION,
    GENERATE_PACKING_SLIP_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    ActualPickQuantity, CarrierCode, CarrierManifestId, CarrierServiceCode, CatalogItemId,
    ConfigurationScope, ManifestReference, OrderId, OrderLineId, PickQuantity, ShipmentDocumentId,
    ShipmentDocumentType, ShipmentId, ShipmentRevision, ShipmentStatus, ShortShipDemandQuantities,
    TenantId, Timestamp, TrackingNumber, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::insert_order_activity_tx;

use super::{
    document_policy::resolve_document_policy_tx, enqueue_order_event_tx, lock_order_tx,
    lock_shipment_tx, order_hint_for_shipment_tx, positive,
};

mod render;

use render::{render_carton_label_set, render_packing_slip};

const PACKING_SLIP_TYPE: ShipmentDocumentType = ShipmentDocumentType::PackingSlip;
const CARTON_LABEL_SET_TYPE: ShipmentDocumentType = ShipmentDocumentType::CartonLabelSet;
const MEDIA_TYPE: &str = "text/html; charset=utf-8";
const RENDERER_VERSION: i64 = 1;

#[derive(Debug)]
struct AddressSnapshot {
    role: String,
    name: Option<String>,
    company: Option<String>,
    line1: String,
    line2: Option<String>,
    city: String,
    state: Option<String>,
    postal_code: String,
    country: String,
    phone: Option<String>,
    email: Option<String>,
}

#[derive(Debug)]
struct DocumentLine {
    sequence: i64,
    order_line_id: OrderLineId,
    line_key: String,
    item_id: CatalogItemId,
    item_description: String,
    uom: String,
    ordered_quantity: i64,
    accepted_short_quantity: i64,
    accepted_substitute_quantity: i64,
    packed_quantity: i64,
}

#[derive(Debug)]
struct DocumentCarton {
    shipment_carton_id: i64,
    carton_id: i64,
    license_plate_id: i64,
    sequence: i64,
    barcode: String,
    packed_quantity: i64,
    weight_grams: Option<i64>,
    length_mm: Option<i64>,
    width_mm: Option<i64>,
    height_mm: Option<i64>,
    tracking_assignment_id: Option<i64>,
    tracking_number: Option<TrackingNumber>,
}

#[derive(Debug)]
struct DocumentManifest {
    manifest_id: CarrierManifestId,
    carrier_code: CarrierCode,
    service_code: Option<CarrierServiceCode>,
    manifest_reference: ManifestReference,
}

struct NewDocument<'a> {
    document_type: ShipmentDocumentType,
    policy: &'a DocumentPolicyReadModel,
    manifest: Option<&'a DocumentManifest>,
    file_name: &'a str,
    content: &'a str,
    lines: &'a [DocumentLine],
    cartons: &'a [DocumentCarton],
}

pub async fn generate_packing_slip(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &GeneratePackingSlipCommand,
) -> AppResult<GeneratePackingSlipResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, GENERATE_PACKING_SLIP_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared
        .replayed::<GeneratePackingSlipResult>(&mut tx)
        .await?
    {
        require_document_visible_tx(
            &mut tx,
            access.tenant_id,
            result.document.document_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order_id =
        order_hint_for_shipment_tx(&mut tx, access.tenant_id, command.shipment_id).await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
    let shipment = lock_shipment_tx(&mut tx, access.tenant_id, command.shipment_id, &scope).await?;
    if shipment.order_id != order.id || shipment.inventory_owner_id != order.inventory_owner_id {
        return Err(AppError::not_found("shipment"));
    }
    if shipment.revision != command.expected_revision {
        return Err(AppError::conflict(
            "shipment changed before packing-slip generation",
        ));
    }
    if shipment.status == ShipmentStatus::Cancelled {
        return Err(AppError::conflict(
            "packing slips cannot be generated for a cancelled shipment attempt",
        ));
    }
    let generated_at = now_iso();
    let policy = resolve_document_policy_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.facility_id,
        generated_at,
        true,
    )
    .await?;
    require_expected_policy(&policy, &command.expected_policy)?;
    require_permitted_document(&policy, PACKING_SLIP_TYPE)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "shipment-document:{}:{}:{}",
            access.tenant_id,
            command.shipment_id,
            PACKING_SLIP_TYPE.as_str()
        ))
        .execute(&mut *tx)
        .await?;
    let existing: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM shipment_documents WHERE tenant_id = $1 AND shipment_id = $2 AND document_type = $3)",
    )
    .bind(access.tenant_id.get())
    .bind(command.shipment_id.get())
    .bind(PACKING_SLIP_TYPE.as_str())
    .fetch_one(&mut *tx)
    .await?;
    if existing {
        return Err(AppError::conflict(
            "shipment packing slip has already been generated",
        ));
    }

    let addresses = load_addresses_tx(&mut tx, access.tenant_id, command.shipment_id).await?;
    let lines = load_document_lines_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id.get(),
        shipment.order_id,
        shipment.packing_session_id.get(),
    )
    .await?;
    let manifest = if policy.require_tracking_barcode {
        Some(load_document_manifest_tx(&mut tx, access.tenant_id, shipment.id).await?)
    } else {
        None
    };
    let cartons = if let Some(manifest) = manifest.as_ref() {
        load_document_cartons_tx(
            &mut tx,
            access.tenant_id,
            command.shipment_id,
            Some(manifest.manifest_id),
        )
        .await?
    } else {
        load_document_cartons_tx(&mut tx, access.tenant_id, command.shipment_id, None).await?
    };
    if addresses.len() != 2 || lines.is_empty() || cartons.is_empty() {
        return Err(AppError::internal(
            "shipment snapshots are incomplete for packing-slip generation",
        ));
    }
    if policy.require_tracking_barcode {
        require_tracking_barcodes(&cartons)?;
    }
    let content = render_packing_slip(
        shipment.id,
        &order.order_key,
        &addresses,
        &lines,
        &cartons,
        shipment.demand,
        policy.require_tracking_barcode,
    )?;
    let file_name = format!("packing-slip-shipment-{}.html", shipment.id.get());
    let (document_id, content_sha256_hex, line_count) = insert_document_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        &shipment,
        NewDocument {
            document_type: PACKING_SLIP_TYPE,
            policy: &policy,
            manifest: manifest.as_ref(),
            file_name: &file_name,
            content: &content,
            lines: &lines,
            cartons: &cartons,
        },
        generated_at,
    )
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "generated packing slip {document_id} for shipment {}",
            shipment.id
        ),
    )
    .await?;
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.facility_id,
        context.actor_id.get(),
        shipment.order_id,
        "shipping.packing_slip_generated",
        &format!("shipment-document:{}:generated", document_id.get()),
        serde_json::json!({
            "document_id": document_id,
            "document_type": PACKING_SLIP_TYPE,
            "shipment_id": shipment.id,
            "order_id": shipment.order_id,
            "shipment_revision": shipment.revision,
            "carton_count": shipment.carton_count,
            "line_count": line_count,
            "ordered_quantity": shipment.demand.ordered(),
            "packed_quantity": shipment.demand.effective(),
            "accepted_short_quantity": shipment.demand.accepted_short(),
            "accepted_substitute_quantity": shipment.demand.accepted_substitute(),
            "content_sha256": content_sha256_hex,
            "policy": policy,
            "generated_at": generated_at,
        }),
        generated_at,
    )
    .await?;
    let document_row =
        load_visible_document_row_tx(&mut tx, access.tenant_id, document_id, &scope).await?;
    let document = map_document_row(&document_row)?;
    Ok(prepared
        .commit(tx, GeneratePackingSlipResult { document })
        .await?)
}

pub async fn generate_carton_label_set(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &GenerateCartonLabelSetCommand,
) -> AppResult<GenerateCartonLabelSetResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, GENERATE_CARTON_LABEL_SET_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    if let Some(result) = prepared
        .replayed::<GenerateCartonLabelSetResult>(&mut tx)
        .await?
    {
        require_document_visible_tx(
            &mut tx,
            access.tenant_id,
            result.document.document_id,
            &scope,
        )
        .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let order_id =
        order_hint_for_shipment_tx(&mut tx, access.tenant_id, command.shipment_id).await?;
    let order = lock_order_tx(&mut tx, access.tenant_id, order_id, &scope).await?;
    let shipment = lock_shipment_tx(&mut tx, access.tenant_id, command.shipment_id, &scope).await?;
    if shipment.order_id != order.id || shipment.inventory_owner_id != order.inventory_owner_id {
        return Err(AppError::not_found("shipment"));
    }
    if shipment.revision != command.expected_revision {
        return Err(AppError::conflict(
            "shipment changed before carton-label generation",
        ));
    }
    if !matches!(shipment.status, ShipmentStatus::Manifested) {
        return Err(AppError::conflict(
            "carton labels can only be generated before a manifested shipment departs",
        ));
    }
    let generated_at = now_iso();
    let policy = resolve_document_policy_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.facility_id,
        generated_at,
        true,
    )
    .await?;
    require_expected_policy(&policy, &command.expected_policy)?;
    require_permitted_document(&policy, CARTON_LABEL_SET_TYPE)?;
    lock_document_key_tx(
        &mut tx,
        access.tenant_id,
        shipment.id,
        CARTON_LABEL_SET_TYPE,
    )
    .await?;
    require_document_type_available_tx(
        &mut tx,
        access.tenant_id,
        shipment.id,
        CARTON_LABEL_SET_TYPE,
        "shipment carton labels have already been generated",
    )
    .await?;

    let manifest = load_document_manifest_tx(&mut tx, access.tenant_id, shipment.id).await?;
    let addresses = load_addresses_tx(&mut tx, access.tenant_id, shipment.id).await?;
    let lines = load_document_lines_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id.get(),
        shipment.order_id,
        shipment.packing_session_id.get(),
    )
    .await?;
    let cartons = load_document_cartons_tx(
        &mut tx,
        access.tenant_id,
        shipment.id,
        Some(manifest.manifest_id),
    )
    .await?;
    if addresses.len() != 2
        || lines.is_empty()
        || cartons.is_empty()
        || cartons
            .iter()
            .any(|carton| carton.tracking_number.is_none())
    {
        return Err(AppError::internal(
            "shipment snapshots are incomplete for carton-label generation",
        ));
    }
    require_tracking_barcodes(&cartons)?;
    let content = render_carton_label_set(
        shipment.id,
        &order.order_key,
        &addresses,
        &cartons,
        &manifest,
    )?;
    let file_name = format!("carton-labels-shipment-{}.html", shipment.id.get());
    let (document_id, content_sha256_hex, line_count) = insert_document_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        &shipment,
        NewDocument {
            document_type: CARTON_LABEL_SET_TYPE,
            policy: &policy,
            manifest: Some(&manifest),
            file_name: &file_name,
            content: &content,
            lines: &lines,
            cartons: &cartons,
        },
        generated_at,
    )
    .await?;
    insert_order_activity_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.order_id.get(),
        Some(context.actor_id.get()),
        &format!(
            "generated carton label set {document_id} for shipment {}",
            shipment.id
        ),
    )
    .await?;
    enqueue_order_event_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id,
        shipment.facility_id,
        context.actor_id.get(),
        shipment.order_id,
        "shipping.carton_label_set_generated",
        &format!("shipment-document:{}:generated", document_id.get()),
        serde_json::json!({
            "document_id": document_id,
            "document_type": CARTON_LABEL_SET_TYPE,
            "shipment_id": shipment.id,
            "order_id": shipment.order_id,
            "manifest_id": manifest.manifest_id,
            "carrier_code": manifest.carrier_code,
            "service_code": manifest.service_code,
            "shipment_revision": shipment.revision,
            "carton_count": shipment.carton_count,
            "line_count": line_count,
            "content_sha256": content_sha256_hex,
            "policy": policy,
            "generated_at": generated_at,
        }),
        generated_at,
    )
    .await?;
    let document_row =
        load_visible_document_row_tx(&mut tx, access.tenant_id, document_id, &scope).await?;
    let document = map_document_row(&document_row)?;
    Ok(prepared
        .commit(tx, GenerateCartonLabelSetResult { document })
        .await?)
}

pub async fn list_documents(
    db: &Db,
    access: &TenantAccess,
    query: ShipmentDocumentListQuery,
) -> AppResult<ShipmentDocumentListReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let shipment_scope = sqlx::query(
        r#"SELECT inventory_owner_id,facility_id FROM shipments
            WHERE tenant_id = $1 AND id = $2
              AND ($3 OR facility_id = ANY($4))
              AND ($5 OR inventory_owner_id = ANY($6))"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.shipment_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let shipment_scope = shipment_scope.ok_or_else(|| AppError::not_found("shipment"))?;
    let inventory_owner_id = positive(
        shipment_scope.try_get("inventory_owner_id")?,
        wareboxes_domain::InventoryOwnerId::new,
    )?;
    let facility_id = positive(
        shipment_scope.try_get("facility_id")?,
        wareboxes_domain::FacilityId::new,
    )?;
    let policy = resolve_document_policy_tx(
        &mut tx,
        access.tenant_id,
        inventory_owner_id,
        facility_id,
        now_iso(),
        false,
    )
    .await?;
    let rows = sqlx::query(
        "SELECT * FROM shipment_documents WHERE tenant_id = $1 AND shipment_id = $2 ORDER BY generated_at, id",
    )
    .bind(access.tenant_id.get())
    .bind(query.shipment_id.get())
    .fetch_all(&mut *tx)
    .await?;
    let documents = rows
        .iter()
        .map(map_document_row)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(ShipmentDocumentListReadModel { policy, documents })
}

pub async fn get_document_content(
    db: &Db,
    access: &TenantAccess,
    query: ShipmentDocumentContentQuery,
) -> AppResult<ShipmentDocumentContentReadModel> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let row =
        load_visible_document_row_tx(&mut tx, access.tenant_id, query.document_id, &scope).await?;
    let document = map_document_row(&row)?;
    let content = row.try_get("content")?;
    tx.commit().await?;
    Ok(ShipmentDocumentContentReadModel { document, content })
}

async fn require_document_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    document_id: ShipmentDocumentId,
    scope: &ScopeBindings,
) -> AppResult<()> {
    load_visible_document_row_tx(tx, tenant_id, document_id, scope)
        .await
        .map(|_| ())
}

async fn load_visible_document_row_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    document_id: ShipmentDocumentId,
    scope: &ScopeBindings,
) -> AppResult<sqlx::postgres::PgRow> {
    sqlx::query(
        r#"
        SELECT document.*
        FROM shipment_documents document
        INNER JOIN shipments shipment
          ON shipment.tenant_id = document.tenant_id
         AND shipment.inventory_owner_id = document.inventory_owner_id
         AND shipment.facility_id = document.facility_id
         AND shipment.id = document.shipment_id
        WHERE document.tenant_id = $1 AND document.id = $2
          AND ($3 OR shipment.facility_id = ANY($4))
          AND ($5 OR shipment.inventory_owner_id = ANY($6))
        "#,
    )
    .bind(tenant_id.get())
    .bind(document_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("shipment document"))
}

fn map_document_row(row: &sqlx::postgres::PgRow) -> AppResult<ShipmentDocumentReadModel> {
    let document_type_text: String = row.try_get("document_type")?;
    let policy = map_policy_row(row)?;
    Ok(ShipmentDocumentReadModel {
        document_id: positive(row.try_get("id")?, ShipmentDocumentId::new)?,
        shipment_id: positive(row.try_get("shipment_id")?, ShipmentId::new)?,
        order_id: positive(row.try_get("order_id")?, OrderId::new)?,
        document_type: ShipmentDocumentType::parse(&document_type_text)
            .ok_or_else(|| AppError::internal("shipment document has an invalid type"))?,
        manifest_id: row
            .try_get::<Option<i64>, _>("carrier_manifest_id")?
            .map(|value| positive(value, CarrierManifestId::new))
            .transpose()?,
        carrier_code: row
            .try_get::<Option<String>, _>("carrier_code")?
            .map(|value| {
                CarrierCode::new(value).map_err(|error| AppError::internal(error.to_string()))
            })
            .transpose()?,
        service_code: row
            .try_get::<Option<String>, _>("service_code")?
            .map(|value| {
                CarrierServiceCode::new(value)
                    .map_err(|error| AppError::internal(error.to_string()))
            })
            .transpose()?,
        manifest_reference: row
            .try_get::<Option<String>, _>("manifest_reference")?
            .map(|value| {
                ManifestReference::new(value).map_err(|error| AppError::internal(error.to_string()))
            })
            .transpose()?,
        file_name: row.try_get("file_name")?,
        media_type: row.try_get("media_type")?,
        content_length: row.try_get("content_length")?,
        content_sha256: hex::encode(row.try_get::<Vec<u8>, _>("content_sha256")?),
        shipment_revision_at_generation: positive(
            row.try_get("shipment_revision_at_generation")?,
            ShipmentRevision::new,
        )?,
        carton_count: row.try_get("carton_count")?,
        line_count: row.try_get("line_count")?,
        demand: ShortShipDemandQuantities::with_substitution(
            PickQuantity::new(row.try_get("ordered_qty")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            ActualPickQuantity::new(row.try_get("accepted_short_qty")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            ActualPickQuantity::new(row.try_get("accepted_substitute_qty")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        policy,
        generated_by: positive(row.try_get("generated_by_user_id")?, UserId::new)?,
        generated_at: row.try_get::<Timestamp, _>("generated_at")?,
    })
}

fn map_policy_row(row: &sqlx::postgres::PgRow) -> AppResult<DocumentPolicyReadModel> {
    let source = match row.try_get::<String, _>("policy_source")?.as_str() {
        "product_default" => DocumentPolicySource::ProductDefault,
        "configuration" => DocumentPolicySource::Configuration,
        _ => {
            return Err(AppError::internal(
                "shipment document policy source is invalid",
            ))
        }
    };
    let configuration_id = row
        .try_get::<Option<i64>, _>("policy_configuration_id")?
        .map(|value| positive(value, wareboxes_domain::ConfigurationVersionId::new))
        .transpose()?;
    let configuration_revision = row.try_get("policy_configuration_revision")?;
    let configuration_scope = match row.try_get::<Option<String>, _>("policy_scope_level")? {
        None => None,
        Some(level) => Some(match level.as_str() {
            "tenant" => ConfigurationScope::Tenant,
            "inventory_owner" => ConfigurationScope::InventoryOwner {
                inventory_owner_id: positive(
                    required_policy_scope_id(row, "policy_inventory_owner_id")?,
                    wareboxes_domain::InventoryOwnerId::new,
                )?,
            },
            "facility" => ConfigurationScope::Facility {
                facility_id: positive(
                    required_policy_scope_id(row, "policy_facility_id")?,
                    wareboxes_domain::FacilityId::new,
                )?,
            },
            "owner_facility" => ConfigurationScope::OwnerFacility {
                inventory_owner_id: positive(
                    required_policy_scope_id(row, "policy_inventory_owner_id")?,
                    wareboxes_domain::InventoryOwnerId::new,
                )?,
                facility_id: positive(
                    required_policy_scope_id(row, "policy_facility_id")?,
                    wareboxes_domain::FacilityId::new,
                )?,
            },
            _ => {
                return Err(AppError::internal(
                    "shipment document policy scope is invalid",
                ))
            }
        }),
    };
    let definition = row.try_get::<serde_json::Value, _>("policy_definition")?;
    let generate_packing_slip = definition
        .get("generate_packing_slip")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| AppError::internal("shipment document packing-slip policy is invalid"))?;
    let generate_carton_label = definition
        .get("generate_carton_label")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| AppError::internal("shipment document carton-label policy is invalid"))?;
    let require_tracking_barcode = definition
        .get("require_tracking_barcode")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| AppError::internal("shipment document tracking policy is invalid"))?;
    Ok(DocumentPolicyReadModel {
        source,
        configuration_id,
        configuration_revision,
        configuration_scope,
        generate_packing_slip,
        generate_carton_label,
        require_tracking_barcode,
        policy_hash: row.try_get("policy_hash")?,
    })
}

fn required_policy_scope_id(row: &sqlx::postgres::PgRow, name: &str) -> AppResult<i64> {
    row.try_get::<Option<i64>, _>(name)?
        .ok_or_else(|| AppError::internal(format!("shipment document {name} is missing")))
}

fn policy_scope_values(
    scope: Option<ConfigurationScope>,
) -> (Option<&'static str>, Option<i64>, Option<i64>) {
    match scope {
        None => (None, None, None),
        Some(ConfigurationScope::Tenant) => (Some("tenant"), None, None),
        Some(ConfigurationScope::InventoryOwner { inventory_owner_id }) => (
            Some("inventory_owner"),
            Some(inventory_owner_id.get()),
            None,
        ),
        Some(ConfigurationScope::Facility { facility_id }) => {
            (Some("facility"), None, Some(facility_id.get()))
        }
        Some(ConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        }) => (
            Some("owner_facility"),
            Some(inventory_owner_id.get()),
            Some(facility_id.get()),
        ),
    }
}

fn require_expected_policy(
    actual: &DocumentPolicyReadModel,
    expected: &wareboxes_application::shipping::DocumentPolicyExpectation,
) -> AppResult<()> {
    if actual.matches_expectation(expected) {
        Ok(())
    } else {
        Err(AppError::conflict(
            "document policy changed before generation",
        ))
    }
}

fn require_permitted_document(
    policy: &DocumentPolicyReadModel,
    document_type: ShipmentDocumentType,
) -> AppResult<()> {
    if policy.permits(document_type) {
        Ok(())
    } else {
        Err(AppError::conflict(format!(
            "the effective document policy disables {} generation",
            document_type.as_str().replace('_', " ")
        )))
    }
}

fn require_tracking_barcodes(cartons: &[DocumentCarton]) -> AppResult<()> {
    for carton in cartons {
        let tracking = carton.tracking_number.as_ref().ok_or_else(|| {
            AppError::conflict("the effective document policy requires carton tracking barcodes")
        })?;
        wareboxes_barcodes::svg("code128", tracking.as_str()).map_err(|_| {
            AppError::conflict("a carton tracking number cannot be encoded as a Code 128 barcode")
        })?;
    }
    Ok(())
}

async fn load_addresses_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
) -> AppResult<Vec<AddressSnapshot>> {
    let rows = sqlx::query(
        r#"SELECT address_role, name, company, line1, line2, city, state,
                  postal_code, country, phone, email
           FROM shipment_address_snapshots
           WHERE tenant_id = $1 AND shipment_id = $2
           ORDER BY CASE address_role WHEN 'origin' THEN 1 ELSE 2 END"#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(AddressSnapshot {
                role: row.try_get("address_role")?,
                name: row.try_get("name")?,
                company: row.try_get("company")?,
                line1: row.try_get("line1")?,
                line2: row.try_get("line2")?,
                city: row.try_get("city")?,
                state: row.try_get("state")?,
                postal_code: row.try_get("postal_code")?,
                country: row.try_get("country")?,
                phone: row.try_get("phone")?,
                email: row.try_get("email")?,
            })
        })
        .collect()
}

async fn load_document_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: i64,
    order_id: OrderId,
    packing_session_id: i64,
) -> AppResult<Vec<DocumentLine>> {
    let rows = sqlx::query(
        r#"
        SELECT item.id AS order_item_id, item.line_number, item.line_key,
               item.item_id, item.uom,
               COALESCE(NULLIF(btrim(catalog.description), ''), 'Item ' || item.item_id::text)
                   AS item_description,
               demand.original_qty, demand.accepted_short_qty,
               demand.accepted_substitute_qty, demand.effective_qty,
               COALESCE(SUM(content.packed_qty) FILTER
                   (WHERE position.current_carton_content_id IS NOT NULL), 0)::bigint AS packed_qty
        FROM outbound_effective_demand demand
        INNER JOIN order_items item
          ON item.tenant_id = demand.tenant_id
         AND item.inventory_owner_id = demand.inventory_owner_id
         AND item.order_id = demand.order_id AND item.id = demand.order_item_id
        INNER JOIN items catalog
          ON catalog.tenant_id = item.tenant_id AND catalog.id = item.item_id
        LEFT JOIN carton_contents content
          ON content.tenant_id = demand.tenant_id
         AND content.inventory_owner_id = demand.inventory_owner_id
         AND content.order_id = demand.order_id AND content.order_item_id = demand.order_item_id
         AND content.packing_session_id = $4
        LEFT JOIN packing_allocation_positions position
          ON position.tenant_id=content.tenant_id
         AND position.inventory_owner_id=content.inventory_owner_id
         AND position.facility_id=content.facility_id
         AND position.packing_session_id=content.packing_session_id
         AND position.packing_session_allocation_id=content.packing_session_allocation_id
         AND position.current_carton_content_id=content.id
         AND position.state='packed'
        WHERE demand.tenant_id = $1 AND demand.inventory_owner_id = $2 AND demand.order_id = $3
        GROUP BY item.id, item.line_number, item.line_key, item.item_id, item.uom,
                 catalog.description, demand.original_qty, demand.accepted_short_qty,
                 demand.accepted_substitute_qty,
                 demand.effective_qty
        ORDER BY item.line_number, item.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id)
    .bind(order_id.get())
    .bind(packing_session_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let effective: i64 = row.try_get("effective_qty")?;
            let packed: i64 = row.try_get("packed_qty")?;
            if effective != packed || effective < 0 {
                return Err(AppError::internal(
                    "shipment line packing quantity does not match effective demand",
                ));
            }
            Ok(DocumentLine {
                sequence: row.try_get("line_number")?,
                order_line_id: positive(row.try_get("order_item_id")?, OrderLineId::new)?,
                line_key: row.try_get("line_key")?,
                item_id: positive(row.try_get("item_id")?, CatalogItemId::new)?,
                item_description: row.try_get("item_description")?,
                uom: row.try_get("uom")?,
                ordered_quantity: row.try_get("original_qty")?,
                accepted_short_quantity: row.try_get("accepted_short_qty")?,
                accepted_substitute_quantity: row.try_get("accepted_substitute_qty")?,
                packed_quantity: packed,
            })
        })
        .collect()
}

async fn load_document_cartons_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
    manifest_id: Option<CarrierManifestId>,
) -> AppResult<Vec<DocumentCarton>> {
    let rows = sqlx::query(
        r#"
        SELECT carton.id AS shipment_carton_id, carton.carton_id,
               carton.license_plate_id, carton.sequence, carton.carton_barcode,
               carton.packed_qty, carton.weight_g, carton.length_mm,
               carton.width_mm, carton.height_mm,
               package.id AS tracking_assignment_id, package.tracking_number
        FROM shipment_cartons carton
        LEFT JOIN shipment_manifest_packages package
          ON package.tenant_id = carton.tenant_id
         AND package.inventory_owner_id = carton.inventory_owner_id
         AND package.facility_id = carton.facility_id
         AND package.shipment_id = carton.shipment_id
         AND package.shipment_carton_id = carton.id
         AND package.manifest_id = $3
        WHERE carton.tenant_id = $1 AND carton.shipment_id = $2
        ORDER BY carton.sequence, carton.id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .bind(manifest_id.map(|value| value.get()))
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(DocumentCarton {
                shipment_carton_id: row.try_get("shipment_carton_id")?,
                carton_id: row.try_get("carton_id")?,
                license_plate_id: row.try_get("license_plate_id")?,
                sequence: row.try_get("sequence")?,
                barcode: row.try_get("carton_barcode")?,
                packed_quantity: row.try_get("packed_qty")?,
                weight_grams: row.try_get("weight_g")?,
                length_mm: row.try_get("length_mm")?,
                width_mm: row.try_get("width_mm")?,
                height_mm: row.try_get("height_mm")?,
                tracking_assignment_id: row.try_get("tracking_assignment_id")?,
                tracking_number: row
                    .try_get::<Option<String>, _>("tracking_number")?
                    .map(|value| {
                        TrackingNumber::new(value)
                            .map_err(|error| AppError::internal(error.to_string()))
                    })
                    .transpose()?,
            })
        })
        .collect()
}

async fn load_document_manifest_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
) -> AppResult<DocumentManifest> {
    let row = sqlx::query(
        r#"SELECT id, carrier, service, manifest_number
           FROM shipment_manifests
           WHERE tenant_id = $1 AND shipment_id = $2"#,
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::conflict("shipment has no carrier manifest"))?;
    Ok(DocumentManifest {
        manifest_id: positive(row.try_get("id")?, CarrierManifestId::new)?,
        carrier_code: CarrierCode::new(row.try_get::<String, _>("carrier")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        service_code: row
            .try_get::<Option<String>, _>("service")?
            .map(|value| {
                CarrierServiceCode::new(value)
                    .map_err(|error| AppError::internal(error.to_string()))
            })
            .transpose()?,
        manifest_reference: ManifestReference::new(row.try_get::<String, _>("manifest_number")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

async fn lock_document_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
    document_type: ShipmentDocumentType,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "shipment-document:{tenant_id}:{shipment_id}:{document_type}"
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn require_document_type_available_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
    document_type: ShipmentDocumentType,
    conflict_message: &'static str,
) -> AppResult<()> {
    let existing: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM shipment_documents WHERE tenant_id = $1 AND shipment_id = $2 AND document_type = $3)",
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .bind(document_type.as_str())
    .fetch_one(&mut **tx)
    .await?;
    if existing {
        return Err(AppError::conflict(conflict_message));
    }
    Ok(())
}

async fn insert_document_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: i64,
    shipment: &super::LockedShipment,
    document: NewDocument<'_>,
    generated_at: Timestamp,
) -> AppResult<(ShipmentDocumentId, String, i64)> {
    let content_length = i64::try_from(document.content.len())
        .map_err(|_| AppError::internal("shipment document content is too large"))?;
    let content_sha256 = Sha256::digest(document.content.as_bytes()).to_vec();
    let content_sha256_hex = hex::encode(&content_sha256);
    let line_count = i64::try_from(document.lines.len())
        .map_err(|_| AppError::internal("shipment document has too many lines"))?;
    let (policy_scope_level, policy_owner_id, policy_facility_id) =
        policy_scope_values(document.policy.configuration_scope);
    let policy_definition = serde_json::json!({
        "kind": "document",
        "generate_packing_slip": document.policy.generate_packing_slip,
        "generate_carton_label": document.policy.generate_carton_label,
        "require_tracking_barcode": document.policy.require_tracking_barcode,
    });
    let document_id_raw: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO shipment_documents (
            tenant_id, inventory_owner_id, facility_id, shipment_id, order_id,
            document_type, carrier_manifest_id, carrier_code, service_code,
            manifest_reference, file_name, media_type, renderer_version,
            shipment_revision_at_generation, carton_count, line_count,
            ordered_qty, accepted_short_qty, accepted_substitute_qty, packed_qty,
            policy_source, policy_configuration_id, policy_configuration_revision,
            policy_scope_level, policy_inventory_owner_id, policy_facility_id,
            policy_definition, policy_hash,
            content, content_length, content_sha256, generated_by_user_id, generated_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
            $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24,
            $25, $26, $27, $28, $29, $30, $31, $32, $33
        ) RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(shipment.inventory_owner_id.get())
    .bind(shipment.facility_id.get())
    .bind(shipment.id.get())
    .bind(shipment.order_id.get())
    .bind(document.document_type.as_str())
    .bind(document.manifest.map(|value| value.manifest_id.get()))
    .bind(document.manifest.map(|value| value.carrier_code.as_str()))
    .bind(
        document
            .manifest
            .and_then(|value| value.service_code.as_ref().map(|code| code.as_str())),
    )
    .bind(
        document
            .manifest
            .map(|value| value.manifest_reference.as_str()),
    )
    .bind(document.file_name)
    .bind(MEDIA_TYPE)
    .bind(RENDERER_VERSION)
    .bind(shipment.revision.get())
    .bind(shipment.carton_count)
    .bind(line_count)
    .bind(shipment.demand.ordered().get())
    .bind(shipment.demand.accepted_short().get())
    .bind(shipment.demand.accepted_substitute().get())
    .bind(shipment.demand.effective().get())
    .bind(match document.policy.source {
        DocumentPolicySource::ProductDefault => "product_default",
        DocumentPolicySource::Configuration => "configuration",
    })
    .bind(document.policy.configuration_id.map(|value| value.get()))
    .bind(document.policy.configuration_revision)
    .bind(policy_scope_level)
    .bind(policy_owner_id)
    .bind(policy_facility_id)
    .bind(policy_definition)
    .bind(&document.policy.policy_hash)
    .bind(document.content)
    .bind(content_length)
    .bind(&content_sha256)
    .bind(actor_id)
    .bind(generated_at)
    .fetch_one(&mut **tx)
    .await?;
    let document_id = positive(document_id_raw, ShipmentDocumentId::new)?;
    insert_document_lines_tx(
        tx,
        tenant_id,
        shipment.inventory_owner_id.get(),
        shipment.facility_id.get(),
        document_id,
        shipment.id,
        shipment.order_id,
        document.lines,
    )
    .await?;
    insert_document_cartons_tx(
        tx,
        tenant_id,
        shipment.inventory_owner_id.get(),
        shipment.facility_id.get(),
        document_id,
        shipment.id,
        shipment.order_id,
        document.cartons,
    )
    .await?;
    Ok((document_id, content_sha256_hex, line_count))
}

#[allow(clippy::too_many_arguments)]
async fn insert_document_lines_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
    document_id: ShipmentDocumentId,
    shipment_id: ShipmentId,
    order_id: OrderId,
    lines: &[DocumentLine],
) -> AppResult<()> {
    for line in lines {
        sqlx::query(
            r#"INSERT INTO shipment_document_lines (
                   tenant_id, inventory_owner_id, facility_id, shipment_document_id,
                   shipment_id, order_id, order_item_id, sequence, line_key, item_id,
                   item_description, uom, ordered_qty, accepted_short_qty,
                   accepted_substitute_qty, packed_qty
               ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)"#,
        )
        .bind(tenant_id.get())
        .bind(owner_id)
        .bind(facility_id)
        .bind(document_id.get())
        .bind(shipment_id.get())
        .bind(order_id.get())
        .bind(line.order_line_id.get())
        .bind(line.sequence)
        .bind(&line.line_key)
        .bind(line.item_id.get())
        .bind(&line.item_description)
        .bind(&line.uom)
        .bind(line.ordered_quantity)
        .bind(line.accepted_short_quantity)
        .bind(line.accepted_substitute_quantity)
        .bind(line.packed_quantity)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_document_cartons_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: i64,
    facility_id: i64,
    document_id: ShipmentDocumentId,
    shipment_id: ShipmentId,
    order_id: OrderId,
    cartons: &[DocumentCarton],
) -> AppResult<()> {
    for carton in cartons {
        sqlx::query(
            r#"INSERT INTO shipment_document_cartons (
                   tenant_id, inventory_owner_id, facility_id, shipment_document_id,
                   shipment_id, order_id, shipment_carton_id, carton_id,
                   license_plate_id, sequence, carton_barcode, packed_qty, weight_g,
                   length_mm, width_mm, height_mm, tracking_assignment_id, tracking_number
               ) VALUES (
                   $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                   $13, $14, $15, $16, $17, $18)"#,
        )
        .bind(tenant_id.get())
        .bind(owner_id)
        .bind(facility_id)
        .bind(document_id.get())
        .bind(shipment_id.get())
        .bind(order_id.get())
        .bind(carton.shipment_carton_id)
        .bind(carton.carton_id)
        .bind(carton.license_plate_id)
        .bind(carton.sequence)
        .bind(&carton.barcode)
        .bind(carton.packed_quantity)
        .bind(carton.weight_grams)
        .bind(carton.length_mm)
        .bind(carton.width_mm)
        .bind(carton.height_mm)
        .bind(carton.tracking_assignment_id)
        .bind(carton.tracking_number.as_ref().map(TrackingNumber::as_str))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}
