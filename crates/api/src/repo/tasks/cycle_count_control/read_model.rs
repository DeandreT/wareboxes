use sqlx::Row;
use wareboxes_application::cycle_count_control::{
    CycleCountPolicyPage, CycleCountPolicyPageQuery, CycleCountPolicyReadModel,
    CycleCountVariancePage, CycleCountVariancePageQuery, CycleCountVarianceReadModel,
    CycleCountVarianceStockReadModel,
};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    CatalogItemId, CycleCountPolicyId, CycleCountPolicyRevision, CycleCountTolerancePolicy,
    CycleCountVarianceId, CycleCountVarianceRevision, FacilityId, InventoryBalanceId,
    InventoryOwnerId, LocationId, UserId,
};

use crate::db::{bind_tenant_context, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

use super::variance_status;

pub async fn cycle_count_policy_page(
    db: &Db,
    access: &TenantAccess,
    query: CycleCountPolicyPageQuery,
) -> AppResult<CycleCountPolicyPage> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        "wms_supervisor",
    )
    .await?;
    if query.limit == 0 || query.limit > 100 {
        return Err(AppError::bad_request(
            "cycle count policy page limit must be between 1 and 100",
        ));
    }
    if query
        .facility_id
        .is_some_and(|id| !scope.includes_facility(id.get()))
        || query
            .inventory_owner_id
            .is_some_and(|id| !scope.includes_inventory_owner(id.get()))
    {
        return Err(AppError::not_found("cycle count policy scope"));
    }
    let rows = sqlx::query(
        r#"
        SELECT policy.id, policy.inventory_owner_id, owner.name AS inventory_owner_name,
               policy.facility_id, facility.name AS facility_name,
               policy.absolute_tolerance_qty, policy.percentage_tolerance_bps,
               policy.automatic_recount_limit, policy.revision,
               policy.configured_by_user_id, policy.effective_from
        FROM cycle_count_policies policy
        JOIN inventory_owners owner
          ON owner.tenant_id=policy.tenant_id AND owner.id=policy.inventory_owner_id
         AND owner.deleted IS NULL
        JOIN facilities facility
          ON facility.tenant_id=policy.tenant_id AND facility.id=policy.facility_id
         AND facility.deleted IS NULL
        WHERE policy.tenant_id=$1 AND policy.effective_to IS NULL
          AND ($2 OR policy.facility_id=ANY($3))
          AND ($4 OR policy.inventory_owner_id=ANY($5))
          AND ($6::bigint IS NULL OR policy.facility_id=$6)
          AND ($7::bigint IS NULL OR policy.inventory_owner_id=$7)
          AND ($8::bigint IS NULL OR policy.id>$8)
        ORDER BY policy.id
        LIMIT $9
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(query.facility_id.map(FacilityId::get))
    .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(query.after_policy_id.map(CycleCountPolicyId::get))
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(map_policy)
        .collect::<AppResult<Vec<_>>>()?;
    let next_after_policy_id = if rows.len() > usize::from(query.limit) {
        items.last().map(|item| item.policy_id)
    } else {
        None
    };
    tx.commit().await?;
    Ok(CycleCountPolicyPage {
        items: std::mem::take(&mut items),
        next_after_policy_id,
    })
}

pub async fn cycle_count_variance_page(
    db: &Db,
    access: &TenantAccess,
    query: CycleCountVariancePageQuery,
) -> AppResult<CycleCountVariancePage> {
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        access.user_id.get(),
        "wms_supervisor",
    )
    .await?;
    if query.limit == 0 || query.limit > 100 {
        return Err(AppError::bad_request(
            "cycle count variance page limit must be between 1 and 100",
        ));
    }
    if query
        .facility_id
        .is_some_and(|id| !scope.includes_facility(id.get()))
        || query
            .inventory_owner_id
            .is_some_and(|id| !scope.includes_inventory_owner(id.get()))
    {
        return Err(AppError::not_found("cycle count variance scope"));
    }
    let rows = sqlx::query(
        r#"
        SELECT variance.id, variance.revision, variance.state,
               variance.inventory_owner_id, owner.name AS inventory_owner_name,
               variance.facility_id, facility.name AS facility_name,
               variance.inventory_balance_id, variance.location_id,
               location.barcode AS location_barcode, location.name AS location_name,
               variance.item_id, item.description AS item_description,
               sku.name AS primary_sku, plate.barcode AS license_plate_barcode,
               variance.uom, variance.lot, variance.serial,
               variance.inventory_status, variance.policy_id,
               variance.policy_revision, variance.absolute_tolerance_qty,
               variance.percentage_tolerance_bps, variance.automatic_recount_limit,
               variance.count_policy_source, variance.count_configuration_id,
               variance.count_configuration_revision, variance.count_scope_level,
               variance.count_inventory_owner_id, variance.count_facility_id,
               variance.count_absolute_tolerance_qty,
               variance.count_percentage_tolerance_bps,
               variance.count_approval_threshold_qty, variance.count_policy_hash,
               variance.latest_task_id, variance.latest_attempt_sequence,
               variance.automatic_recounts_used, variance.system_qty_on_hand,
               variance.counted_qty, variance.variance_qty,
               variance.allowed_variance_qty, variance.inventory_transaction_id,
               variance.created_at, variance.modified_at
        FROM cycle_count_variance_cases variance
        JOIN inventory_owners owner
          ON owner.tenant_id=variance.tenant_id AND owner.id=variance.inventory_owner_id
         AND owner.deleted IS NULL
        JOIN facilities facility
          ON facility.tenant_id=variance.tenant_id AND facility.id=variance.facility_id
         AND facility.deleted IS NULL
        JOIN locations location
          ON location.tenant_id=variance.tenant_id AND location.id=variance.location_id
        JOIN items item
          ON item.tenant_id=variance.tenant_id AND item.id=variance.item_id
        LEFT JOIN license_plates plate
          ON plate.tenant_id=variance.tenant_id AND plate.id=variance.license_plate_id
        LEFT JOIN LATERAL (
            SELECT name FROM skus
            WHERE tenant_id=variance.tenant_id AND item_id=variance.item_id
              AND deleted IS NULL
            ORDER BY id LIMIT 1
        ) sku ON true
        WHERE variance.tenant_id=$1
          AND ($2 OR variance.facility_id=ANY($3))
          AND ($4 OR variance.inventory_owner_id=ANY($5))
          AND ($6::bigint IS NULL OR variance.facility_id=$6)
          AND ($7::bigint IS NULL OR variance.inventory_owner_id=$7)
          AND ($8::text IS NULL OR variance.state=$8)
          AND ($9::bigint IS NULL OR variance.id>$9)
        ORDER BY variance.id
        LIMIT $10
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(query.facility_id.map(FacilityId::get))
    .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(query.status.map(status_text))
    .bind(query.after_variance_id.map(CycleCountVarianceId::get))
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let items = rows
        .iter()
        .take(usize::from(query.limit))
        .map(map_variance)
        .collect::<AppResult<Vec<_>>>()?;
    let next_after_variance_id = if rows.len() > usize::from(query.limit) {
        items.last().map(|item| item.variance_id)
    } else {
        None
    };
    tx.commit().await?;
    Ok(CycleCountVariancePage {
        items,
        next_after_variance_id,
    })
}

fn map_policy(row: &sqlx::postgres::PgRow) -> AppResult<CycleCountPolicyReadModel> {
    Ok(CycleCountPolicyReadModel {
        policy_id: id(row, "id", CycleCountPolicyId::new)?,
        inventory_owner_id: id(row, "inventory_owner_id", InventoryOwnerId::new)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: id(row, "facility_id", FacilityId::new)?,
        facility_name: row.try_get("facility_name")?,
        policy: policy(row)?,
        revision: revision(row, "revision")?,
        configured_by: id(row, "configured_by_user_id", UserId::new)?,
        configured_at: row.try_get("effective_from")?,
    })
}

fn map_variance(row: &sqlx::postgres::PgRow) -> AppResult<CycleCountVarianceReadModel> {
    Ok(CycleCountVarianceReadModel {
        variance_id: id(row, "id", CycleCountVarianceId::new)?,
        revision: CycleCountVarianceRevision::new(row.try_get("revision")?)
            .map_err(|error| AppError::internal(error.to_string()))?,
        status: variance_status(&row.try_get::<String, _>("state")?)?,
        inventory_owner_id: id(row, "inventory_owner_id", InventoryOwnerId::new)?,
        inventory_owner_name: row.try_get("inventory_owner_name")?,
        facility_id: id(row, "facility_id", FacilityId::new)?,
        facility_name: row.try_get("facility_name")?,
        stock: CycleCountVarianceStockReadModel {
            inventory_balance_id: id(row, "inventory_balance_id", InventoryBalanceId::new)?,
            location_id: id(row, "location_id", LocationId::new)?,
            location_barcode: row.try_get("location_barcode")?,
            location_name: row.try_get("location_name")?,
            item_id: CatalogItemId::new(row.try_get("item_id")?)
                .map_err(|error| AppError::internal(error.to_string()))?,
            item_description: row.try_get("item_description")?,
            primary_sku: row.try_get("primary_sku")?,
            license_plate_barcode: row.try_get("license_plate_barcode")?,
            uom: row.try_get("uom")?,
            lot: row.try_get("lot")?,
            serial: row.try_get("serial")?,
            inventory_status: row.try_get("inventory_status")?,
        },
        policy_id: id(row, "policy_id", CycleCountPolicyId::new)?,
        policy_revision: revision(row, "policy_revision")?,
        policy: policy(row)?,
        decision_policy: super::decision_policy::count_decision_policy_from_row(row)?,
        latest_task_id: row.try_get("latest_task_id")?,
        latest_attempt_sequence: u16::try_from(row.try_get::<i16, _>("latest_attempt_sequence")?)
            .map_err(|_| {
            AppError::internal("stored count attempt is invalid")
        })?,
        automatic_recounts_used: u16::try_from(row.try_get::<i16, _>("automatic_recounts_used")?)
            .map_err(|_| {
            AppError::internal("stored recount usage is invalid")
        })?,
        system_quantity: row.try_get("system_qty_on_hand")?,
        counted_quantity: row.try_get("counted_qty")?,
        variance_quantity: row.try_get("variance_qty")?,
        allowed_variance_quantity: row.try_get("allowed_variance_qty")?,
        inventory_transaction_id: row.try_get("inventory_transaction_id")?,
        created_at: row.try_get("created_at")?,
        modified_at: row.try_get("modified_at")?,
    })
}

fn policy(row: &sqlx::postgres::PgRow) -> AppResult<CycleCountTolerancePolicy> {
    CycleCountTolerancePolicy::new(
        row.try_get("absolute_tolerance_qty")?,
        u32::try_from(row.try_get::<i32, _>("percentage_tolerance_bps")?)
            .map_err(|_| AppError::internal("stored percentage tolerance is invalid"))?,
        u16::try_from(row.try_get::<i16, _>("automatic_recount_limit")?)
            .map_err(|_| AppError::internal("stored recount limit is invalid"))?,
    )
    .map_err(|error| AppError::internal(error.to_string()))
}

fn revision(row: &sqlx::postgres::PgRow, name: &str) -> AppResult<CycleCountPolicyRevision> {
    CycleCountPolicyRevision::new(row.try_get(name)?)
        .map_err(|error| AppError::internal(error.to_string()))
}

fn id<T>(
    row: &sqlx::postgres::PgRow,
    name: &str,
    constructor: impl FnOnce(i64) -> Result<T, wareboxes_domain::InvalidId>,
) -> AppResult<T> {
    constructor(row.try_get(name)?).map_err(|error| AppError::internal(error.to_string()))
}

const fn status_text(status: wareboxes_domain::CycleCountVarianceStatus) -> &'static str {
    match status {
        wareboxes_domain::CycleCountVarianceStatus::AwaitingRecount => "awaiting_recount",
        wareboxes_domain::CycleCountVarianceStatus::AwaitingApproval => "awaiting_approval",
        wareboxes_domain::CycleCountVarianceStatus::Posted => "posted",
    }
}
