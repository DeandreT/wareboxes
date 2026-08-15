use wareboxes_application::yard::{YardWorkspace, YardWorkspaceFilter};
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::YardVisitId;

use super::models::{appointment, asset, location, read_visit_tx};
use super::{require_facility, require_scope, PERMISSION};
use crate::db::{begin_tenant_transaction, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx};

pub async fn workspace(
    db: &Db,
    access: &TenantAccess,
    filter: &YardWorkspaceFilter,
) -> AppResult<YardWorkspace> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;
    if let Some(facility_id) = filter.facility_id {
        require_facility(&scope, facility_id)?;
    }
    if let Some(owner_id) = filter.inventory_owner_id {
        if let Some(facility_id) = filter.facility_id {
            require_scope(&scope, owner_id, facility_id)?;
        } else if !scope.includes_inventory_owner(owner_id.get()) {
            return Err(AppError::not_found("yard record"));
        }
    }
    let location_rows = sqlx::query(
        r#"SELECT location.*,facility.name AS facility_name FROM yard_locations location
           JOIN facilities facility ON facility.tenant_id=location.tenant_id
             AND facility.id=location.facility_id
           WHERE location.tenant_id=$1 AND location.active
             AND ($2 OR location.facility_id=ANY($3))
             AND ($4::BIGINT IS NULL OR location.facility_id=$4)
           ORDER BY facility.name,location.kind,location.code LIMIT 500"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(filter.facility_id.map(|id| id.get()))
    .fetch_all(&mut *tx)
    .await?;
    let asset_rows = sqlx::query(
        "SELECT * FROM yard_assets WHERE tenant_id=$1 AND active ORDER BY kind,asset_number LIMIT 500",
    )
    .bind(access.tenant_id.get())
    .fetch_all(&mut *tx)
    .await?;
    let appointment_rows = sqlx::query(
        r#"SELECT appointment.*,owner.name AS inventory_owner_name,
                  facility.name AS facility_name
           FROM yard_appointments appointment
           JOIN inventory_owners owner ON owner.tenant_id=appointment.tenant_id
             AND owner.id=appointment.inventory_owner_id
           JOIN facilities facility ON facility.tenant_id=appointment.tenant_id
             AND facility.id=appointment.facility_id
           WHERE appointment.tenant_id=$1
             AND ($2 OR appointment.facility_id=ANY($3))
             AND ($4 OR appointment.inventory_owner_id=ANY($5))
             AND ($6::BIGINT IS NULL OR appointment.facility_id=$6)
             AND ($7::BIGINT IS NULL OR appointment.inventory_owner_id=$7)
             AND ($8 OR appointment.status IN ('scheduled','checked_in'))
           ORDER BY appointment.scheduled_from,appointment.id LIMIT 500"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.facility_id.map(|id| id.get()))
    .bind(filter.inventory_owner_id.map(|id| id.get()))
    .bind(filter.include_completed)
    .fetch_all(&mut *tx)
    .await?;
    let visit_ids = sqlx::query_scalar::<_, i64>(
        r#"SELECT id FROM yard_visits visit WHERE tenant_id=$1
             AND ($2 OR facility_id=ANY($3))
             AND ($4 OR inventory_owner_id=ANY($5))
             AND ($6::BIGINT IS NULL OR facility_id=$6)
             AND ($7::BIGINT IS NULL OR inventory_owner_id=$7)
             AND ($8 OR status<>'gated_out')
             AND ($9::BIGINT IS NULL OR id<$9)
           ORDER BY id DESC LIMIT $10"#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(filter.facility_id.map(|id| id.get()))
    .bind(filter.inventory_owner_id.map(|id| id.get()))
    .bind(filter.include_completed)
    .bind(filter.before_visit_id.map(|id| id.get()))
    .bind(i64::from(filter.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let has_more = visit_ids.len() > usize::from(filter.limit);
    let visible_ids = visit_ids
        .iter()
        .take(usize::from(filter.limit))
        .copied()
        .collect::<Vec<_>>();
    let mut visits = Vec::with_capacity(visible_ids.len());
    for visit_id in &visible_ids {
        visits.push(
            read_visit_tx(
                &mut tx,
                access.tenant_id,
                YardVisitId::new(*visit_id)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            )
            .await?,
        );
    }
    let next_visit_id = has_more
        .then(|| visible_ids.last().copied())
        .flatten()
        .map(YardVisitId::new)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let result = YardWorkspace {
        locations: location_rows
            .iter()
            .map(location)
            .collect::<AppResult<Vec<_>>>()?,
        assets: asset_rows
            .iter()
            .map(asset)
            .collect::<AppResult<Vec<_>>>()?,
        appointments: appointment_rows
            .iter()
            .map(appointment)
            .collect::<AppResult<Vec<_>>>()?,
        visits,
        next_visit_id,
    };
    tx.commit().await?;
    Ok(result)
}
