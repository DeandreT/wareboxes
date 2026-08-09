use sha2::{Digest, Sha256};
use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::shipping::{
    GeneratePackingSlipCommand, GeneratePackingSlipResult, ShipmentDocumentContentQuery,
    ShipmentDocumentContentReadModel, ShipmentDocumentListQuery, ShipmentDocumentReadModel,
    GENERATE_PACKING_SLIP_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    ActualPickQuantity, CatalogItemId, OrderId, OrderLineId, PickQuantity, ShipmentDocumentId,
    ShipmentDocumentType, ShipmentId, ShipmentRevision, ShortShipDemandQuantities, TenantId,
    Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::insert_order_activity_tx;

use super::{
    enqueue_order_event_tx, lock_order_tx, lock_shipment_tx, order_hint_for_shipment_tx, positive,
};

const DOCUMENT_TYPE: ShipmentDocumentType = ShipmentDocumentType::PackingSlip;
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
    packed_quantity: i64,
}

#[derive(Debug)]
struct DocumentCarton {
    sequence: i64,
    barcode: String,
    packed_quantity: i64,
    weight_grams: Option<i64>,
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
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "shipment-document:{}:{}:{}",
            access.tenant_id,
            command.shipment_id,
            DOCUMENT_TYPE.as_str()
        ))
        .execute(&mut *tx)
        .await?;
    let existing: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM shipment_documents WHERE tenant_id = $1 AND shipment_id = $2 AND document_type = $3)",
    )
    .bind(access.tenant_id.get())
    .bind(command.shipment_id.get())
    .bind(DOCUMENT_TYPE.as_str())
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
    let cartons = load_document_cartons_tx(&mut tx, access.tenant_id, command.shipment_id).await?;
    if addresses.len() != 2 || lines.is_empty() || cartons.is_empty() {
        return Err(AppError::internal(
            "shipment snapshots are incomplete for packing-slip generation",
        ));
    }
    let content = render_packing_slip(
        shipment.id,
        &order.order_key,
        &addresses,
        &lines,
        &cartons,
        shipment.demand,
    );
    let content_length = i64::try_from(content.len())
        .map_err(|_| AppError::internal("packing slip content is too large"))?;
    let content_sha256 = Sha256::digest(content.as_bytes()).to_vec();
    let content_sha256_hex = hex::encode(&content_sha256);
    let file_name = format!("packing-slip-shipment-{}.html", shipment.id.get());
    let generated_at = now_iso();
    let line_count = i64::try_from(lines.len())
        .map_err(|_| AppError::internal("packing slip has too many lines"))?;
    let document_id_raw: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO shipment_documents (
            tenant_id, inventory_owner_id, facility_id, shipment_id, order_id,
            document_type, file_name, media_type, renderer_version,
            shipment_revision_at_generation, carton_count, line_count,
            ordered_qty, accepted_short_qty, packed_qty, content, content_length,
            content_sha256, generated_by_user_id, generated_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
            $11, $12, $13, $14, $15, $16, $17, $18, $19, $20
        ) RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(shipment.inventory_owner_id.get())
    .bind(shipment.facility_id.get())
    .bind(shipment.id.get())
    .bind(shipment.order_id.get())
    .bind(DOCUMENT_TYPE.as_str())
    .bind(&file_name)
    .bind(MEDIA_TYPE)
    .bind(RENDERER_VERSION)
    .bind(shipment.revision.get())
    .bind(shipment.carton_count)
    .bind(line_count)
    .bind(shipment.demand.ordered().get())
    .bind(shipment.demand.accepted_short().get())
    .bind(shipment.demand.effective().get())
    .bind(&content)
    .bind(content_length)
    .bind(&content_sha256)
    .bind(context.actor_id.get())
    .bind(generated_at)
    .fetch_one(&mut *tx)
    .await?;
    let document_id = positive(document_id_raw, ShipmentDocumentId::new)?;
    insert_document_lines_tx(
        &mut tx,
        access.tenant_id,
        shipment.inventory_owner_id.get(),
        shipment.facility_id.get(),
        document_id,
        shipment.id,
        shipment.order_id,
        &lines,
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
            "document_type": DOCUMENT_TYPE,
            "shipment_id": shipment.id,
            "order_id": shipment.order_id,
            "shipment_revision": shipment.revision,
            "carton_count": shipment.carton_count,
            "line_count": line_count,
            "ordered_quantity": shipment.demand.ordered(),
            "packed_quantity": shipment.demand.effective(),
            "accepted_short_quantity": shipment.demand.accepted_short(),
            "content_sha256": content_sha256_hex,
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

pub async fn list_documents(
    db: &Db,
    access: &TenantAccess,
    query: ShipmentDocumentListQuery,
) -> AppResult<Vec<ShipmentDocumentReadModel>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let visible = sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS (
            SELECT 1 FROM shipments
            WHERE tenant_id = $1 AND id = $2
              AND ($3 OR facility_id = ANY($4))
              AND ($5 OR inventory_owner_id = ANY($6)))"#,
    )
    .bind(access.tenant_id.get())
    .bind(query.shipment_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut *tx)
    .await?;
    if !visible {
        return Err(AppError::not_found("shipment"));
    }
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
    Ok(documents)
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
    Ok(ShipmentDocumentReadModel {
        document_id: positive(row.try_get("id")?, ShipmentDocumentId::new)?,
        shipment_id: positive(row.try_get("shipment_id")?, ShipmentId::new)?,
        order_id: positive(row.try_get("order_id")?, OrderId::new)?,
        document_type: ShipmentDocumentType::parse(&document_type_text)
            .ok_or_else(|| AppError::internal("shipment document has an invalid type"))?,
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
        demand: ShortShipDemandQuantities::new(
            PickQuantity::new(row.try_get("ordered_qty")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            ActualPickQuantity::new(row.try_get("accepted_short_qty")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?,
        generated_by: positive(row.try_get("generated_by_user_id")?, UserId::new)?,
        generated_at: row.try_get::<Timestamp, _>("generated_at")?,
    })
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
               demand.original_qty, demand.accepted_short_qty, demand.effective_qty,
               COALESCE(SUM(content.packed_qty), 0)::bigint AS packed_qty
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
        WHERE demand.tenant_id = $1 AND demand.inventory_owner_id = $2 AND demand.order_id = $3
        GROUP BY item.id, item.line_number, item.line_key, item.item_id, item.uom,
                 catalog.description, demand.original_qty, demand.accepted_short_qty,
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
            if effective != packed || effective <= 0 {
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
                packed_quantity: packed,
            })
        })
        .collect()
}

async fn load_document_cartons_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    shipment_id: ShipmentId,
) -> AppResult<Vec<DocumentCarton>> {
    let rows = sqlx::query(
        "SELECT sequence, carton_barcode, packed_qty, weight_g FROM shipment_cartons WHERE tenant_id = $1 AND shipment_id = $2 ORDER BY sequence, id",
    )
    .bind(tenant_id.get())
    .bind(shipment_id.get())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(DocumentCarton {
                sequence: row.try_get("sequence")?,
                barcode: row.try_get("carton_barcode")?,
                packed_quantity: row.try_get("packed_qty")?,
                weight_grams: row.try_get("weight_g")?,
            })
        })
        .collect()
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
                   item_description, uom, ordered_qty, accepted_short_qty, packed_qty
               ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)"#,
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
        .bind(line.packed_quantity)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn render_packing_slip(
    shipment_id: ShipmentId,
    order_key: &str,
    addresses: &[AddressSnapshot],
    lines: &[DocumentLine],
    cartons: &[DocumentCarton],
    demand: ShortShipDemandQuantities,
) -> String {
    let origin = addresses.iter().find(|address| address.role == "origin");
    let destination = addresses
        .iter()
        .find(|address| address.role == "destination");
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Packing slip</title><style>body{font:14px system-ui,sans-serif;color:#111;margin:32px}h1{font-size:24px;margin:0 0 8px}h2{font-size:15px;margin:24px 0 8px}.meta,.addresses{display:grid;grid-template-columns:1fr 1fr;gap:24px}table{width:100%;border-collapse:collapse}th,td{text-align:left;border-bottom:1px solid #bbb;padding:7px 5px}.num{text-align:right}.summary{margin-left:auto;width:320px}.muted{color:#555}@media print{body{margin:12mm}}</style></head><body>",
    );
    html.push_str("<h1>Packing slip</h1><div class=\"meta\"><div><strong>Order</strong><br>");
    escape_html_into(order_key, &mut html);
    html.push_str("</div><div><strong>Shipment</strong><br>");
    html.push_str(&shipment_id.get().to_string());
    html.push_str("</div></div><div class=\"addresses\">");
    render_address("Ship from", origin, &mut html);
    render_address("Ship to", destination, &mut html);
    html.push_str("</div><h2>Contents</h2><table><thead><tr><th>Line</th><th>Item</th><th>Description</th><th>UOM</th><th class=\"num\">Ordered</th><th class=\"num\">Packed</th><th class=\"num\">Short</th></tr></thead><tbody>");
    for line in lines {
        html.push_str("<tr><td>");
        escape_html_into(&line.line_key, &mut html);
        html.push_str("</td><td>");
        html.push_str(&line.item_id.get().to_string());
        html.push_str("</td><td>");
        escape_html_into(&line.item_description, &mut html);
        html.push_str("</td><td>");
        escape_html_into(&line.uom, &mut html);
        html.push_str("</td><td class=\"num\">");
        html.push_str(&line.ordered_quantity.to_string());
        html.push_str("</td><td class=\"num\">");
        html.push_str(&line.packed_quantity.to_string());
        html.push_str("</td><td class=\"num\">");
        html.push_str(&line.accepted_short_quantity.to_string());
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table><h2>Cartons</h2><table><thead><tr><th>#</th><th>Carton</th><th class=\"num\">Quantity</th><th class=\"num\">Weight (g)</th></tr></thead><tbody>");
    for carton in cartons {
        html.push_str("<tr><td>");
        html.push_str(&carton.sequence.to_string());
        html.push_str("</td><td>");
        escape_html_into(&carton.barcode, &mut html);
        html.push_str("</td><td class=\"num\">");
        html.push_str(&carton.packed_quantity.to_string());
        html.push_str("</td><td class=\"num\">");
        html.push_str(
            &carton
                .weight_grams
                .map_or_else(|| "-".to_owned(), |weight| weight.to_string()),
        );
        html.push_str("</td></tr>");
    }
    html.push_str(
        "</tbody></table><table class=\"summary\"><tbody><tr><th>Ordered</th><td class=\"num\">",
    );
    html.push_str(&demand.ordered().get().to_string());
    html.push_str("</td></tr><tr><th>Packed</th><td class=\"num\">");
    html.push_str(&demand.effective().get().to_string());
    html.push_str("</td></tr><tr><th>Accepted short</th><td class=\"num\">");
    html.push_str(&demand.accepted_short().get().to_string());
    html.push_str("</td></tr></tbody></table></body></html>");
    html
}

fn render_address(label: &str, address: Option<&AddressSnapshot>, html: &mut String) {
    html.push_str("<section><h2>");
    html.push_str(label);
    html.push_str("</h2>");
    if let Some(address) = address {
        for value in [
            address.name.as_deref(),
            address.company.as_deref(),
            Some(address.line1.as_str()),
            address.line2.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            escape_html_into(value, html);
            html.push_str("<br>");
        }
        escape_html_into(&address.city, html);
        if let Some(state) = address.state.as_deref() {
            html.push_str(", ");
            escape_html_into(state, html);
        }
        html.push(' ');
        escape_html_into(&address.postal_code, html);
        html.push_str("<br>");
        escape_html_into(&address.country, html);
        if let Some(phone) = address.phone.as_deref() {
            html.push_str("<br>");
            escape_html_into(phone, html);
        }
        if let Some(email) = address.email.as_deref() {
            html.push_str("<br>");
            escape_html_into(email, html);
        }
    }
    html.push_str("</section>");
}

fn escape_html_into(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escaping_prevents_snapshot_content_from_becoming_markup() {
        let mut output = String::new();
        escape_html_into("<script>\"x\" & 'y'</script>", &mut output);
        assert_eq!(
            output,
            "&lt;script&gt;&quot;x&quot; &amp; &#39;y&#39;&lt;/script&gt;"
        );
    }
}
