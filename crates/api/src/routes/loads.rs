use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
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
const MAX_UPLOAD_BYTES: usize = 8 * 1024 * 1024;

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
    Err(AppError::conflict(
        "use the scanned v1 inbound arrival workflow",
    ))
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
    if body.status.is_some_and(|status| {
        matches!(
            status,
            LoadStatus::Arrived | LoadStatus::Receiving | LoadStatus::Received | LoadStatus::Closed
        )
    }) {
        let load = repo::loads::get_load_in_scope(&state.db, &user.tenant, body.load_id, false)
            .await?
            .ok_or_else(|| AppError::not_found("load"))?;
        if load.r#type == LoadType::Inbound {
            return Err(AppError::conflict(
                "inbound execution states require the scanned v1 workflow",
            ));
        }
    }
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

pub async fn upload_file(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(load_id): Path<i64>,
    mut multipart: Multipart,
) -> AppResult<Json<i64>> {
    user.require_permission(&state.db, PERM).await?;
    require_active_load(&state.db, &user, load_id).await?;

    let mut category = LoadFileCategory::General;
    let mut upload = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::bad_request(format!("invalid document upload: {error}")))?
    {
        match field.name() {
            Some("category") => {
                let value = field.text().await.map_err(|error| {
                    AppError::bad_request(format!("invalid document category: {error}"))
                })?;
                category = LoadFileCategory::parse(value.trim())
                    .ok_or_else(|| AppError::bad_request("invalid document category"))?;
            }
            Some("file") if upload.is_none() => {
                let original_name = uploaded_file_name(field.file_name().unwrap_or_default())?;
                let content_type = field.content_type().map(str::to_owned);
                let content = field.bytes().await.map_err(|error| {
                    AppError::bad_request(format!("invalid document content: {error}"))
                })?;
                if content.is_empty() {
                    return Err(AppError::bad_request("document is empty"));
                }
                if content.len() > MAX_UPLOAD_BYTES {
                    return Err(AppError::bad_request(format!(
                        "document exceeds the {} MB upload limit",
                        MAX_UPLOAD_BYTES / (1024 * 1024)
                    )));
                }
                upload = Some((original_name, content_type, content));
            }
            Some("file") => return Err(AppError::bad_request("upload exactly one document")),
            Some(_) | None => return Err(AppError::bad_request("invalid document upload field")),
        }
    }

    let (original_name, content_type, content) =
        upload.ok_or_else(|| AppError::bad_request("choose a document to upload"))?;
    let id = repo::loads::add_uploaded_file(
        &state.db,
        user.tenant.tenant_id,
        user.user.id,
        load_id,
        &original_name,
        content_type.as_deref(),
        category,
        &content,
    )
    .await?;
    Ok(Json(id))
}

pub async fn file_content(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(file_id): Path<i64>,
) -> AppResult<Response> {
    user.require_permission(&state.db, PERM).await?;
    if repo::access::load_file_dimensions(&state.db, &user.tenant, file_id)
        .await?
        .is_none()
    {
        return Err(AppError::not_found("document"));
    }
    let file = repo::loads::get_file_content(&state.db, user.tenant.tenant_id, file_id)
        .await?
        .ok_or_else(|| AppError::not_found("document content"))?;
    let mut response = Response::new(Body::from(file.content));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        file.content_type
            .as_deref()
            .and_then(|value| HeaderValue::from_str(value).ok())
            .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream")),
    );
    let disposition = format!(
        "attachment; filename=\"{}\"",
        download_file_name(&file.original_name)
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .map_err(|error| AppError::internal(format!("invalid document filename: {error}")))?,
    );
    Ok(response)
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

fn uploaded_file_name(value: &str) -> AppResult<String> {
    let name = value
        .split(['/', '\\'])
        .next_back()
        .unwrap_or_default()
        .trim();
    if name.is_empty() {
        return Err(AppError::bad_request("document filename is required"));
    }
    if name.chars().count() > 255 || name.chars().any(char::is_control) {
        return Err(AppError::bad_request("document filename is invalid"));
    }
    Ok(name.to_owned())
}

fn download_file_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | ' ') {
                character
            } else {
                '_'
            }
        })
        .collect()
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

    #[test]
    fn uploaded_document_names_are_bounded_and_download_safe() {
        assert_eq!(
            uploaded_file_name(r"C:\fakepath\Bill of lading.pdf").unwrap(),
            "Bill of lading.pdf"
        );
        assert!(uploaded_file_name(" ").is_err());
        assert!(uploaded_file_name("line\nbreak.pdf").is_err());
        assert!(uploaded_file_name(&"a".repeat(256)).is_err());
        assert_eq!(download_file_name("client \"BOL\".pdf"), "client _BOL_.pdf");
    }
}
