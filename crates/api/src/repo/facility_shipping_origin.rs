//! Atomic facility shipping-origin configuration.

use sqlx::Row;
use wareboxes_application::facility_shipping_origin::{
    ConfigureFacilityShippingOriginCommand, ConfigureFacilityShippingOriginResult,
    FACILITY_SHIPPING_ORIGIN_CONFIGURE_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    AddressId, FacilityId, FacilityRevision, FacilityShippingOrigin,
    FacilityShippingOriginConfigurationId, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::address::{insert_address_tx, NewAddress};
use crate::repo::orders;

struct LockedFacility {
    previous_address_id: Option<i64>,
    revision: FacilityRevision,
}

pub async fn configure_facility_shipping_origin(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigureFacilityShippingOriginCommand,
) -> AppResult<ConfigureFacilityShippingOriginResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(
        context,
        FACILITY_SHIPPING_ORIGIN_CONFIGURE_OPERATION,
        command,
    )?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "admin").await?;

    if let Some(result) = prepared
        .replayed::<ConfigureFacilityShippingOriginResult>(&mut tx)
        .await?
    {
        require_replayed_configuration_visible_tx(&mut tx, access.tenant_id, &result, &scope)
            .await?;
        tx.commit().await?;
        return Ok(result);
    }

    let facility =
        lock_facility_tx(&mut tx, access.tenant_id, command.facility_id(), &scope).await?;
    if facility.revision != command.expected_revision() {
        return Err(AppError::conflict(format!(
            "facility revision changed from {} to {}",
            command.expected_revision().get(),
            facility.revision.get()
        )));
    }
    let revision = facility
        .revision
        .checked_next()
        .ok_or_else(|| AppError::internal("facility revision overflow"))?;
    let configured_at = now_iso();
    let address_id = insert_origin_address_tx(&mut tx, access.tenant_id, command.origin()).await?;
    update_facility_origin_tx(
        &mut tx,
        access.tenant_id,
        command.facility_id(),
        facility.revision,
        address_id,
    )
    .await?;
    let configuration_id = insert_configuration_audit_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        command,
        facility.previous_address_id,
        address_id,
        revision,
        configured_at,
    )
    .await?;
    let result = ConfigureFacilityShippingOriginResult {
        configuration_id,
        facility_id: command.facility_id(),
        address_id,
        revision,
        origin: command.origin().clone(),
        configured_by: context.actor_id,
        configured_at,
    };
    enqueue_configured_event_tx(&mut tx, access.tenant_id, &result).await?;
    Ok(prepared.commit(tx, result).await?)
}

async fn lock_facility_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    scope: &ScopeBindings,
) -> AppResult<LockedFacility> {
    let row = sqlx::query(
        r#"
        SELECT address_id, revision
        FROM facilities
        WHERE tenant_id = $1
          AND id = $2
          AND deleted IS NULL
          AND ($3 OR id = ANY($4))
        FOR UPDATE
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("facility"))?;
    Ok(LockedFacility {
        previous_address_id: row.try_get("address_id")?,
        revision: FacilityRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

async fn require_replayed_configuration_visible_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &ConfigureFacilityShippingOriginResult,
    scope: &ScopeBindings,
) -> AppResult<()> {
    if !scope.includes_facility(result.facility_id.get()) {
        return Err(AppError::not_found(
            "facility shipping origin configuration",
        ));
    }
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM facility_shipping_origin_configurations configuration
            INNER JOIN facilities facility
                ON facility.tenant_id = configuration.tenant_id
               AND facility.id = configuration.facility_id
            WHERE configuration.tenant_id = $1
              AND configuration.id = $2
              AND configuration.facility_id = $3
              AND configuration.address_id = $4
              AND facility.deleted IS NULL
        )
        "#,
    )
    .bind(tenant_id.get())
    .bind(result.configuration_id.get())
    .bind(result.facility_id.get())
    .bind(result.address_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if !visible {
        return Err(AppError::not_found(
            "facility shipping origin configuration",
        ));
    }
    Ok(())
}

async fn insert_origin_address_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    origin: &FacilityShippingOrigin,
) -> AppResult<AddressId> {
    let id = insert_address_tx(
        tx,
        tenant_id,
        NewAddress {
            name: origin.name(),
            company: origin.company(),
            line1: origin.line1(),
            line2: origin.line2(),
            city: Some(origin.city()),
            state: origin.state(),
            postal_code: Some(origin.postal_code()),
            country: origin.country(),
            phone: origin.phone(),
            email: origin.email(),
        },
    )
    .await?;
    AddressId::new(id).map_err(|error| AppError::internal(error.to_string()))
}

async fn update_facility_origin_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    facility_id: FacilityId,
    expected_revision: FacilityRevision,
    address_id: AddressId,
) -> AppResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE facilities
        SET address_id = $1, revision = revision + 1
        WHERE tenant_id = $2 AND id = $3 AND deleted IS NULL AND revision = $4
        "#,
    )
    .bind(address_id.get())
    .bind(tenant_id.get())
    .bind(facility_id.get())
    .bind(expected_revision.get())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "facility changed during shipping-origin configuration",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_configuration_audit_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: UserId,
    command: &ConfigureFacilityShippingOriginCommand,
    previous_address_id: Option<i64>,
    address_id: AddressId,
    revision: FacilityRevision,
    configured_at: Timestamp,
) -> AppResult<FacilityShippingOriginConfigurationId> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO facility_shipping_origin_configurations (
            tenant_id, facility_id, previous_address_id, address_id,
            configured_by_user_id, configured_at, expected_revision,
            resulting_revision
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(command.facility_id().get())
    .bind(previous_address_id)
    .bind(address_id.get())
    .bind(actor_id.get())
    .bind(configured_at)
    .bind(command.expected_revision().get())
    .bind(revision.get())
    .fetch_one(&mut **tx)
    .await?;
    FacilityShippingOriginConfigurationId::new(id)
        .map_err(|error| AppError::internal(error.to_string()))
}

async fn enqueue_configured_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    result: &ConfigureFacilityShippingOriginResult,
) -> AppResult<()> {
    let ordering_key = format!("facility:{}", result.facility_id.get());
    let aggregate_sequence = orders::next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let event_key = format!(
        "facility:{}:shipping-origin-configured:{}",
        result.facility_id.get(),
        result.revision.get()
    );
    let aggregate_id = result.facility_id.to_string();
    let payload = serde_json::json!({
        "configuration_id": result.configuration_id,
        "facility_id": result.facility_id,
        "address_id": result.address_id,
        "revision": result.revision,
        "origin": result.origin,
        "configured_by": result.configured_by,
        "configured_at": result.configured_at,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: None,
            facility_id: Some(result.facility_id),
            actor_user_id: Some(result.configured_by.get()),
            event_key: &event_key,
            aggregate_type: "facility",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence,
            event_type: "facility.shipping_origin.configured",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.configured_at,
        },
    )
    .await?;
    Ok(())
}
