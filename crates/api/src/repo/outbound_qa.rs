//! Policy-driven outbound carton verification between packing and shipping.

mod policy;
mod session;

pub use policy::configure_policy;
pub use session::{complete, get_session, start, verify_carton};

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::outbound_qa::{
    OutboundQaCartonVerificationReadModel, OutboundQaSessionReadModel,
};
use wareboxes_application::outbox::NewOutboxEvent;
use wareboxes_domain::{
    CartonId, FacilityId, InventoryOwnerId, LicensePlateId, OrderId,
    OutboundQaCartonVerificationId, OutboundQaPolicyId, OutboundQaPolicyRevision,
    OutboundQaProgress, OutboundQaRequirement, OutboundQaScanValue, OutboundQaSessionId,
    OutboundQaSessionRevision, OutboundQaSessionStatus, PackSessionId, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::outbox;

use crate::error::{AppError, AppResult};
use crate::repo::access::ScopeBindings;
use crate::repo::orders::next_outbox_sequence_tx;

#[derive(Debug, Clone)]
pub(crate) struct ActivePolicy {
    pub policy_id: OutboundQaPolicyId,
    pub requirement: OutboundQaRequirement,
    pub revision: OutboundQaPolicyRevision,
}

pub(crate) async fn active_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    lock: bool,
) -> AppResult<Option<ActivePolicy>> {
    let suffix = if lock { " FOR SHARE" } else { "" };
    let sql = format!(
        r#"
        SELECT id, requirement, revision
        FROM outbound_qa_policies
        WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
          AND effective_to IS NULL{suffix}
        "#,
    );
    sqlx::query(&sql)
        .bind(tenant_id.get())
        .bind(owner_id.get())
        .bind(facility_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .map(policy_from_row)
        .transpose()
}

fn policy_from_row(row: sqlx::postgres::PgRow) -> AppResult<ActivePolicy> {
    let requirement: String = row.try_get("requirement")?;
    Ok(ActivePolicy {
        policy_id: positive(row.try_get("id")?, OutboundQaPolicyId::new)?,
        requirement: OutboundQaRequirement::parse(&requirement)
            .ok_or_else(|| AppError::internal("outbound QA policy has an invalid requirement"))?,
        revision: OutboundQaPolicyRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

pub(crate) async fn require_current_qa_passed_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    packing_session_id: PackSessionId,
) -> AppResult<()> {
    let Some(policy) = active_policy_tx(tx, tenant_id, owner_id, facility_id, true).await? else {
        return Ok(());
    };
    if policy.requirement == OutboundQaRequirement::NotRequired {
        return Ok(());
    }
    let passed: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM outbound_qa_sessions session
            WHERE session.tenant_id=$1 AND session.inventory_owner_id=$2
              AND session.facility_id=$3 AND session.packing_session_id=$4
              AND session.policy_id=$5 AND session.policy_revision=$6
              AND session.state='passed'
              AND session.verified_carton_count=session.expected_carton_count)
        "#,
    )
    .bind(tenant_id.get())
    .bind(owner_id.get())
    .bind(facility_id.get())
    .bind(packing_session_id.get())
    .bind(policy.policy_id.get())
    .bind(policy.revision.get())
    .fetch_one(&mut **tx)
    .await?;
    if passed {
        Ok(())
    } else {
        Err(AppError::conflict(
            "outbound QA must pass before shipment creation",
        ))
    }
}

pub(crate) async fn load_session_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    session_id: i64,
) -> AppResult<OutboundQaSessionReadModel> {
    let row = sqlx::query(
        r#"
        SELECT id, packing_session_id, order_id, inventory_owner_id, facility_id,
               policy_id, policy_revision, state, revision, expected_carton_count,
               verified_carton_count, started_by_user_id, started_at,
               passed_by_user_id, passed_at
        FROM outbound_qa_sessions WHERE tenant_id=$1 AND id=$2
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("outbound QA session"))?;
    let state: String = row.try_get("state")?;
    let verification_rows = sqlx::query(
        r#"
        SELECT id, carton_id, license_plate_id, sequence, carton_barcode,
               content_count, packed_qty, verified_by_user_id, verified_at
        FROM outbound_qa_carton_verifications
        WHERE tenant_id=$1 AND outbound_qa_session_id=$2
        ORDER BY sequence, id
        "#,
    )
    .bind(tenant_id.get())
    .bind(session_id)
    .fetch_all(&mut **tx)
    .await?;
    let verifications = verification_rows
        .into_iter()
        .map(|row| {
            Ok(OutboundQaCartonVerificationReadModel {
                verification_id: positive(row.try_get("id")?, OutboundQaCartonVerificationId::new)?,
                carton_id: positive(row.try_get("carton_id")?, CartonId::new)?,
                license_plate_id: positive(row.try_get("license_plate_id")?, LicensePlateId::new)?,
                sequence: row.try_get("sequence")?,
                carton_barcode: OutboundQaScanValue::new(
                    row.try_get::<String, _>("carton_barcode")?,
                )
                .map_err(|error| AppError::internal(error.to_string()))?,
                content_count: row.try_get("content_count")?,
                packed_quantity: row.try_get("packed_qty")?,
                verified_by: positive(row.try_get("verified_by_user_id")?, UserId::new)?,
                verified_at: row.try_get("verified_at")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let expected: i64 = row.try_get("expected_carton_count")?;
    let verified: i64 = row.try_get("verified_carton_count")?;
    if usize::try_from(verified).ok() != Some(verifications.len()) {
        return Err(AppError::internal(
            "outbound QA verification projection is inconsistent",
        ));
    }
    Ok(OutboundQaSessionReadModel {
        session_id: positive(row.try_get("id")?, OutboundQaSessionId::new)?,
        packing_session_id: positive(row.try_get("packing_session_id")?, PackSessionId::new)?,
        order_id: positive(row.try_get("order_id")?, OrderId::new)?,
        inventory_owner_id: positive(row.try_get("inventory_owner_id")?, InventoryOwnerId::new)?,
        facility_id: positive(row.try_get("facility_id")?, FacilityId::new)?,
        policy_id: positive(row.try_get("policy_id")?, OutboundQaPolicyId::new)?,
        policy_revision: OutboundQaPolicyRevision::new(row.try_get("policy_revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status: OutboundQaSessionStatus::parse(&state)
            .ok_or_else(|| AppError::internal("outbound QA session has an invalid status"))?,
        revision: OutboundQaSessionRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        progress: OutboundQaProgress::new(expected, verified)
            .map_err(|error| AppError::internal(error.to_string()))?,
        started_by: positive(row.try_get("started_by_user_id")?, UserId::new)?,
        started_at: row.try_get("started_at")?,
        passed_by: row
            .try_get::<Option<i64>, _>("passed_by_user_id")?
            .map(|id| positive(id, UserId::new))
            .transpose()?,
        passed_at: row.try_get("passed_at")?,
        verifications,
    })
}

pub(crate) fn require_scope(
    scope: &ScopeBindings,
    owner_id: i64,
    facility_id: i64,
    resource: &'static str,
) -> AppResult<()> {
    if scope.includes_inventory_owner(owner_id) && scope.includes_facility(facility_id) {
        Ok(())
    } else {
        Err(AppError::not_found(resource))
    }
}

pub(crate) async fn require_stored_visible_before_replay_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored = sqlx::query(
        r#"
        SELECT (result_json->>'policy_id')::bigint AS policy_id,
               (result_json->>'session_id')::bigint AS session_id
        FROM command_idempotency_records
        WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3
        "#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(stored) = stored else {
        return Ok(());
    };
    if let Some(session_id) = stored.try_get::<Option<i64>, _>("session_id")? {
        let row = sqlx::query(
            "SELECT inventory_owner_id,facility_id FROM outbound_qa_sessions WHERE tenant_id=$1 AND id=$2",
        )
        .bind(prepared.tenant_id().get())
        .bind(session_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("outbound QA session"))?;
        return require_scope(
            scope,
            row.try_get("inventory_owner_id")?,
            row.try_get("facility_id")?,
            "outbound QA session",
        );
    }
    let policy_id: i64 = stored
        .try_get::<Option<i64>, _>("policy_id")?
        .ok_or_else(|| AppError::internal("stored outbound QA result is invalid"))?;
    let row = sqlx::query(
        "SELECT inventory_owner_id,facility_id FROM outbound_qa_policies WHERE tenant_id=$1 AND id=$2",
    )
    .bind(prepared.tenant_id().get())
    .bind(policy_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("outbound QA policy"))?;
    require_scope(
        scope,
        row.try_get("inventory_owner_id")?,
        row.try_get("facility_id")?,
        "outbound QA policy",
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    actor_id: i64,
    ordering_key: &str,
    aggregate_type: &str,
    aggregate_id: i64,
    event_type: &str,
    event_suffix: &str,
    payload: &serde_json::Value,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let sequence = next_outbox_sequence_tx(tx, tenant_id, ordering_key).await?;
    let event_key = format!("{ordering_key}:{event_suffix}");
    let aggregate_id = aggregate_id.to_string();
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: Some(owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(actor_id),
            event_key: &event_key,
            aggregate_type,
            aggregate_id: &aggregate_id,
            ordering_key,
            aggregate_sequence: sequence,
            event_type,
            schema_version: 1,
            payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

fn positive<T>(
    value: i64,
    constructor: impl FnOnce(i64) -> Result<T, wareboxes_domain::InvalidId>,
) -> AppResult<T> {
    constructor(value).map_err(|error| AppError::internal(error.to_string()))
}
