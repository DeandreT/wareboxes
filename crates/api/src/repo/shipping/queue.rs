use sqlx::Row;
use wareboxes_application::outbound_qa::{
    OutboundQaPolicyReadModel, OutboundQaSessionSummaryReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    FacilityId, FacilityRevision, InventoryOwnerId, OrderId, OrderRevision, OutboundQaPolicyId,
    OutboundQaPolicyRevision, OutboundQaProgress, OutboundQaRequirement, OutboundQaSessionId,
    OutboundQaSessionRevision, OutboundQaSessionStatus, PackSessionId, ShipmentId,
    ShipmentRevision, ShipmentStatus, Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{begin_tenant_transaction, Db};

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

const MAX_QUEUE_PAGE_SIZE: u16 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippingQueueCursor {
    pub facility_id: Option<i64>,
    pub rush_rank: i16,
    pub ship_by: Option<Timestamp>,
    pub order_id: OrderId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippingQueueShipment {
    pub shipment_id: ShipmentId,
    pub status: ShipmentStatus,
    pub revision: ShipmentRevision,
    pub carton_count: i64,
    pub shipped_quantity: i64,
    pub departed_carton_count: i64,
    pub departed_quantity: i64,
    pub carrier_code: Option<String>,
    pub service_code: Option<String>,
    pub created_at: Timestamp,
    pub manifested_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippingQueueEntry {
    pub order_id: OrderId,
    pub order_key: String,
    pub order_revision: OrderRevision,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub facility_revision: FacilityRevision,
    pub packing_session_id: PackSessionId,
    pub rush: bool,
    pub ship_by: Option<Timestamp>,
    pub origin_ready: bool,
    pub destination_ready: bool,
    pub outbound_qa_policy: Option<OutboundQaPolicyReadModel>,
    pub outbound_qa_session: Option<OutboundQaSessionSummaryReadModel>,
    pub shipment: Option<ShippingQueueShipment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippingQueuePage {
    pub items: Vec<ShippingQueueEntry>,
    pub next_cursor: Option<ShippingQueueCursor>,
}

pub async fn shipping_queue(
    db: &Db,
    access: &TenantAccess,
    facility_id: Option<i64>,
    after: Option<&ShippingQueueCursor>,
    limit: u16,
) -> AppResult<ShippingQueuePage> {
    if limit == 0 || limit > MAX_QUEUE_PAGE_SIZE {
        return Err(AppError::bad_request(
            "shipping queue page size is outside the supported range",
        ));
    }
    if facility_id.is_some_and(|id| id <= 0)
        || after.is_some_and(|cursor| {
            cursor.facility_id != facility_id || !matches!(cursor.rush_rank, 0 | 1)
        })
    {
        return Err(AppError::bad_request(
            "shipping queue cursor does not match its facility filter",
        ));
    }

    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let after_rush_rank = after.map(|cursor| cursor.rush_rank);
    let after_ship_by = after.and_then(|cursor| cursor.ship_by);
    let after_order_id = after.map(|cursor| cursor.order_id.get());
    let fetch_limit = i64::from(limit) + 1;
    let rows = sqlx::query(
        r#"
        SELECT order_header.id AS order_id,
               order_header.order_key,
               order_header.revision AS order_revision,
               order_header.inventory_owner_id,
               inventory_owner.name AS inventory_owner_name,
               session.facility_id,
               facility.name AS facility_name,
               facility.revision AS facility_revision,
               session.id AS packing_session_id,
               session.revision AS packing_session_revision,
               order_header.rush,
               order_header.ship_by,
               (
                   origin.id IS NOT NULL
                   AND (NULLIF(btrim(origin.name), '') IS NOT NULL
                        OR NULLIF(btrim(origin.company), '') IS NOT NULL)
                   AND NULLIF(btrim(origin.line1), '') IS NOT NULL
                   AND NULLIF(btrim(origin.city), '') IS NOT NULL
                   AND NULLIF(btrim(origin.postal_code), '') IS NOT NULL
                   AND NULLIF(btrim(origin.country), '') IS NOT NULL
               ) AS origin_ready,
               (
                   destination.id IS NOT NULL
                   AND (NULLIF(btrim(destination.name), '') IS NOT NULL
                        OR NULLIF(btrim(destination.company), '') IS NOT NULL)
                   AND NULLIF(btrim(destination.line1), '') IS NOT NULL
                   AND NULLIF(btrim(destination.city), '') IS NOT NULL
                   AND NULLIF(btrim(destination.postal_code), '') IS NOT NULL
                   AND NULLIF(btrim(destination.country), '') IS NOT NULL
               ) AS destination_ready,
               qa_policy.id AS qa_policy_id,
               qa_policy.requirement AS qa_requirement,
               qa_policy.revision AS qa_policy_revision,
               qa_policy.configured_by_user_id AS qa_configured_by_user_id,
               qa_policy.configured_at AS qa_configured_at,
               qa_session.id AS qa_session_id,
               qa_session.policy_revision AS qa_session_policy_revision,
               qa_session.state AS qa_session_state,
               qa_session.revision AS qa_session_revision,
               qa_session.expected_carton_count AS qa_expected_carton_count,
               qa_session.verified_carton_count AS qa_verified_carton_count,
               qa_session.started_at AS qa_started_at,
               qa_session.passed_at AS qa_passed_at,
               shipment.id AS shipment_id,
               shipment.state AS shipment_state,
               shipment.revision AS shipment_revision,
               shipment.carton_count,
               shipment.shipped_qty,
               shipment.departed_carton_count,
               shipment.departed_qty,
               shipment.carrier,
               shipment.service,
               shipment.created_at,
               shipment.manifested_at
        FROM orders order_header
        INNER JOIN packing_sessions session
          ON session.tenant_id = order_header.tenant_id
         AND session.inventory_owner_id = order_header.inventory_owner_id
         AND session.order_id = order_header.id
         AND session.state = 'ready_to_manifest'
        INNER JOIN inventory_owners inventory_owner
          ON inventory_owner.tenant_id = order_header.tenant_id
         AND inventory_owner.id = order_header.inventory_owner_id
         AND inventory_owner.deleted IS NULL
        INNER JOIN facilities facility
          ON facility.tenant_id = session.tenant_id
         AND facility.id = session.facility_id
         AND facility.deleted IS NULL
        LEFT JOIN addresses origin
          ON origin.tenant_id = facility.tenant_id
         AND origin.id = facility.address_id
         AND origin.deleted IS NULL
        LEFT JOIN addresses destination
          ON destination.tenant_id = order_header.tenant_id
         AND destination.id = order_header.address_id
         AND destination.deleted IS NULL
        LEFT JOIN outbound_qa_policies qa_policy
          ON qa_policy.tenant_id = session.tenant_id
         AND qa_policy.inventory_owner_id = session.inventory_owner_id
         AND qa_policy.facility_id = session.facility_id
         AND qa_policy.effective_to IS NULL
        LEFT JOIN outbound_qa_sessions qa_session
          ON qa_session.tenant_id = session.tenant_id
         AND qa_session.inventory_owner_id = session.inventory_owner_id
         AND qa_session.facility_id = session.facility_id
         AND qa_session.packing_session_id = session.id
         AND qa_session.policy_id = qa_policy.id
        LEFT JOIN shipments shipment
          ON shipment.tenant_id = session.tenant_id
         AND shipment.inventory_owner_id = session.inventory_owner_id
         AND shipment.facility_id = session.facility_id
         AND shipment.packing_session_id = session.id
         AND shipment.order_id = order_header.id
        WHERE order_header.tenant_id = $1
          AND order_header.deleted IS NULL
          AND order_header.status = 'awaiting shipment'
          AND ($2 OR session.facility_id = ANY($3))
          AND ($4 OR order_header.inventory_owner_id = ANY($5))
          AND ($6::BIGINT IS NULL OR session.facility_id = $6)
          AND (
              $7::SMALLINT IS NULL
              OR (
                  CASE WHEN order_header.rush THEN 0 ELSE 1 END,
                  COALESCE(order_header.ship_by, 'infinity'::TIMESTAMPTZ),
                  order_header.id
              ) > (
                  $7,
                  COALESCE($8::TIMESTAMPTZ, 'infinity'::TIMESTAMPTZ),
                  $9
              )
          )
        ORDER BY order_header.rush DESC,
                 order_header.ship_by ASC NULLS LAST,
                 order_header.id ASC
        LIMIT $10
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(facility_id)
    .bind(after_rush_rank)
    .bind(after_ship_by)
    .bind(after_order_id)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await?;

    let mut items = rows
        .into_iter()
        .map(entry_from_row)
        .collect::<AppResult<Vec<_>>>()?;
    let has_more = items.len() > usize::from(limit);
    if has_more {
        items.pop();
    }
    let next_cursor = has_more
        .then(|| items.last().map(cursor_for_entry))
        .flatten()
        .map(|mut cursor| {
            cursor.facility_id = facility_id;
            cursor
        });
    tx.commit().await?;
    Ok(ShippingQueuePage { items, next_cursor })
}

fn entry_from_row(row: sqlx::postgres::PgRow) -> AppResult<ShippingQueueEntry> {
    let packing_revision = positive_order_revision(row.try_get("packing_session_revision")?)?;
    let order_revision = positive_order_revision(row.try_get("order_revision")?)?;
    let shipment_id: Option<i64> = row.try_get("shipment_id")?;
    let shipment = shipment_id
        .map(|shipment_id| {
            let state: String = required(&row, "shipment_state")?;
            let shipment = ShippingQueueShipment {
                shipment_id: ShipmentId::new(shipment_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                status: ShipmentStatus::parse(&state).ok_or_else(|| {
                    AppError::internal("shipping queue has invalid shipment state")
                })?,
                revision: ShipmentRevision::new(required(&row, "shipment_revision")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                carton_count: required(&row, "carton_count")?,
                shipped_quantity: required(&row, "shipped_qty")?,
                departed_carton_count: required(&row, "departed_carton_count")?,
                departed_quantity: required(&row, "departed_qty")?,
                carrier_code: row.try_get("carrier")?,
                service_code: row.try_get("service")?,
                created_at: required(&row, "created_at")?,
                manifested_at: row.try_get("manifested_at")?,
            };
            let lifecycle_consistent = match shipment.status {
                ShipmentStatus::AwaitingManifest => {
                    shipment.revision.get() == 1
                        && shipment.carrier_code.is_none()
                        && shipment.service_code.is_none()
                        && shipment.manifested_at.is_none()
                }
                ShipmentStatus::Manifested => {
                    shipment.revision.get() == 2
                        && shipment.carrier_code.is_some()
                        && shipment.manifested_at.is_some()
                }
                ShipmentStatus::PartiallyDeparted => {
                    shipment.revision.get() >= 3
                        && shipment.carrier_code.is_some()
                        && shipment.manifested_at.is_some()
                        && shipment.departed_carton_count > 0
                        && shipment.departed_carton_count < shipment.carton_count
                        && shipment.departed_quantity > 0
                        && shipment.departed_quantity < shipment.shipped_quantity
                }
                ShipmentStatus::Departed => false,
            };
            let expected_order_revision =
                packing_revision.get().checked_add(1).and_then(|revision| {
                    revision.checked_add(shipment.revision.get().saturating_sub(2).max(0))
                });
            if !lifecycle_consistent
                || shipment.carton_count <= 0
                || shipment.shipped_quantity <= 0
                || expected_order_revision != Some(order_revision.get())
            {
                return Err(AppError::internal(
                    format!(
                        "shipping queue order and shipment state are inconsistent: status={:?} shipment_revision={} packing_revision={} order_revision={} expected={expected_order_revision:?} departed_cartons={} departed_qty={}",
                        shipment.status,
                        shipment.revision.get(),
                        packing_revision.get(),
                        order_revision.get(),
                        shipment.departed_carton_count,
                        shipment.departed_quantity,
                    ),
                ));
            }
            Ok(shipment)
        })
        .transpose()?;
    if shipment.is_none() && order_revision != packing_revision {
        return Err(AppError::internal(
            "shipping queue order and packing revision are inconsistent",
        ));
    }
    let owner_id = required_positive(&row, "inventory_owner_id")?;
    let facility_id = required_positive(&row, "facility_id")?;
    let qa_policy_id: Option<i64> = row.try_get("qa_policy_id")?;
    let outbound_qa_policy = qa_policy_id
        .map(|policy_id| {
            let requirement: String = required(&row, "qa_requirement")?;
            Ok::<OutboundQaPolicyReadModel, AppError>(OutboundQaPolicyReadModel {
                policy_id: OutboundQaPolicyId::new(policy_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                inventory_owner_id: InventoryOwnerId::new(owner_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                facility_id: FacilityId::new(facility_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                requirement: OutboundQaRequirement::parse(&requirement).ok_or_else(|| {
                    AppError::internal("shipping queue has invalid outbound QA requirement")
                })?,
                revision: OutboundQaPolicyRevision::new(required(&row, "qa_policy_revision")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                configured_by: UserId::new(required(&row, "qa_configured_by_user_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                configured_at: required(&row, "qa_configured_at")?,
            })
        })
        .transpose()?;
    let qa_session_id: Option<i64> = row.try_get("qa_session_id")?;
    let outbound_qa_session = qa_session_id
        .map(|session_id| {
            let status_text: String = required(&row, "qa_session_state")?;
            let status = OutboundQaSessionStatus::parse(&status_text)
                .ok_or_else(|| AppError::internal("shipping queue has invalid QA status"))?;
            let expected = required(&row, "qa_expected_carton_count")?;
            let verified = required(&row, "qa_verified_carton_count")?;
            let progress = OutboundQaProgress::new(expected, verified)
                .map_err(|error| AppError::internal(error.to_string()))?;
            let passed_at: Option<Timestamp> = row.try_get("qa_passed_at")?;
            if expected <= 0
                || (status == OutboundQaSessionStatus::Passed)
                    != (progress.is_complete() && passed_at.is_some())
            {
                return Err(AppError::internal(
                    "shipping queue outbound QA state is inconsistent",
                ));
            }
            Ok(OutboundQaSessionSummaryReadModel {
                session_id: OutboundQaSessionId::new(session_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                policy_id: OutboundQaPolicyId::new(qa_policy_id.ok_or_else(|| {
                    AppError::internal("shipping queue QA session has no policy")
                })?)
                .map_err(|error| AppError::internal(error.to_string()))?,
                policy_revision: OutboundQaPolicyRevision::new(required(
                    &row,
                    "qa_session_policy_revision",
                )?)
                .map_err(|error| AppError::internal(error.to_string()))?,
                status,
                revision: OutboundQaSessionRevision::new(required(&row, "qa_session_revision")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                progress,
                started_at: required(&row, "qa_started_at")?,
                passed_at,
            })
        })
        .transpose()?;
    if let (Some(policy), Some(session)) = (&outbound_qa_policy, &outbound_qa_session) {
        if session.policy_id != policy.policy_id || session.policy_revision != policy.revision {
            return Err(AppError::internal(
                "shipping queue outbound QA policy snapshot is inconsistent",
            ));
        }
    }

    Ok(ShippingQueueEntry {
        order_id: OrderId::new(row.try_get("order_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        order_key: row.try_get("order_key")?,
        order_revision,
        inventory_owner_id: owner_id,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id,
        facility_name: row.try_get("facility_name")?,
        facility_revision: FacilityRevision::new(row.try_get("facility_revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        packing_session_id: PackSessionId::new(row.try_get("packing_session_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        rush: row.try_get("rush")?,
        ship_by: row.try_get("ship_by")?,
        origin_ready: row.try_get("origin_ready")?,
        destination_ready: row.try_get("destination_ready")?,
        outbound_qa_policy,
        outbound_qa_session,
        shipment,
    })
}

fn required<T>(row: &sqlx::postgres::PgRow, column: &str) -> AppResult<T>
where
    T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get::<Option<T>, _>(column)?
        .ok_or_else(|| AppError::internal(format!("shipping queue has no {column}")))
}

fn required_positive(row: &sqlx::postgres::PgRow, column: &str) -> AppResult<i64> {
    let value = required::<i64>(row, column)?;
    if value > 0 {
        Ok(value)
    } else {
        Err(AppError::internal(format!(
            "shipping queue has invalid {column}"
        )))
    }
}

fn positive_order_revision(value: i64) -> AppResult<OrderRevision> {
    OrderRevision::new(value).map_err(|error| AppError::internal(error.to_string()))
}

fn cursor_for_entry(entry: &ShippingQueueEntry) -> ShippingQueueCursor {
    ShippingQueueCursor {
        facility_id: None,
        rush_rank: if entry.rush { 0 } else { 1 },
        ship_by: entry.ship_by,
        order_id: entry.order_id,
    }
}
