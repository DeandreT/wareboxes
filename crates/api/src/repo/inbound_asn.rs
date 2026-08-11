//! Advance shipping notice source intake and source-bound load planning.

use std::collections::HashMap;

use sqlx::Row;
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::inbound_asn::{
    CreateInboundAsnCommand, CreateInboundAsnResult, CreatedInboundAsnLineResult,
    InboundAsnLineReadModel, InboundAsnPage, InboundAsnPageFilter, InboundAsnReadModel,
    PlanInboundAsnLoadCommand, PlanInboundAsnLoadResult, PlannedInboundAsnLoadLineResult,
    CREATE_INBOUND_ASN_OPERATION, PLAN_INBOUND_ASN_LOAD_OPERATION,
};
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    plan_inbound_asn, CatalogItemId, FacilityId, InboundAsnId, InboundAsnLineId,
    InboundAsnLoadPlanId, InboundAsnRevision, InboundAsnStatus, InboundLoadId, InboundLoadLineId,
    InventoryOwnerId, PurchaseOrderAsnSourceId, PurchaseOrderId, Timestamp, UserId,
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
    command: &CreateInboundAsnCommand,
) -> AppResult<CreateInboundAsnResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CREATE_INBOUND_ASN_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed::<CreateInboundAsnResult>(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }

    let notice = &command.notice;
    if !scope.includes_inventory_owner(notice.inventory_owner_id().get())
        || !scope.includes_facility(notice.facility_id().get())
    {
        return Err(AppError::forbidden());
    }
    lock_source_identity(
        &mut tx,
        access,
        notice.inventory_owner_id().get(),
        notice.number().as_str(),
    )
    .await?;
    lock_source_scope(
        &mut tx,
        access,
        notice.inventory_owner_id().get(),
        notice.facility_id().get(),
    )
    .await?;
    let item_uoms = lock_source_items(&mut tx, access, command).await?;
    let line_count = i64::try_from(notice.lines().len())
        .map_err(|_| AppError::bad_request("ASN line count exceeds i64"))?;
    let total_expected_quantity = notice.lines().iter().try_fold(0_i64, |total, line| {
        total
            .checked_add(line.expected_quantity().get())
            .ok_or_else(|| AppError::bad_request("ASN expected quantity exceeds i64"))
    })?;
    let created_at = now_iso();
    let asn_id = InboundAsnId::new(
        sqlx::query_scalar(
            r#"
            INSERT INTO inbound_asns
                (tenant_id,inventory_owner_id,facility_id,number,supplier,expected_at,
                 status,revision,line_count,total_expected_quantity,created_by_user_id,created_at)
            VALUES ($1,$2,$3,$4,$5,$6,'open',1,$7,$8,$9,$10)
            RETURNING id
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(notice.inventory_owner_id().get())
        .bind(notice.facility_id().get())
        .bind(notice.number().as_str())
        .bind(notice.supplier().as_str())
        .bind(notice.expected_at())
        .bind(line_count)
        .bind(total_expected_quantity)
        .bind(context.actor_id.get())
        .bind(created_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let mut lines = Vec::with_capacity(notice.lines().len());
    for (index, line) in notice.lines().iter().enumerate() {
        let sequence = i64::try_from(index + 1)
            .map_err(|_| AppError::bad_request("ASN line sequence exceeds i64"))?;
        let uom = item_uoms
            .get(&line.item_id().get())
            .ok_or_else(|| AppError::conflict("ASN item is no longer available to this client"))?;
        let line_id = InboundAsnLineId::new(
            sqlx::query_scalar(
                r#"
                INSERT INTO inbound_asn_lines
                    (tenant_id,inventory_owner_id,facility_id,asn_id,sequence,item_id,uom,
                     expected_quantity,lot,serial,expiration)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
                RETURNING id
                "#,
            )
            .bind(access.tenant_id.get())
            .bind(notice.inventory_owner_id().get())
            .bind(notice.facility_id().get())
            .bind(asn_id.get())
            .bind(sequence)
            .bind(line.item_id().get())
            .bind(uom)
            .bind(line.expected_quantity().get())
            .bind(line.lot())
            .bind(line.serial())
            .bind(line.expiration())
            .fetch_one(&mut *tx)
            .await?,
        )
        .map_err(|error| AppError::internal(error.to_string()))?;
        lines.push(CreatedInboundAsnLineResult {
            line_id,
            item_id: line.item_id(),
            expected_quantity: line.expected_quantity().get(),
        });
    }
    let result = CreateInboundAsnResult {
        asn_id,
        number: notice.number().as_str().to_owned(),
        status: InboundAsnStatus::Open,
        revision: InboundAsnRevision::new(1)
            .map_err(|error| AppError::internal(error.to_string()))?,
        lines,
        total_expected_quantity,
        created_by: context.actor_id,
        created_at,
    };
    enqueue_event(
        &mut tx,
        access,
        context,
        notice.inventory_owner_id(),
        notice.facility_id(),
        &result,
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
    command: &PlanInboundAsnLoadCommand,
) -> AppResult<PlanInboundAsnLoadResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, PLAN_INBOUND_ASN_LOAD_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "wms").await?;
    require_stored_visible_before_replay(&mut tx, access, &prepared, &scope).await?;
    if let Some(result) = prepared
        .replayed::<PlanInboundAsnLoadResult>(&mut tx)
        .await?
    {
        tx.commit().await?;
        return Ok(result);
    }

    let header = sqlx::query(
        r#"
        SELECT inventory_owner_id,facility_id,number,supplier,expected_at,status,revision,
               line_count,total_expected_quantity
        FROM inbound_asns
        WHERE tenant_id=$1 AND id=$2
          AND ($3 OR facility_id=ANY($4))
          AND ($5 OR inventory_owner_id=ANY($6))
        FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.asn_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("advance shipping notice"))?;
    let inventory_owner_id: i64 = header.try_get("inventory_owner_id")?;
    let facility_id: i64 = header.try_get("facility_id")?;
    let status = parse_status(header.try_get::<String, _>("status")?.as_str())?;
    let revision = revision(header.try_get("revision")?)?;
    if revision != command.expected_revision {
        return Err(AppError::conflict(
            "advance shipping notice changed; refresh before planning the load",
        ));
    }
    let resulting_revision = plan_inbound_asn(status, revision)
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
        SELECT line.id,line.sequence,line.item_id,line.uom,line.expected_quantity,
               line.lot,line.serial,line.expiration
        FROM inbound_asn_lines line
        INNER JOIN items item
          ON item.tenant_id=line.tenant_id AND item.id=line.item_id AND item.deleted IS NULL
        INNER JOIN inventory_owner_items owner_item
          ON owner_item.tenant_id=line.tenant_id
         AND owner_item.inventory_owner_id=line.inventory_owner_id
         AND owner_item.item_id=line.item_id AND owner_item.deleted IS NULL
        WHERE line.tenant_id=$1 AND line.asn_id=$2
        ORDER BY line.sequence,line.id
        FOR SHARE OF item,owner_item
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.asn_id.get())
    .fetch_all(&mut *tx)
    .await?;
    let expected_line_count: i64 = header.try_get("line_count")?;
    if i64::try_from(source_lines.len()).ok() != Some(expected_line_count) {
        return Err(AppError::conflict(
            "advance shipping notice line set is no longer executable",
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
    let total_expected_quantity: i64 = header.try_get("total_expected_quantity")?;
    let plan_id = InboundAsnLoadPlanId::new(
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
        .bind(command.asn_id.get())
        .bind(load_id.get())
        .bind(command.details.receiving_location_id().get())
        .bind(revision.get())
        .bind(resulting_revision.get())
        .bind(line_count)
        .bind(total_expected_quantity)
        .bind(context.actor_id.get())
        .bind(planned_at)
        .fetch_one(&mut *tx)
        .await?,
    )
    .map_err(|error| AppError::internal(error.to_string()))?;
    let mut lines = Vec::with_capacity(source_lines.len());
    for source in source_lines {
        let asn_line_id = InboundAsnLineId::new(source.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?;
        let item_id = CatalogItemId::new(source.try_get("item_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?;
        let expected_quantity: i64 = source.try_get("expected_quantity")?;
        let load_line_id = InboundLoadLineId::new(
            sqlx::query_scalar(
                r#"
                INSERT INTO load_lines
                    (tenant_id,created,load_id,item_id,expected_qty,lot,serial,expiration,status)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'pending')
                RETURNING id
                "#,
            )
            .bind(access.tenant_id.get())
            .bind(planned_at)
            .bind(load_id.get())
            .bind(item_id.get())
            .bind(expected_quantity)
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
            SELECT $1,$2,$3,$4,$5,$6,$7,$8,sequence,item_id,expected_quantity,lot,serial,expiration
            FROM inbound_asn_lines
            WHERE tenant_id=$1 AND asn_id=$4 AND id=$7
            "#,
        )
        .bind(access.tenant_id.get())
        .bind(inventory_owner_id)
        .bind(facility_id)
        .bind(command.asn_id.get())
        .bind(plan_id.get())
        .bind(load_id.get())
        .bind(asn_line_id.get())
        .bind(load_line_id.get())
        .execute(&mut *tx)
        .await?;
        lines.push(PlannedInboundAsnLoadLineResult {
            asn_line_id,
            load_line_id,
            item_id,
            expected_quantity,
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
    .bind(command.asn_id.get())
    .bind(revision.get())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::conflict(
            "advance shipping notice changed while planning its load",
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO load_activity
            (tenant_id,created,load_id,user_id,action,message,metadata_json)
        VALUES ($1,$2,$3,$4,'planned','inbound load planned from ASN source',$5)
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(planned_at)
    .bind(load_id.get())
    .bind(context.actor_id.get())
    .bind(
        serde_json::json!({
            "asn_id": command.asn_id.get(),
            "plan_id": plan_id.get(),
            "asn_number": number,
            "supplier": header.try_get::<String, _>("supplier")?,
            "line_count": line_count,
            "total_expected_quantity": total_expected_quantity,
        })
        .to_string(),
    )
    .execute(&mut *tx)
    .await?;
    let result = PlanInboundAsnLoadResult {
        plan_id,
        asn_id: command.asn_id,
        asn_status: InboundAsnStatus::Planned,
        asn_revision: resulting_revision,
        load_id,
        execution_barcode,
        lines,
        total_expected_quantity,
        planned_by: context.actor_id,
        planned_at,
    };
    enqueue_planned_events(
        &mut tx,
        access,
        context,
        inventory_owner_id,
        facility_id,
        &number,
        &result,
    )
    .await?;
    insert_result(&mut tx, &prepared.completed_result(&result, None)?).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn page(
    db: &Db,
    access: &TenantAccess,
    filter: &InboundAsnPageFilter,
) -> AppResult<InboundAsnPage> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let limit = i64::from(filter.limit);
    let offset = i64::try_from(filter.offset)
        .map_err(|_| AppError::bad_request("ASN page offset exceeds i64"))?;
    let status = filter.status.map(InboundAsnStatus::as_str);
    let search = filter.search.as_deref();
    let rows = sqlx::query(
        r#"
        SELECT asn.id,asn.inventory_owner_id,owner.name AS inventory_owner_name,
               asn.facility_id,facility.name AS facility_name,asn.number,asn.supplier,
               asn.expected_at,asn.status,asn.revision,asn.line_count,
               asn.total_expected_quantity,asn.load_id,asn.created_by_user_id,asn.created_at,
               asn.planned_by_user_id,asn.planned_at,
               po_source.id AS purchase_order_source_id,
               po_source.purchase_order_id,purchase.number AS purchase_order_number
        FROM inbound_asns asn
        INNER JOIN inventory_owners owner
          ON owner.tenant_id=asn.tenant_id AND owner.id=asn.inventory_owner_id
        INNER JOIN facilities facility
          ON facility.tenant_id=asn.tenant_id AND facility.id=asn.facility_id
        LEFT JOIN purchase_order_asn_sources po_source
          ON po_source.tenant_id=asn.tenant_id AND po_source.asn_id=asn.id
        LEFT JOIN purchase_orders purchase
          ON purchase.tenant_id=po_source.tenant_id
         AND purchase.id=po_source.purchase_order_id
        WHERE asn.tenant_id=$1
          AND ($2 OR asn.facility_id=ANY($3))
          AND ($4 OR asn.inventory_owner_id=ANY($5))
          AND ($6::BIGINT IS NULL OR asn.facility_id=$6)
          AND ($7::BIGINT IS NULL OR asn.inventory_owner_id=$7)
          AND ($8::TEXT IS NULL OR asn.status=$8)
          AND ($9::TEXT IS NULL OR asn.number ILIKE '%' || $9 || '%'
               OR asn.supplier ILIKE '%' || $9 || '%'
               OR purchase.number ILIKE '%' || $9 || '%')
        ORDER BY asn.created_at DESC,asn.id DESC
        OFFSET $10 LIMIT $11+1
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.facility_id.map(FacilityId::get))
    .bind(filter.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(status)
    .bind(search)
    .bind(offset)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > usize::from(filter.limit);
    let entries = rows
        .iter()
        .take(usize::from(filter.limit))
        .map(map_header)
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(InboundAsnPage {
        entries,
        next_offset: has_more.then(|| filter.offset + u64::from(filter.limit)),
    })
}

pub async fn detail(
    db: &Db,
    access: &TenantAccess,
    asn_id: InboundAsnId,
) -> AppResult<Option<InboundAsnReadModel>> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let header = sqlx::query(
        r#"
        SELECT asn.id,asn.inventory_owner_id,owner.name AS inventory_owner_name,
               asn.facility_id,facility.name AS facility_name,asn.number,asn.supplier,
               asn.expected_at,asn.status,asn.revision,asn.line_count,
               asn.total_expected_quantity,asn.load_id,asn.created_by_user_id,asn.created_at,
               asn.planned_by_user_id,asn.planned_at,
               po_source.id AS purchase_order_source_id,
               po_source.purchase_order_id,purchase.number AS purchase_order_number
        FROM inbound_asns asn
        INNER JOIN inventory_owners owner
          ON owner.tenant_id=asn.tenant_id AND owner.id=asn.inventory_owner_id
        INNER JOIN facilities facility
          ON facility.tenant_id=asn.tenant_id AND facility.id=asn.facility_id
        LEFT JOIN purchase_order_asn_sources po_source
          ON po_source.tenant_id=asn.tenant_id AND po_source.asn_id=asn.id
        LEFT JOIN purchase_orders purchase
          ON purchase.tenant_id=po_source.tenant_id
         AND purchase.id=po_source.purchase_order_id
        WHERE asn.tenant_id=$1 AND asn.id=$2
          AND ($3 OR asn.facility_id=ANY($4))
          AND ($5 OR asn.inventory_owner_id=ANY($6))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(asn_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(header) = header else {
        tx.commit().await?;
        return Ok(None);
    };
    let mut result = map_header(&header)?;
    let rows = sqlx::query(
        r#"
        SELECT line.id,line.sequence,line.item_id,COALESCE(item.description,'Item #' || item.id) AS item_description,
               line.uom,line.expected_quantity,line.lot,line.serial,line.expiration
        FROM inbound_asn_lines line
        INNER JOIN items item ON item.tenant_id=line.tenant_id AND item.id=line.item_id
        WHERE line.tenant_id=$1 AND line.asn_id=$2
        ORDER BY line.sequence,line.id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(asn_id.get())
    .fetch_all(&mut *tx)
    .await?;
    result.lines = rows
        .iter()
        .map(|row| {
            Ok(InboundAsnLineReadModel {
                line_id: InboundAsnLineId::new(row.try_get("id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                sequence: row.try_get("sequence")?,
                item_id: CatalogItemId::new(row.try_get("item_id")?)
                    .map_err(|error| AppError::internal(error.to_string()))?,
                item_description: row.try_get("item_description")?,
                uom: row.try_get("uom")?,
                expected_quantity: row.try_get("expected_quantity")?,
                lot: row.try_get("lot")?,
                serial: row.try_get("serial")?,
                expiration: row.try_get("expiration")?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(Some(result))
}

async fn lock_source_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    inventory_owner_id: i64,
    number: &str,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "inbound-asn:{}:{inventory_owner_id}:{}",
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
            "advance shipping notice number already exists for this client",
        ))
    } else {
        Ok(())
    }
}

async fn lock_source_scope(
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
        Err(AppError::not_found("ASN client or facility"))
    } else {
        Ok(())
    }
}

async fn lock_source_items(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    command: &CreateInboundAsnCommand,
) -> AppResult<HashMap<i64, String>> {
    let item_ids = command
        .notice
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
        ORDER BY item.id
        FOR SHARE OF owner_item,item
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.notice.inventory_owner_id().get())
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
            "every ASN item must remain active and linked to the client",
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
        SELECT EXISTS(
            SELECT 1 FROM locations
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
            "receiving location is not executable for this ASN",
        ))
    }
}

async fn require_stored_visible_before_replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let operation = prepared.operation().as_str();
    let stored_asn_id: Option<i64> = sqlx::query_scalar(
        "SELECT (result_json->>'asn_id')::BIGINT FROM command_idempotency_records WHERE tenant_id=$1 AND operation=$2 AND idempotency_key=$3",
    )
        .bind(access.tenant_id.get())
        .bind(operation)
        .bind(prepared.idempotency_key())
        .fetch_optional(&mut **tx)
        .await?;
    let Some(asn_id) = stored_asn_id else {
        return Ok(());
    };
    let visible: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM inbound_asns
            WHERE tenant_id=$1 AND id=$2
              AND ($3 OR facility_id=ANY($4))
              AND ($5 OR inventory_owner_id=ANY($6)))
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(asn_id)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_one(&mut **tx)
    .await?;
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("advance shipping notice"))
    }
}

fn map_header(row: &sqlx::postgres::PgRow) -> AppResult<InboundAsnReadModel> {
    Ok(InboundAsnReadModel {
        asn_id: InboundAsnId::new(row.try_get("id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_id: InventoryOwnerId::new(row.try_get("inventory_owner_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: FacilityId::new(row.try_get("facility_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        facility_name: row.try_get("facility_name")?,
        number: row.try_get("number")?,
        supplier: row.try_get("supplier")?,
        expected_at: row.try_get("expected_at")?,
        status: parse_status(row.try_get::<String, _>("status")?.as_str())?,
        revision: revision(row.try_get("revision")?)?,
        line_count: row.try_get("line_count")?,
        total_expected_quantity: row.try_get("total_expected_quantity")?,
        load_id: row
            .try_get::<Option<i64>, _>("load_id")?
            .map(InboundLoadId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        created_by: UserId::new(row.try_get("created_by_user_id")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        created_at: row.try_get("created_at")?,
        planned_by: row
            .try_get::<Option<i64>, _>("planned_by_user_id")?
            .map(UserId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        planned_at: row.try_get("planned_at")?,
        purchase_order_source_id: row
            .try_get::<Option<i64>, _>("purchase_order_source_id")?
            .map(PurchaseOrderAsnSourceId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        purchase_order_id: row
            .try_get::<Option<i64>, _>("purchase_order_id")?
            .map(PurchaseOrderId::new)
            .transpose()
            .map_err(|error| AppError::internal(error.to_string()))?,
        purchase_order_number: row.try_get("purchase_order_number")?,
        lines: Vec::new(),
    })
}

fn parse_status(value: &str) -> AppResult<InboundAsnStatus> {
    InboundAsnStatus::parse(value).ok_or_else(|| AppError::internal("stored ASN status is invalid"))
}

fn revision(value: i64) -> AppResult<InboundAsnRevision> {
    InboundAsnRevision::new(value).map_err(|error| AppError::internal(error.to_string()))
}

async fn enqueue_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
    result: &CreateInboundAsnResult,
) -> AppResult<()> {
    let event_key = format!("inbound-asn:{}:created", result.asn_id.get());
    let aggregate_id = result.asn_id.to_string();
    let ordering_key = format!("inbound-asn:{}", result.asn_id.get());
    let payload = serde_json::json!({
        "asn_id": result.asn_id.get(),
        "number": result.number,
        "status": "open",
        "revision": result.revision.get(),
        "line_count": result.lines.len(),
        "total_expected_quantity": result.total_expected_quantity,
        "created_by": result.created_by.get(),
        "created_at": result.created_at,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(inventory_owner_id),
            facility_id: Some(facility_id),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &event_key,
            aggregate_type: "inbound_asn",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: result.revision.get(),
            event_type: "inbound.asn.created",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.created_at,
        },
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_planned_events(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    context: &CommandContext,
    inventory_owner_id: i64,
    facility_id: i64,
    number: &str,
    result: &PlanInboundAsnLoadResult,
) -> AppResult<()> {
    let owner = InventoryOwnerId::new(inventory_owner_id)
        .map_err(|error| AppError::internal(error.to_string()))?;
    let facility =
        FacilityId::new(facility_id).map_err(|error| AppError::internal(error.to_string()))?;
    let asn_event_key = format!("inbound-asn:{}:load-planned", result.asn_id.get());
    let asn_aggregate_id = result.asn_id.to_string();
    let asn_ordering_key = format!("inbound-asn:{}", result.asn_id.get());
    let payload = serde_json::json!({
        "plan_id": result.plan_id.get(),
        "asn_id": result.asn_id.get(),
        "number": number,
        "status": "planned",
        "revision": result.asn_revision.get(),
        "load_id": result.load_id.get(),
        "execution_barcode": result.execution_barcode,
        "line_count": result.lines.len(),
        "total_expected_quantity": result.total_expected_quantity,
        "planned_by": result.planned_by.get(),
        "planned_at": result.planned_at,
    });
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(owner),
            facility_id: Some(facility),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &asn_event_key,
            aggregate_type: "inbound_asn",
            aggregate_id: &asn_aggregate_id,
            ordering_key: &asn_ordering_key,
            aggregate_sequence: result.asn_revision.get(),
            event_type: "inbound.asn.load_planned",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.planned_at,
        },
    )
    .await?;
    let load_event_key = format!("inbound-load:{}:planned-from-asn", result.load_id.get());
    let load_aggregate_id = result.load_id.to_string();
    let load_ordering_key = format!("inbound-load:{}", result.load_id.get());
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id: access.tenant_id,
            inventory_owner_id: Some(owner),
            facility_id: Some(facility),
            actor_user_id: Some(context.actor_id.get()),
            event_key: &load_event_key,
            aggregate_type: "inbound_load",
            aggregate_id: &load_aggregate_id,
            ordering_key: &load_ordering_key,
            aggregate_sequence: 1,
            event_type: "inbound.load.planned",
            schema_version: 1,
            payload: &payload,
            occurred_at: result.planned_at,
        },
    )
    .await?;
    Ok(())
}
