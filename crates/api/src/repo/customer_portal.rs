//! Scope-safe customer visibility projections.

use sqlx::Row;
use wareboxes_application::customer_portal::{
    CustomerPortalDocument, CustomerPortalDocumentContent, CustomerPortalInventoryLine,
    CustomerPortalOrder, CustomerPortalQuery, CustomerPortalShipment, CustomerPortalWorkspace,
    CUSTOMER_PORTAL_PERMISSION, MAX_CUSTOMER_PORTAL_RESULTS,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{OrderStatus, ShipmentDocumentType, ShipmentStatus};
use wareboxes_persistence_postgres::db::{begin_tenant_transaction, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub async fn workspace(
    db: &Db,
    access: &TenantAccess,
    query: &CustomerPortalQuery,
) -> AppResult<CustomerPortalWorkspace> {
    validate_query(access, query)?;
    let search = query.search.as_deref();
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        CUSTOMER_PORTAL_PERMISSION,
    )
    .await?;

    let inventory_rows = sqlx::query(
        r#"
        SELECT balance.inventory_owner_id, owner.name AS inventory_owner_name,
               balance.facility_id, facility.name AS facility_name,
               balance.item_id, item.description AS item_description,
               sku.name AS primary_sku, batch.lot, batch.expiration,
               balance.uom, balance.status,
               SUM(balance.qty_on_hand)::BIGINT AS on_hand,
               SUM(balance.qty_reserved)::BIGINT AS reserved,
               SUM(balance.qty_held)::BIGINT AS held
        FROM inventory_balances balance
        INNER JOIN inventory_owners owner
          ON owner.tenant_id=balance.tenant_id AND owner.id=balance.inventory_owner_id
        INNER JOIN facilities facility
          ON facility.tenant_id=balance.tenant_id AND facility.id=balance.facility_id
        INNER JOIN items item
          ON item.tenant_id=balance.tenant_id AND item.id=balance.item_id
        INNER JOIN item_batches batch
          ON batch.tenant_id=balance.tenant_id
         AND batch.inventory_owner_id=balance.inventory_owner_id
         AND batch.id=balance.item_batch_id
        LEFT JOIN LATERAL (
            SELECT item_sku.name
            FROM skus item_sku
            WHERE item_sku.tenant_id=balance.tenant_id
              AND item_sku.item_id=balance.item_id
              AND item_sku.deleted IS NULL
            ORDER BY item_sku.id
            LIMIT 1
        ) sku ON TRUE
        WHERE balance.tenant_id=$1 AND balance.deleted IS NULL
          AND ($2 OR balance.facility_id=ANY($3))
          AND ($4 OR balance.inventory_owner_id=ANY($5))
          AND ($6::BIGINT IS NULL OR balance.inventory_owner_id=$6)
          AND ($7::BIGINT IS NULL OR balance.facility_id=$7)
          AND ($8::TEXT IS NULL
               OR STRPOS(LOWER(owner.name),LOWER($8))>0
               OR STRPOS(LOWER(facility.name),LOWER($8))>0
               OR STRPOS(LOWER(COALESCE(sku.name,'')),LOWER($8))>0
               OR STRPOS(LOWER(COALESCE(item.description,'')),LOWER($8))>0
               OR STRPOS(LOWER(COALESCE(batch.lot,'')),LOWER($8))>0)
        GROUP BY balance.inventory_owner_id, owner.name, balance.facility_id,
                 facility.name, balance.item_id, item.description, sku.name,
                 batch.lot, batch.expiration, balance.uom, balance.status
        ORDER BY owner.name, facility.name, COALESCE(sku.name,item.description,''),
                 batch.lot NULLS FIRST, balance.uom, balance.status
        LIMIT $9
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(query.inventory_owner_id)
    .bind(query.facility_id)
    .bind(search)
    .bind(MAX_CUSTOMER_PORTAL_RESULTS)
    .fetch_all(&mut *tx)
    .await?;

    let order_rows = sqlx::query(
        r#"
        SELECT order_header.id, order_header.order_key,
               order_header.inventory_owner_id, owner.name AS inventory_owner_name,
               order_facility.facility_id, facility.name AS facility_name,
               order_header.status, order_header.rush, order_header.ship_by,
               order_header.created, address.company AS destination_company,
               address.city AS destination_city, address.state AS destination_region,
               address.country AS destination_country,
               COALESCE((SELECT SUM(line.qty)::BIGINT FROM order_items line
                         WHERE line.tenant_id=order_header.tenant_id
                           AND line.inventory_owner_id=order_header.inventory_owner_id
                           AND line.order_id=order_header.id AND line.deleted IS NULL),0) AS ordered_quantity
        FROM orders order_header
        INNER JOIN inventory_owners owner
          ON owner.tenant_id=order_header.tenant_id AND owner.id=order_header.inventory_owner_id
        INNER JOIN addresses address
          ON address.tenant_id=order_header.tenant_id AND address.id=order_header.address_id
        LEFT JOIN LATERAL (
            SELECT candidate.facility_id
            FROM (
                SELECT shipment.facility_id, shipment.created_at AS observed_at
                FROM shipments shipment
                WHERE shipment.tenant_id=order_header.tenant_id
                  AND shipment.inventory_owner_id=order_header.inventory_owner_id
                  AND shipment.order_id=order_header.id
                UNION ALL
                SELECT release.facility_id, release.released_at
                FROM order_releases release
                WHERE release.tenant_id=order_header.tenant_id
                  AND release.inventory_owner_id=order_header.inventory_owner_id
                  AND release.order_id=order_header.id
                UNION ALL
                SELECT reservation.facility_id, reservation.created
                FROM inventory_reservations reservation
                WHERE reservation.tenant_id=order_header.tenant_id
                  AND reservation.inventory_owner_id=order_header.inventory_owner_id
                  AND reservation.order_id=order_header.id
            ) candidate
            ORDER BY candidate.observed_at DESC, candidate.facility_id
            LIMIT 1
        ) order_facility ON TRUE
        LEFT JOIN facilities facility
          ON facility.tenant_id=order_header.tenant_id
         AND facility.id=order_facility.facility_id
        WHERE order_header.tenant_id=$1 AND order_header.deleted IS NULL
          AND ($2 OR order_header.inventory_owner_id=ANY($3))
          AND ($4 OR order_facility.facility_id=ANY($5))
          AND ($6::BIGINT IS NULL OR order_header.inventory_owner_id=$6)
          AND ($7::BIGINT IS NULL OR order_facility.facility_id=$7)
          AND ($8 OR order_header.status NOT IN ('shipped','cancelled','void'))
          AND ($9::TEXT IS NULL
               OR STRPOS(LOWER(order_header.order_key),LOWER($9))>0
               OR STRPOS(LOWER(owner.name),LOWER($9))>0
               OR STRPOS(LOWER(COALESCE(address.company,'')),LOWER($9))>0)
        ORDER BY order_header.created DESC, order_header.id DESC
        LIMIT $10
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(query.inventory_owner_id)
    .bind(query.facility_id)
    .bind(query.include_history)
    .bind(search)
    .bind(MAX_CUSTOMER_PORTAL_RESULTS)
    .fetch_all(&mut *tx)
    .await?;

    let shipment_rows = sqlx::query(
        r#"
        SELECT shipment.id, shipment.order_id, order_header.order_key,
               shipment.inventory_owner_id, owner.name AS inventory_owner_name,
               shipment.facility_id, facility.name AS facility_name,
               CASE WHEN cancellation.id IS NULL THEN shipment.state ELSE 'cancelled' END AS status,
               shipment.carton_count, shipment.shipped_qty, shipment.carrier,
               shipment.service, shipment.created_at, shipment.manifested_at,
               shipment.departed_at,
               COALESCE(ARRAY_AGG(DISTINCT package.tracking_number)
                        FILTER (WHERE package.tracking_number IS NOT NULL),'{}') AS tracking_numbers
        FROM shipments shipment
        INNER JOIN orders order_header
          ON order_header.tenant_id=shipment.tenant_id
         AND order_header.inventory_owner_id=shipment.inventory_owner_id
         AND order_header.id=shipment.order_id
        INNER JOIN inventory_owners owner
          ON owner.tenant_id=shipment.tenant_id AND owner.id=shipment.inventory_owner_id
        INNER JOIN facilities facility
          ON facility.tenant_id=shipment.tenant_id AND facility.id=shipment.facility_id
        LEFT JOIN shipment_cancellations cancellation
          ON cancellation.tenant_id=shipment.tenant_id
         AND cancellation.inventory_owner_id=shipment.inventory_owner_id
         AND cancellation.facility_id=shipment.facility_id
         AND cancellation.shipment_id=shipment.id
        LEFT JOIN shipment_manifest_packages package
          ON package.tenant_id=shipment.tenant_id
         AND package.inventory_owner_id=shipment.inventory_owner_id
         AND package.facility_id=shipment.facility_id
         AND package.shipment_id=shipment.id
        WHERE shipment.tenant_id=$1
          AND ($2 OR shipment.facility_id=ANY($3))
          AND ($4 OR shipment.inventory_owner_id=ANY($5))
          AND ($6::BIGINT IS NULL OR shipment.inventory_owner_id=$6)
          AND ($7::BIGINT IS NULL OR shipment.facility_id=$7)
          AND ($8 OR (shipment.state<>'departed' AND cancellation.id IS NULL))
          AND ($9::TEXT IS NULL
               OR STRPOS(LOWER(order_header.order_key),LOWER($9))>0
               OR STRPOS(LOWER(owner.name),LOWER($9))>0
               OR STRPOS(LOWER(COALESCE(shipment.carrier,'')),LOWER($9))>0
               OR STRPOS(LOWER(COALESCE(package.tracking_number,'')),LOWER($9))>0)
        GROUP BY shipment.id, shipment.order_id, order_header.order_key,
                 shipment.inventory_owner_id, owner.name, shipment.facility_id,
                 facility.name, cancellation.id
        ORDER BY shipment.created_at DESC, shipment.id DESC
        LIMIT $10
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(query.inventory_owner_id)
    .bind(query.facility_id)
    .bind(query.include_history)
    .bind(search)
    .bind(MAX_CUSTOMER_PORTAL_RESULTS)
    .fetch_all(&mut *tx)
    .await?;

    let document_rows = sqlx::query(
        r#"
        SELECT document.id, document.shipment_id, document.order_id,
               order_header.order_key, document.inventory_owner_id,
               document.facility_id, document.document_type, document.file_name,
               document.media_type, document.content_length,
               document.content_sha256, document.generated_at
        FROM shipment_documents document
        INNER JOIN orders order_header
          ON order_header.tenant_id=document.tenant_id
         AND order_header.inventory_owner_id=document.inventory_owner_id
         AND order_header.id=document.order_id
        WHERE document.tenant_id=$1
          AND ($2 OR document.facility_id=ANY($3))
          AND ($4 OR document.inventory_owner_id=ANY($5))
          AND ($6::BIGINT IS NULL OR document.inventory_owner_id=$6)
          AND ($7::BIGINT IS NULL OR document.facility_id=$7)
          AND ($8::TEXT IS NULL
               OR STRPOS(LOWER(order_header.order_key),LOWER($8))>0
               OR STRPOS(LOWER(document.file_name),LOWER($8))>0)
        ORDER BY document.generated_at DESC, document.id DESC
        LIMIT $9
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(query.inventory_owner_id)
    .bind(query.facility_id)
    .bind(search)
    .bind(MAX_CUSTOMER_PORTAL_RESULTS)
    .fetch_all(&mut *tx)
    .await?;

    let inventory = inventory_rows
        .iter()
        .map(map_inventory)
        .collect::<AppResult<Vec<_>>>()?;
    let orders = order_rows
        .iter()
        .map(map_order)
        .collect::<AppResult<Vec<_>>>()?;
    let shipments = shipment_rows
        .iter()
        .map(map_shipment)
        .collect::<AppResult<Vec<_>>>()?;
    let documents = document_rows
        .iter()
        .map(map_document)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(CustomerPortalWorkspace {
        inventory,
        orders,
        shipments,
        documents,
    })
}

pub async fn document_content(
    db: &Db,
    access: &TenantAccess,
    document_id: i64,
) -> AppResult<CustomerPortalDocumentContent> {
    if document_id <= 0 {
        return Err(AppError::bad_request(
            "shipment document ID must be positive",
        ));
    }
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        CUSTOMER_PORTAL_PERMISSION,
    )
    .await?;
    let row = sqlx::query(
        r#"
        SELECT document.id, document.shipment_id, document.order_id,
               order_header.order_key, document.inventory_owner_id,
               document.facility_id, document.document_type, document.file_name,
               document.media_type, document.content_length,
               document.content_sha256, document.generated_at, document.content
        FROM shipment_documents document
        INNER JOIN orders order_header
          ON order_header.tenant_id=document.tenant_id
         AND order_header.inventory_owner_id=document.inventory_owner_id
         AND order_header.id=document.order_id
        WHERE document.tenant_id=$1 AND document.id=$2
          AND ($3 OR document.facility_id=ANY($4))
          AND ($5 OR document.inventory_owner_id=ANY($6))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(document_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("shipment document"))?;
    let document = map_document(&row)?;
    let content: String = row.try_get("content")?;
    tx.commit().await?;
    Ok(CustomerPortalDocumentContent {
        document,
        content: content.into_bytes(),
    })
}

fn validate_query(access: &TenantAccess, query: &CustomerPortalQuery) -> AppResult<()> {
    if let Some(owner_id) = query.inventory_owner_id {
        if owner_id <= 0 || !access.owner_scope.includes_raw(owner_id) {
            return Err(AppError::forbidden());
        }
    }
    if let Some(facility_id) = query.facility_id {
        if facility_id <= 0 || !access.site_scope.includes_raw(facility_id) {
            return Err(AppError::forbidden());
        }
    }
    if let Some(search) = query.search.as_deref() {
        if search.is_empty()
            || search.trim() != search
            || search.chars().count() > 100
            || search.chars().any(char::is_control)
        {
            return Err(AppError::bad_request(
                "portal search must be trimmed and at most 100 characters",
            ));
        }
    }
    Ok(())
}

fn map_inventory(row: &sqlx::postgres::PgRow) -> AppResult<CustomerPortalInventoryLine> {
    let on_hand: i64 = row.try_get("on_hand")?;
    let reserved: i64 = row.try_get("reserved")?;
    let held: i64 = row.try_get("held")?;
    let available = on_hand
        .checked_sub(reserved)
        .and_then(|value| value.checked_sub(held))
        .ok_or_else(|| AppError::internal("portal inventory quantity overflow"))?;
    Ok(CustomerPortalInventoryLine {
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: row.try_get("facility_id")?,
        facility_name: row.try_get("facility_name")?,
        item_id: row.try_get("item_id")?,
        item_description: row.try_get("item_description")?,
        primary_sku: row.try_get("primary_sku")?,
        lot: row.try_get("lot")?,
        expiration: row.try_get("expiration")?,
        uom: row.try_get("uom")?,
        status: row.try_get("status")?,
        on_hand,
        reserved,
        held,
        available,
    })
}

fn map_order(row: &sqlx::postgres::PgRow) -> AppResult<CustomerPortalOrder> {
    let status: String = row.try_get("status")?;
    Ok(CustomerPortalOrder {
        order_id: row.try_get("id")?,
        order_key: row.try_get("order_key")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: row.try_get("facility_id")?,
        facility_name: row.try_get("facility_name")?,
        status: OrderStatus::parse(&status)
            .ok_or_else(|| AppError::internal("portal order has an invalid status"))?,
        rush: row.try_get("rush")?,
        ordered_quantity: row.try_get("ordered_quantity")?,
        ship_by: row.try_get("ship_by")?,
        created_at: row.try_get("created")?,
        destination_company: row.try_get("destination_company")?,
        destination_city: row.try_get("destination_city")?,
        destination_region: row.try_get("destination_region")?,
        destination_country: row.try_get("destination_country")?,
    })
}

fn map_shipment(row: &sqlx::postgres::PgRow) -> AppResult<CustomerPortalShipment> {
    let status: String = row.try_get("status")?;
    Ok(CustomerPortalShipment {
        shipment_id: row.try_get("id")?,
        order_id: row.try_get("order_id")?,
        order_key: row.try_get("order_key")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: row.try_get("facility_id")?,
        facility_name: row.try_get("facility_name")?,
        status: ShipmentStatus::parse(&status)
            .ok_or_else(|| AppError::internal("portal shipment has an invalid status"))?,
        carton_count: row.try_get("carton_count")?,
        shipped_quantity: row.try_get("shipped_qty")?,
        carrier: row.try_get("carrier")?,
        service: row.try_get("service")?,
        tracking_numbers: row.try_get("tracking_numbers")?,
        created_at: row.try_get("created_at")?,
        manifested_at: row.try_get("manifested_at")?,
        departed_at: row.try_get("departed_at")?,
    })
}

fn map_document(row: &sqlx::postgres::PgRow) -> AppResult<CustomerPortalDocument> {
    let document_type: String = row.try_get("document_type")?;
    let content_sha256: Vec<u8> = row.try_get("content_sha256")?;
    Ok(CustomerPortalDocument {
        document_id: row.try_get("id")?,
        shipment_id: row.try_get("shipment_id")?,
        order_id: row.try_get("order_id")?,
        order_key: row.try_get("order_key")?,
        inventory_owner_id: row.try_get("inventory_owner_id")?,
        facility_id: row.try_get("facility_id")?,
        document_type: ShipmentDocumentType::parse(&document_type)
            .ok_or_else(|| AppError::internal("portal document has an invalid type"))?,
        file_name: row.try_get("file_name")?,
        media_type: row.try_get("media_type")?,
        content_length: row.try_get("content_length")?,
        content_sha256: hex::encode(content_sha256),
        generated_at: row.try_get("generated_at")?,
    })
}

trait IncludesRaw {
    fn includes_raw(&self, id: i64) -> bool;
}

impl IncludesRaw for wareboxes_domain::OwnerScope {
    fn includes_raw(&self, id: i64) -> bool {
        self.all_inventory_owners
            || self
                .inventory_owner_ids
                .iter()
                .any(|candidate| candidate.get() == id)
    }
}

impl IncludesRaw for wareboxes_domain::SiteScope {
    fn includes_raw(&self, id: i64) -> bool {
        self.all_facilities
            || self
                .facility_ids
                .iter()
                .any(|candidate| candidate.get() == id)
    }
}
