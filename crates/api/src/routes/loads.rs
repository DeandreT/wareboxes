use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use wareboxes_core::dto::{
    AddLoad, AddLoadFile, AddLoadLine, AddLoadNote, ArriveLoad, LoadFileIdRequest, LoadIdRequest,
    LoadNoteIdRequest, LoadUpdate,
};
use wareboxes_core::models::{Load, LoadFileCategory, LoadStatus, LoadType};

use crate::auth::CurrentTenant;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::permissions;
use crate::repo;
use crate::routes::validate;
use crate::state::AppState;

const PERM: &str = "wms";
const DEFAULT_LOAD_LIMIT: i64 = 500;
const MAX_LOAD_LIMIT: i64 = 2_000;

async fn require_active_load(
    db: &Db,
    user: &CurrentTenant,
    load_id: i64,
) -> AppResult<repo::access::OperationalDimensions> {
    repo::access::load_dimensions(db, &user.tenant, load_id, false)
        .await?
        .ok_or_else(|| AppError::not_found("load"))
}

async fn require_active_location_in_facility(
    db: &Db,
    tenant_id: wareboxes_domain::TenantId,
    facility_id: i64,
    location_id: i64,
    label: &'static str,
) -> AppResult<()> {
    if !wareboxes_persistence_postgres::locations::active_location_exists_in_facility(
        db,
        tenant_id,
        facility_id,
        location_id,
    )
    .await?
    {
        return Err(AppError::bad_request(format!("{label} not found")));
    }
    match wareboxes_persistence_postgres::locations::location_active_state(
        db,
        tenant_id,
        location_id,
    )
    .await?
    {
        Some(true) => Ok(()),
        Some(false) => Err(AppError::bad_request(format!("{label} is inactive"))),
        None => Err(AppError::bad_request(format!("{label} not found"))),
    }
}

#[derive(Debug, Deserialize)]
pub struct LoadListQuery {
    #[serde(default)]
    pub show_deleted: bool,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub status: Option<String>,
    pub load_type: Option<String>,
    pub inventory_owner_id: Option<i64>,
    pub facility_id: Option<i64>,
    pub appointment_date: Option<String>,
    pub sort: Option<String>,
    pub direction: Option<String>,
}

fn load_summary_query(query: LoadListQuery) -> AppResult<repo::loads::LoadSummaryQuery> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_LOAD_LIMIT)
        .clamp(1, MAX_LOAD_LIMIT);
    let offset = query.offset.unwrap_or(0).max(0);
    let search = query
        .search
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if search
        .as_ref()
        .is_some_and(|value| value.chars().count() > 200)
    {
        return Err(AppError::bad_request(
            "load search must not exceed 200 characters",
        ));
    }
    let status = query
        .status
        .map(|value| {
            LoadStatus::parse(&value).ok_or_else(|| AppError::bad_request("invalid load status"))
        })
        .transpose()?;
    let load_type = query
        .load_type
        .map(|value| {
            LoadType::parse(&value).ok_or_else(|| AppError::bad_request("invalid load type"))
        })
        .transpose()?;
    if query.inventory_owner_id.is_some_and(|value| value <= 0)
        || query.facility_id.is_some_and(|value| value <= 0)
    {
        return Err(AppError::bad_request(
            "inventory_owner_id and facility_id must be positive",
        ));
    }
    let appointment_date = query
        .appointment_date
        .map(|value| {
            chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d")
                .map_err(|_| AppError::bad_request("appointment_date must use YYYY-MM-DD"))
        })
        .transpose()?;
    let sort = match query.sort.as_deref().unwrap_or("id") {
        "id" => repo::loads::LoadSummarySort::Id,
        "type" => repo::loads::LoadSummarySort::LoadType,
        "reference" => repo::loads::LoadSummarySort::Reference,
        "inventory_owner" => repo::loads::LoadSummarySort::InventoryOwner,
        "facility" => repo::loads::LoadSummarySort::Facility,
        "status" => repo::loads::LoadSummarySort::Status,
        "appointment" => repo::loads::LoadSummarySort::Appointment,
        _ => return Err(AppError::bad_request("invalid load sort")),
    };
    let direction = match query.direction.as_deref().unwrap_or("desc") {
        "asc" => repo::loads::LoadSummaryDirection::Ascending,
        "desc" => repo::loads::LoadSummaryDirection::Descending,
        _ => return Err(AppError::bad_request("invalid load sort direction")),
    };
    Ok(repo::loads::LoadSummaryQuery {
        show_deleted: query.show_deleted,
        search,
        status,
        load_type,
        inventory_owner_id: query.inventory_owner_id,
        facility_id: query.facility_id,
        appointment_date,
        sort,
        direction,
        limit,
        offset,
    })
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(q): Query<LoadListQuery>,
) -> AppResult<Json<Vec<Load>>> {
    user.require_permission(&state.db, PERM).await?;
    let query = load_summary_query(q)?;
    Ok(Json(
        repo::loads::get_load_summaries_page_in_scope(&state.db, &user.tenant, &query).await?,
    ))
}

pub async fn get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(load_id): Path<i64>,
) -> AppResult<Json<Option<Load>>> {
    user.require_permission(&state.db, PERM).await?;
    let show_deleted_notes =
        permissions::user_has_permission(&state.db, user.tenant.tenant_id, user.user.id, "admin")
            .await?;
    Ok(Json(
        repo::loads::get_load_in_scope(&state.db, &user.tenant, load_id, show_deleted_notes)
            .await?,
    ))
}

pub async fn mobile_inbound_list(
    State(state): State<AppState>,
    user: CurrentTenant,
) -> AppResult<Json<Vec<Load>>> {
    user.require_permission(&state.db, PERM).await?;
    let mut query = repo::loads::LoadSummaryQuery::operational(MAX_LOAD_LIMIT, 0);
    query.load_type = Some(LoadType::Inbound);
    Ok(Json(
        repo::loads::get_load_summaries_page_in_scope(&state.db, &user.tenant, &query).await?,
    ))
}

pub async fn mobile_inbound_get(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(load_id): Path<i64>,
) -> AppResult<Json<Option<Load>>> {
    user.require_permission(&state.db, PERM).await?;
    let load = repo::loads::get_load_in_scope(&state.db, &user.tenant, load_id, false)
        .await?
        .filter(|load| load.r#type == LoadType::Inbound);
    Ok(Json(load))
}

pub async fn mobile_arrive(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(load_id): Path<i64>,
    Json(body): Json<ArriveLoad>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    require_active_load(&state.db, &user, load_id).await?;
    if body
        .arrival
        .is_some_and(|arrival| arrival > chrono::Utc::now())
    {
        return Err(crate::error::AppError::bad_request(
            "arrival time cannot be in the future",
        ));
    }
    let ok = repo::loads::update_load(
        &state.db,
        user.tenant.tenant_id,
        user.user.id,
        load_id,
        Some(LoadStatus::Arrived),
        None,
        None,
        body.invoice_number.as_deref(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        body.arrival,
        None,
        None,
        None,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn add(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddLoad>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    user.require_facility(body.facility_id)?;
    user.require_inventory_owner(body.inventory_owner_id)?;
    if !wareboxes_persistence_postgres::facilities::active_facility_exists_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.site_scope,
        body.facility_id,
    )
    .await?
    {
        return Err(AppError::bad_request("Facility not found"));
    }
    if !repo::inventory_owners::active_inventory_owner_exists_in_scope(
        &state.db,
        user.tenant.tenant_id,
        &user.tenant.owner_scope,
        body.inventory_owner_id,
    )
    .await?
    {
        return Err(AppError::bad_request("Inventory owner not found"));
    }
    if let Some(location_id) = body.dock_door_location_id {
        require_active_location_in_facility(
            &state.db,
            user.tenant.tenant_id,
            body.facility_id,
            location_id,
            "Dock door location",
        )
        .await?;
    }
    let id = repo::loads::add_load(
        &state.db,
        user.tenant.tenant_id,
        user.user.id,
        body.facility_id,
        body.inventory_owner_id,
        body.r#type,
        body.reference_number.as_deref(),
        body.invoice_number.as_deref(),
        body.carrier.as_deref(),
        body.trailer_number.as_deref(),
        body.seal_number.as_deref(),
        body.dock_door_location_id,
        body.expected_time,
        body.appointment_time,
    )
    .await?;
    Ok(Json(id))
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<LoadUpdate>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    let Some(dimensions) =
        repo::access::load_dimensions(&state.db, &user.tenant, body.load_id, false).await?
    else {
        return Ok(Json(false));
    };
    if let Some(location_id) = body.dock_door_location_id {
        require_active_location_in_facility(
            &state.db,
            user.tenant.tenant_id,
            dimensions.facility_id.get(),
            location_id,
            "Dock door location",
        )
        .await?;
    }
    let ok = repo::loads::update_load(
        &state.db,
        user.tenant.tenant_id,
        user.user.id,
        body.load_id,
        body.status,
        body.r#type,
        body.reference_number.as_deref(),
        body.invoice_number.as_deref(),
        body.carrier.as_deref(),
        body.trailer_number.as_deref(),
        body.seal_number.as_deref(),
        body.dock_door_location_id,
        body.expected_time,
        body.appointment_time,
        body.actual_time,
        body.arrival,
        body.departure,
        body.rejected,
        body.closed,
    )
    .await?;
    Ok(Json(ok))
}

pub async fn delete(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<LoadIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if repo::access::load_dimensions(&state.db, &user.tenant, body.load_id, false)
        .await?
        .is_none()
    {
        return Ok(Json(false));
    }
    Ok(Json(
        repo::loads::set_load_deleted(
            &state.db,
            user.tenant.tenant_id,
            user.user.id,
            body.load_id,
            true,
        )
        .await?,
    ))
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<LoadIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if repo::access::load_dimensions(&state.db, &user.tenant, body.load_id, true)
        .await?
        .is_none()
    {
        return Ok(Json(false));
    }
    Ok(Json(
        repo::loads::set_load_deleted(
            &state.db,
            user.tenant.tenant_id,
            user.user.id,
            body.load_id,
            false,
        )
        .await?,
    ))
}

pub async fn add_note(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddLoadNote>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    require_active_load(&state.db, &user, body.load_id).await?;
    let id = repo::loads::add_note(
        &state.db,
        user.tenant.tenant_id,
        user.user.id,
        body.load_id,
        &body.note,
    )
    .await?;
    Ok(Json(id))
}

pub async fn delete_note(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<LoadNoteIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if repo::access::load_note_dimensions(&state.db, &user.tenant, body.load_note_id)
        .await?
        .is_none()
    {
        return Ok(Json(false));
    }
    Ok(Json(
        repo::loads::set_load_note_deleted(
            &state.db,
            user.tenant.tenant_id,
            user.user.id,
            body.load_note_id,
            true,
        )
        .await?,
    ))
}

pub async fn add_line(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddLoadLine>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    require_active_load(&state.db, &user, body.load_id).await?;
    let id = repo::loads::add_line(
        &state.db,
        user.tenant.tenant_id,
        user.user.id,
        body.load_id,
        body.item_id,
        body.sku_id,
        body.expected_qty,
        body.lot.as_deref(),
        body.serial.as_deref(),
        body.expiration,
    )
    .await?;
    Ok(Json(id))
}

pub async fn add_file(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<AddLoadFile>,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    require_active_load(&state.db, &user, body.load_id).await?;
    let id = repo::loads::add_file(
        &state.db,
        user.tenant.tenant_id,
        user.user.id,
        body.load_id,
        &body.original_name,
        &body.name,
        &body.path,
        body.content_type.as_deref(),
        body.category.unwrap_or(LoadFileCategory::General),
    )
    .await?;
    Ok(Json(id))
}

pub async fn delete_file(
    State(state): State<AppState>,
    user: CurrentTenant,
    Json(body): Json<LoadFileIdRequest>,
) -> AppResult<Json<bool>> {
    user.require_permission(&state.db, PERM).await?;
    validate(&body)?;
    if repo::access::load_file_dimensions(&state.db, &user.tenant, body.file_id)
        .await?
        .is_none()
    {
        return Ok(Json(false));
    }
    Ok(Json(
        repo::loads::delete_file(&state.db, user.tenant.tenant_id, user.user.id, body.file_id)
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> LoadListQuery {
        LoadListQuery {
            show_deleted: false,
            limit: Some(100),
            offset: Some(0),
            search: None,
            status: None,
            load_type: None,
            inventory_owner_id: None,
            facility_id: None,
            appointment_date: None,
            sort: None,
            direction: None,
        }
    }

    #[test]
    fn load_list_query_maps_all_server_sort_and_filter_fields() {
        let mut request = request();
        request.search = Some("  TRAILER-7  ".to_owned());
        request.status = Some("arrived".to_owned());
        request.load_type = Some("inbound".to_owned());
        request.inventory_owner_id = Some(2);
        request.facility_id = Some(3);
        request.appointment_date = Some("2026-08-10".to_owned());
        request.sort = Some("appointment".to_owned());
        request.direction = Some("asc".to_owned());

        let query = load_summary_query(request).unwrap();
        assert_eq!(query.search.as_deref(), Some("TRAILER-7"));
        assert_eq!(query.status, Some(LoadStatus::Arrived));
        assert_eq!(query.load_type, Some(LoadType::Inbound));
        assert_eq!(query.inventory_owner_id, Some(2));
        assert_eq!(query.facility_id, Some(3));
        assert_eq!(
            query.appointment_date,
            chrono::NaiveDate::from_ymd_opt(2026, 8, 10)
        );
        assert_eq!(query.sort, repo::loads::LoadSummarySort::Appointment);
        assert_eq!(
            query.direction,
            repo::loads::LoadSummaryDirection::Ascending
        );
    }

    #[test]
    fn load_list_query_rejects_invalid_server_sort_and_filters() {
        let invalid: [fn(&mut LoadListQuery); 4] = [
            |request: &mut LoadListQuery| request.sort = Some("unknown".to_owned()),
            |request: &mut LoadListQuery| request.direction = Some("sideways".to_owned()),
            |request: &mut LoadListQuery| request.facility_id = Some(0),
            |request: &mut LoadListQuery| request.appointment_date = Some("08/10/2026".to_owned()),
        ];
        for mutate in invalid {
            let mut request = request();
            mutate(&mut request);
            assert!(load_summary_query(request).is_err());
        }
    }
}
