//! Customer-return authorization and return-bound inbound-load planning.

mod cancellation;
mod read_model;

pub use cancellation::cancel;
pub use read_model::{detail, page};

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::customer_return::{
    CreateCustomerReturnCommand, CreateCustomerReturnResult, CreatedCustomerReturnLineResult,
    PlanCustomerReturnLoadCommand, PlanCustomerReturnLoadResult,
    PlannedCustomerReturnLoadLineResult, CREATE_CUSTOMER_RETURN_OPERATION,
    PLAN_CUSTOMER_RETURN_LOAD_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    plan_customer_return, CatalogItemId, CustomerReturnId, CustomerReturnLineId,
    CustomerReturnLoadPlanId, CustomerReturnRevision, CustomerReturnStatus, FacilityId,
    InboundAsnId, InboundAsnLineId, InboundAsnLoadPlanId, InboundLoadId, InboundLoadLineId,
    InventoryOwnerId, Timestamp,
};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::{insert_result, PostgresPreparedCommandExt};
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use super::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::error::{AppError, AppResult};

pub async fn create(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreateCustomerReturnCommand,
) -> AppResult<CreateCustomerReturnResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CREATE_CUSTOMER_RETURN_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<CreateCustomerReturnResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let authorization = &command.authorization;
    if !scope.includes_inventory_owner(authorization.inventory_owner_id().get())
        || !scope.includes_facility(authorization.facility_id().get())
    {
        return Err(AppError::forbidden());
    }
    lock_return_identity(
        &mut tx,
        access,
        authorization.inventory_owner_id().get(),
        authorization.number().as_str(),
    )
    .await?;
    lock_return_scope(
        &mut tx,
        access,
        authorization.inventory_owner_id().get(),
        authorization.facility_id().get(),
    )
    .await?;
    let item_uoms = lock_return_items(&mut tx, access, command).await?;
    let line_count = i64::try_from(authorization.lines().len())
        .map_err(|_| AppError::bad_request("return line count exceeds i64"))?;
    let total_authorized_quantity =
        authorization
            .lines()
            .iter()
            .try_fold(0_i64, |total, line| {
                total
                    .checked_add(line.authorized_quantity().get())
                    .ok_or_else(|| AppError::bad_request("return quantity exceeds i64"))
            })?;
    let created_at = now_iso();
    let inbound_asn_id = InboundAsnId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO inbound_asns
                (tenant_id,inventory_owner_id,facility_id,number,supplier,expected_at,
                 status,revision,line_count,total_expected_quantity,created_by_user_id,created_at)
            VALUES ($1,$2,$3,$4,'Customer return',$5,'open',1,$6,$7,$8,$9)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(authorization.inventory_owner_id().get())
        .bind(authorization.facility_id().get())
        .bind(authorization.number().as_str())
        .bind(authorization.expected_at())
        .bind(line_count)
        .bind(total_authorized_quantity)
        .bind(context.actor_id.get())
        .bind(created_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let customer_return_id = CustomerReturnId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO customer_returns
                (tenant_id,inventory_owner_id,facility_id,inbound_asn_id,customer_reference)
            VALUES ($1,$2,$3,$4,$5)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(authorization.inventory_owner_id().get())
        .bind(authorization.facility_id().get())
        .bind(inbound_asn_id.get())
        .bind(authorization.customer_reference().as_str())
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;

    let mut lines = Vec::with_capacity(authorization.lines().len());
    for (index, line) in authorization.lines().iter().enumerate() {
        let sequence = i64::try_from(index + 1)
            .map_err(|_| AppError::bad_request("return line sequence exceeds i64"))?;
        let uom = item_uoms.get(&line.item_id().get()).ok_or_else(|| {
            AppError::conflict("return item is no longer available to this client")
        })?;
        let inbound_asn_line_id = InboundAsnLineId::new(
            sqlx::query_scalar(
                r#"
                INSERT INTO inbound_asn_lines
                    (tenant_id,inventory_owner_id,facility_id,asn_id,sequence,item_id,uom,
                     expected_quantity,lot,serial,expiration)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,NULL)
                RETURNING id
                "#,
            )
            .bind(access.tenant_id.get())
            .bind(authorization.inventory_owner_id().get())
            .bind(authorization.facility_id().get())
            .bind(inbound_asn_id.get())
            .bind(sequence)
            .bind(line.item_id().get())
            .bind(uom)
            .bind(line.authorized_quantity().get())
            .bind(line.lot())
            .bind(line.serial())
            .fetch_one(&mut *tx)
            .await?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?;
        let line_id = CustomerReturnLineId::new(
            sqlx::query_scalar(
                r#"
                INSERT INTO customer_return_lines
                    (tenant_id,inventory_owner_id,facility_id,customer_return_id,
                     inbound_asn_id,inbound_asn_line_id,sequence,reason_code,note)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
                RETURNING id
                "#,
            )
            .bind(access.tenant_id.get())
            .bind(authorization.inventory_owner_id().get())
            .bind(authorization.facility_id().get())
            .bind(customer_return_id.get())
            .bind(inbound_asn_id.get())
            .bind(inbound_asn_line_id.get())
            .bind(sequence)
            .bind(line.reason().as_str())
            .bind(line.note())
            .fetch_one(&mut *tx)
            .await?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?;
        lines.push(CreatedCustomerReturnLineResult {
            line_id,
            item_id: line.item_id(),
            authorized_quantity: line.authorized_quantity().get(),
            reason: line.reason(),
        });
    }

    let result = CreateCustomerReturnResult {
        customer_return_id,
        number: authorization.number().as_str().to_owned(),
        status: CustomerReturnStatus::Open,
        revision: return_revision(1)?,
        lines,
        total_authorized_quantity,
        created_by: context.actor_id,
        created_at,
    };
    let payload = serde_json::json!({
        "customer_return_id": result.customer_return_id.get(),
        "number": result.number,
        "customer_reference": authorization.customer_reference().as_str(),
        "status": result.status.as_str(),
        "revision": result.revision.get(),
        "line_count": result.lines.len(),
        "total_authorized_quantity": result.total_authorized_quantity,
        "created_by": result.created_by.get(),
        "created_at": result.created_at,
    });
    enqueue_return_event(
        &mut tx,
        access,
        context,
        authorization.inventory_owner_id(),
        authorization.facility_id(),
        customer_return_id,
        1,
        "inbound.customer_return.created",
        &payload,
        created_at,
    )
    .await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn plan_load(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &PlanCustomerReturnLoadCommand,
) -> AppResult<PlanCustomerReturnLoadResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, PLAN_CUSTOMER_RETURN_LOAD_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<PlanCustomerReturnLoadResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let header = sqlx::query(
        r#"
        SELECT customer_return.inbound_asn_id,asn.inventory_owner_id,asn.facility_id,
               asn.number,asn.expected_at,asn.status,asn.revision,asn.line_count,
               asn.total_expected_quantity
        FROM customer_returns customer_return
        INNER JOIN inbound_asns asn
          ON asn.tenant_id=customer_return.tenant_id
         AND asn.id=customer_return.inbound_asn_id
        WHERE customer_return.tenant_id=$1 AND customer_return.id=$2
          AND ($3 OR customer_return.facility_id=ANY($4))
          AND ($5 OR customer_return.inventory_owner_id=ANY($6))
        FOR UPDATE OF asn
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.customer_return_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("customer return"))?;
    let inventory_owner_id: i64 = header.try_get("inventory_owner_id")?;
    let facility_id: i64 = header.try_get("facility_id")?;
    let inbound_asn_id: i64 = header.try_get("inbound_asn_id")?;
    let status = return_status(header.try_get::<String, _>("status")?.as_str())?;
    let revision = return_revision(header.try_get("revision")?)?;
    if revision != command.expected_revision {
        return Err(AppError::conflict(
            "customer return changed; refresh before planning the load",
        ));
    }
    let resulting_revision = plan_customer_return(status, revision)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    require_receiving_location(
        &mut tx,
        access,
        facility_id,
        command.details.receiving_location_id().get(),
    )
    .await?;
    let source_lines = sqlx::query(
        r#"
        SELECT return_line.id AS return_line_id,source.id AS asn_line_id,
               source.sequence,source.item_id,source.uom,source.expected_quantity,
               source.lot,source.serial,source.expiration
        FROM customer_return_lines return_line
        INNER JOIN inbound_asn_lines source
          ON source.tenant_id=return_line.tenant_id
         AND source.id=return_line.inbound_asn_line_id
        INNER JOIN items item
          ON item.tenant_id=source.tenant_id AND item.id=source.item_id AND item.deleted IS NULL
        INNER JOIN inventory_owner_items owner_item
          ON owner_item.tenant_id=source.tenant_id
         AND owner_item.inventory_owner_id=source.inventory_owner_id
         AND owner_item.item_id=source.item_id AND owner_item.deleted IS NULL
        WHERE return_line.tenant_id=$1 AND return_line.customer_return_id=$2
        ORDER BY return_line.sequence,return_line.id
        FOR SHARE OF item,owner_item
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.customer_return_id.get())
    .fetch_all(&mut *tx)
    .await?;
    if i64::try_from(source_lines.len()).ok() != Some(header.try_get("line_count")?) {
        return Err(AppError::conflict(
            "customer return line set is no longer executable",
        ));
    }

    let planned_at = now_iso();
    let execution_barcode = super::loads::generated_execution_barcode();
    let number: String = header.try_get("number")?;
    let load_id = InboundLoadId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO loads
                (tenant_id,created,facility_id,inventory_owner_id,execution_barcode,status,type,
                 reference_number,carrier,trailer_number,seal_number,dock_door_location_id,
                 expected_time,receive_completed)
            VALUES ($1,$2,$3,$4,$5,'planned','inbound',$6,$7,$8,$9,$10,$11,false)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(planned_at)
        .bind(facility_id)
        .bind(inventory_owner_id)
        .bind(&execution_barcode)
        .bind(&number)
        .bind(command.details.carrier())
        .bind(command.details.trailer_number())
        .bind(command.details.seal_number())
        .bind(command.details.receiving_location_id().get())
        .bind(header.try_get::<Option<Timestamp>, _>("expected_at")?)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let line_count: i64 = header.try_get("line_count")?;
    let total_authorized_quantity: i64 = header.try_get("total_expected_quantity")?;
    let asn_plan_id = InboundAsnLoadPlanId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO inbound_asn_load_plans
                (tenant_id,inventory_owner_id,facility_id,asn_id,load_id,receiving_location_id,
                 expected_asn_revision,resulting_asn_revision,line_count,total_expected_quantity,
                 planned_by_user_id,planned_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(inbound_asn_id)
        .bind(load_id.get())
        .bind(command.details.receiving_location_id().get())
        .bind(revision.get())
        .bind(resulting_revision.get())
        .bind(line_count)
        .bind(total_authorized_quantity)
        .bind(context.actor_id.get())
        .bind(planned_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let mut lines = Vec::with_capacity(source_lines.len());
    for source in source_lines {
        let return_line_id = CustomerReturnLineId::new(source.try_get("return_line_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?;
        let asn_line_id = InboundAsnLineId::new(source.try_get("asn_line_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?;
        let item_id = CatalogItemId::new(source.try_get("item_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?;
        let quantity: i64 = source.try_get("expected_quantity")?;
        let load_line_id = InboundLoadLineId::new(
            sqlx::query_scalar(
                r#"
                INSERT INTO load_lines
                    (tenant_id,created,load_id,item_id,expected_qty,lot,serial,expiration,status)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'pending') RETURNING id
                "#,
            )
            .bind(access.tenant_id.get())
            .bind(planned_at)
            .bind(load_id.get())
            .bind(item_id.get())
            .bind(quantity)
            .bind(source.try_get::<Option<String>, _>("lot")?)
            .bind(source.try_get::<Option<String>, _>("serial")?)
            .bind(source.try_get::<Option<Timestamp>, _>("expiration")?)
            .fetch_one(&mut *tx)
            .await?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?;
        sqlx::query(
            r#"
            INSERT INTO inbound_asn_load_plan_lines
                (tenant_id,inventory_owner_id,facility_id,asn_id,plan_id,load_id,
                 asn_line_id,load_line_id,sequence,item_id,expected_quantity,lot,serial,expiration)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(inbound_asn_id)
        .bind(asn_plan_id.get())
        .bind(load_id.get())
        .bind(asn_line_id.get())
        .bind(load_line_id.get())
        .bind(source.try_get::<i64, _>("sequence")?)
        .bind(item_id.get())
        .bind(quantity)
        .bind(source.try_get::<Option<String>, _>("lot")?)
        .bind(source.try_get::<Option<String>, _>("serial")?)
        .bind(source.try_get::<Option<Timestamp>, _>("expiration")?)
        .execute(&mut *tx)
        .await?;
        lines.push(PlannedCustomerReturnLoadLineResult {
            customer_return_line_id: return_line_id,
            load_line_id,
            item_id,
            authorized_quantity: quantity,
        });
    }
    let updated = sqlx::query(
        r#"
        UPDATE inbound_asns
        SET status='planned',revision=$1,load_id=$2,planned_by_user_id=$3,planned_at=$4
        WHERE tenant_id=$5 AND id=$6 AND status='open' AND revision=$7 AND load_id IS NULL
        "#,
    )
    .bind(resulting_revision.get())
    .bind(load_id.get())
    .bind(context.actor_id.get())
    .bind(planned_at)
    .bind(access.tenant_id.get())
    .bind(inbound_asn_id)
    .bind(revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "customer return changed while planning its load",
        ));
    }
    let plan_id = CustomerReturnLoadPlanId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO customer_return_load_plans
                (tenant_id,inventory_owner_id,facility_id,customer_return_id,inbound_asn_id,
                 inbound_asn_load_plan_id,load_id,receiving_location_id,
                 expected_return_revision,resulting_return_revision,planned_by_user_id,planned_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(command.customer_return_id.get())
        .bind(inbound_asn_id)
        .bind(asn_plan_id.get())
        .bind(load_id.get())
        .bind(command.details.receiving_location_id().get())
        .bind(revision.get())
        .bind(resulting_revision.get())
        .bind(context.actor_id.get())
        .bind(planned_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    sqlx::query(
        r#"
        INSERT INTO load_activity
            (tenant_id,created,load_id,user_id,action,message,metadata_json)
        VALUES ($1,$2,$3,$4,'planned','return load planned from customer authorization',$5)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(planned_at)
    .bind(load_id.get())
    .bind(context.actor_id.get())
    .bind(
        serde_json::json!({
            "customer_return_id": command.customer_return_id.get(),
            "plan_id": plan_id.get(),
            "return_number": number,
            "line_count": line_count,
            "total_authorized_quantity": total_authorized_quantity,
            "receipt_policy": "quarantine_only"
        })
        .to_string(),
    )
    .execute(&mut *tx)
    .await?;
    let result = PlanCustomerReturnLoadResult {
        plan_id,
        customer_return_id: command.customer_return_id,
        status: CustomerReturnStatus::Planned,
        revision: resulting_revision,
        load_id,
        execution_barcode,
        lines,
        total_authorized_quantity,
        planned_by: context.actor_id,
        planned_at,
    };
    let payload = serde_json::json!({
        "plan_id": result.plan_id.get(),
        "customer_return_id": result.customer_return_id.get(),
        "number": number,
        "status": result.status.as_str(),
        "revision": result.revision.get(),
        "load_id": result.load_id.get(),
        "execution_barcode": result.execution_barcode,
        "line_count": result.lines.len(),
        "total_authorized_quantity": result.total_authorized_quantity,
        "receipt_policy": "quarantine_only",
        "planned_by": result.planned_by.get(),
        "planned_at": result.planned_at,
    });
    enqueue_return_event(
        &mut tx,
        access,
        context,
        InventoryOwnerId::new(inventory_owner_id)
            .map_err(|error| AppError::internal(error.to_string()))?,
        FacilityId::new(facility_id).map_err(|error| AppError::internal(error.to_string()))?,
        command.customer_return_id,
        resulting_revision.get(),
        "inbound.customer_return.load_planned",
        &payload,
        planned_at,
    )
    .await?;
    enqueue_load_event(
        &mut tx,
        access,
        context,
        inventory_owner_id,
        facility_id,
        load_id,
        &payload,
        planned_at,
    )
    .await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

async fn lock_return_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    inventory_owner_id: i64,
    number: &str,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "customer-return:{}:{inventory_owner_id}:{}",
            access.tenant_id.get(),
            number.to_uppercase()
        ))
        .execute(&mut **tx)
        .await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM inbound_asns WHERE tenant_id=$1 AND inventory_owner_id=$2 AND number=$3)",
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id)
    .bind(number)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Err(AppError::conflict(
            "return number already exists for this client",
        ))
    } else {
        Ok(())
    }
}

async fn lock_return_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    inventory_owner_id: i64,
    facility_id: i64,
) -> AppResult<()> {
    let owner: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM inventory_owners WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL FOR SHARE",
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id)
    .fetch_optional(&mut **tx)
    .await?;
    let facility: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM facilities WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL FOR SHARE",
    )
    .bind(access.tenant_id.get())
    .bind(facility_id)
    .fetch_optional(&mut **tx)
    .await?;
    if owner.is_none() || facility.is_none() {
        Err(AppError::not_found("return client or facility"))
    } else {
        Ok(())
    }
}

async fn lock_return_items(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    command: &CreateCustomerReturnCommand,
) -> AppResult<HashMap<i64, String>> {
    let item_ids = command
        .authorization
        .lines()
        .iter()
        .map(|line| line.item_id().get())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT item.id,item.packaging_unit
        FROM inventory_owner_items owner_item
        INNER JOIN items item
          ON item.tenant_id=owner_item.tenant_id AND item.id=owner_item.item_id
        WHERE owner_item.tenant_id=$1 AND owner_item.inventory_owner_id=$2
          AND owner_item.item_id=ANY($3) AND owner_item.deleted IS NULL AND item.deleted IS NULL
        ORDER BY item.id FOR SHARE OF owner_item,item
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.authorization.inventory_owner_id().get())
    .bind(&item_ids)
    .fetch_all(&mut **tx)
    .await?;
    let result = rows
        .iter()
        .map(|row| Ok((row.try_get("id")?, row.try_get("packaging_unit")?)))
        .collect::<AppResult<HashMap<_, _>>>()?;
    if item_ids.iter().all(|item_id| result.contains_key(item_id)) {
        Ok(result)
    } else {
        Err(AppError::conflict(
            "every return item must remain active and linked to the client",
        ))
    }
}

async fn require_receiving_location(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    facility_id: i64,
    location_id: i64,
) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(SELECT 1 FROM locations
        WHERE tenant_id=$1 AND facility_id=$2 AND id=$3
          AND deleted IS NULL AND active AND receivable
          AND NULLIF(BTRIM(barcode),'') IS NOT NULL)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(facility_id)
    .bind(location_id)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::conflict(
            "receiving location is not executable for this return",
        ))
    }
}

pub(super) async fn require_stored_visible_before_replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let stored_return_id: Option<i64> = sqlx::query_scalar(
        "SELECT (result_json->>'customer_return_id')::BIGINT FROM command_idempotency_records WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3",
    )
    .bind(access.tenant_id.get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(customer_return_id) = stored_return_id else {
        return Ok(());
    };
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(SELECT 1 FROM customer_returns
        WHERE tenant_id=$1 AND id=$2
          AND ($3 OR facility_id=ANY($4))
          AND ($5 OR inventory_owner_id=ANY($6)))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(customer_return_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("customer return"))
    }
}

pub(super) fn return_status(value: &str) -> AppResult<CustomerReturnStatus> {
    CustomerReturnStatus::parse(value)
        .ok_or_else(|| AppError::internal("stored customer return status is invalid"))
}

pub(super) fn return_revision(value: i64) -> AppResult<CustomerReturnRevision> {
    CustomerReturnRevision::new(value).map_err(|error| AppError::internal(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn enqueue_return_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    owner: InventoryOwnerId,
    facility: FacilityId,
    customer_return_id: CustomerReturnId,
    sequence: i64,
    event_type: &'static str,
    payload: &serde_json::Value,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let event_suffix = event_type.rsplit('.').next().unwrap_or("updated");
    let event_key = format!(
        "customer-return:{}:{event_suffix}",
        customer_return_id.get()
    );
    let aggregate_id = customer_return_id.to_string();
    let ordering_key = format!("customer-return:{}", customer_return_id.get());
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(owner),
            facility_id: Some(facility),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "customer_return",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
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

#[allow(clippy::too_many_arguments)]
async fn enqueue_load_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: i64,
    facility_id: i64,
    load_id: InboundLoadId,
    payload: &serde_json::Value,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let event_key = format!("inbound-load:{}:planned-from-return", load_id.get());
    let aggregate_id = load_id.to_string();
    let ordering_key = format!("inbound-load:{}", load_id.get());
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(
                InventoryOwnerId::new(inventory_owner_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            facility_id: Some(
                FacilityId::new(facility_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "inbound_load",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: 1,
            event_type: "inbound.load.planned",
            schema_version: 1,
            payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}
