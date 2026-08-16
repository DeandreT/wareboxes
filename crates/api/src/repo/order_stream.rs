//! One-transaction order allocation and waveless release.

use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::order_allocation::{
    PlanOrderAllocationCommand, PlanOrderAllocationResult,
};
use wareboxes_application::order_release::ReleaseOrderCommand;
use wareboxes_application::order_stream::{
    StreamOrderCommand, StreamOrderResult, ORDER_STREAM_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::AllocationOutcome;
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};
use crate::repo::order_release::OrderReleaseMode;
use crate::repo::orders::require_replayed_order_visible_tx;

pub async fn stream_order(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &StreamOrderCommand,
) -> AppResult<StreamOrderResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, ORDER_STREAM_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "orders").await?;

    if let Some(result) = prepared.replayed::<StreamOrderResult>(&mut tx).await? {
        require_replayed_order_visible_tx(
            &mut tx,
            access.tenant_id,
            result.release.order_id.get(),
            &scope,
        )
        .await?;
        if !scope.includes_facility(result.release.facility_id.get()) {
            return Err(AppError::not_found("order stream"));
        }
        tx.commit().await?;
        return Ok(result);
    }

    let occurred_at = now_iso();
    let allocation_command = PlanOrderAllocationCommand {
        order_id: command.order_id,
        facility_id: command.facility_id,
        expected_revision: command.expected_revision,
        expected_policy: command.expected_allocation_policy.clone(),
    };
    let allocation: PlanOrderAllocationResult =
        crate::repo::order_allocation::plan_order_allocation_tx(
            &mut tx,
            access,
            context,
            &scope,
            &allocation_command,
            occurred_at,
        )
        .await?;
    if allocation.outcome != AllocationOutcome::FullyAllocated || allocation.shortage_quantity != 0
    {
        return Err(AppError::conflict(
            "order streaming requires every demand line to allocate; no allocation was committed",
        ));
    }

    let release_command = ReleaseOrderCommand {
        order_id: command.order_id,
        facility_id: command.facility_id,
        destination_location_id: command.destination_location_id,
        expected_revision: allocation.revision,
    };
    let release = crate::repo::order_release::release_order_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        &scope,
        &release_command,
        OrderReleaseMode::Waveless,
        occurred_at,
    )
    .await?;
    let result = StreamOrderResult {
        allocation,
        release,
    };
    if !result.is_consistent() {
        return Err(AppError::internal(
            "streamed order allocation and release evidence is inconsistent",
        ));
    }
    Ok(prepared.commit(tx, result).await?)
}
