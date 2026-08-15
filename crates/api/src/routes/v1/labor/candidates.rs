use axum::extract::{Query, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    LaborActivityKind as ApiActivityKind, LaborQuantityBasis as ApiQuantityBasis,
    LaborReferenceCandidatePageRequest, LaborReferenceCandidatePageResponse,
    LaborReferenceCandidateResponse, LaborReferenceType as ApiReferenceType,
    LaborRosterCandidateResponse, LaborRosterPageRequest, LaborRosterPageResponse, OpaqueCursor,
    Revision,
};
use wareboxes_domain::{EmployeeId, FacilityId, InventoryOwnerId, LaborActivityKind};

use super::{
    activity_kind_from_api, quantity_basis_from_api, validation, CERTIFY_PERMISSION,
    EXECUTE_PERMISSION, SUPERVISE_PERMISSION,
};
use crate::auth::CurrentTenant;
use crate::repo;
use crate::routes::v1::error::{V1Error, V1Result};
use crate::state::AppState;

const ROSTER_CURSOR_PREFIX: &str = "lr1.";
const REFERENCE_CURSOR_PREFIX: &str = "lrc1.";

pub async fn roster_candidates(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<LaborRosterPageRequest>,
) -> V1Result<Json<LaborRosterPageResponse>> {
    user.require_any_permission(
        &state.db,
        &[EXECUTE_PERMISSION, SUPERVISE_PERMISSION, CERTIFY_PERMISSION],
    )
    .await?;
    let after = request
        .cursor
        .as_ref()
        .map(|cursor| decode_roster_cursor(cursor, &request))
        .transpose()?;
    let result = repo::labor::roster_candidates(
        &state.db,
        &user.tenant,
        &repo::labor::LaborRosterFilter {
            facility_id: FacilityId::new(request.facility_id).map_err(validation)?,
            inventory_owner_id: request
                .inventory_owner_id
                .map(InventoryOwnerId::new)
                .transpose()
                .map_err(validation)?,
            after,
            limit: u32::from(request.limit.get()),
        },
    )
    .await?;
    Ok(Json(LaborRosterPageResponse {
        items: result
            .items
            .into_iter()
            .map(|item| {
                Ok(LaborRosterCandidateResponse {
                    employee_id: item.employee_id.get(),
                    display_name: item.display_name,
                    title: item.title,
                    facility_id: item.facility_id.get(),
                    attendance_interval_id: item.attendance_interval_id.map(|id| id.get()),
                    attendance_revision: item
                        .attendance_revision
                        .map(|revision| Revision::new(revision.get()))
                        .transpose()
                        .map_err(validation)?,
                    active_activity_id: item.active_activity_id.map(|id| id.get()),
                    certified_skill_ids: item
                        .certified_skill_ids
                        .into_iter()
                        .map(|id| id.get())
                        .collect(),
                    can_clock_in: item.can_clock_in,
                    can_start_activity: item.can_start_activity,
                    eligibility_evidence: item.eligibility_evidence,
                })
            })
            .collect::<V1Result<Vec<_>>>()?,
        next_cursor: result
            .next_after
            .map(|employee_id| encode_roster_cursor(employee_id, &request))
            .transpose()?,
    }))
}

pub async fn reference_candidates(
    State(state): State<AppState>,
    user: CurrentTenant,
    Query(request): Query<LaborReferenceCandidatePageRequest>,
) -> V1Result<Json<LaborReferenceCandidatePageResponse>> {
    user.require_any_permission(&state.db, &[EXECUTE_PERMISSION, SUPERVISE_PERMISSION])
        .await?;
    let after = request
        .cursor
        .as_ref()
        .map(|cursor| decode_reference_cursor(cursor, &request))
        .transpose()?;
    let activity_kind = activity_kind_from_api(request.activity_kind);
    let quantity_basis = quantity_basis_from_api(request.quantity_basis);
    let result = repo::labor::reference_candidates(
        &state.db,
        &user.tenant,
        &repo::labor::LaborReferenceCandidateFilter {
            facility_id: FacilityId::new(request.facility_id).map_err(validation)?,
            inventory_owner_id: request
                .inventory_owner_id
                .map(InventoryOwnerId::new)
                .transpose()
                .map_err(validation)?,
            employee_id: EmployeeId::new(request.employee_id).map_err(validation)?,
            activity_kind,
            quantity_basis,
            after,
            limit: u32::from(request.limit.get()),
        },
    )
    .await?;
    let reference_type = reference_type_for(activity_kind)
        .ok_or_else(|| V1Error::internal("direct labor kind has no reference contract"))?;
    Ok(Json(LaborReferenceCandidatePageResponse {
        employee_id: result.employee_id.get(),
        attendance_interval_id: result.attendance_interval_id.get(),
        items: result
            .items
            .into_iter()
            .map(|item| LaborReferenceCandidateResponse {
                reference_type,
                reference_id: item.reference_id,
                display_label: item.display_label,
                facility_id: item.facility_id.get(),
                inventory_owner_id: item.inventory_owner_id.map(|id| id.get()),
                activity_kind: request.activity_kind,
                quantity_basis: request.quantity_basis,
                canonical_quantity: item.canonical_quantity,
                eligibility_evidence: item.eligibility_evidence,
            })
            .collect(),
        next_cursor: result
            .next_after
            .map(|reference_id| encode_reference_cursor(reference_id, &request))
            .transpose()?,
    }))
}

fn encode_roster_cursor(
    employee_id: EmployeeId,
    request: &LaborRosterPageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{ROSTER_CURSOR_PREFIX}{}.{:016x}",
        roster_filter(request),
        employee_id.get()
    ))
    .map_err(|_| V1Error::internal("generated an invalid labor roster cursor"))
}

fn decode_roster_cursor(
    cursor: &OpaqueCursor,
    request: &LaborRosterPageRequest,
) -> V1Result<EmployeeId> {
    let encoded = cursor
        .as_str()
        .strip_prefix(ROSTER_CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("labor roster"))?;
    let (filter, id) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("labor roster"))?;
    if filter != roster_filter(request) {
        return Err(V1Error::invalid_cursor_for("labor roster filters"));
    }
    let id =
        i64::from_str_radix(id, 16).map_err(|_| V1Error::invalid_cursor_for("labor roster"))?;
    EmployeeId::new(id).map_err(|_| V1Error::invalid_cursor_for("labor roster"))
}

fn roster_filter(request: &LaborRosterPageRequest) -> String {
    format!(
        "{:016x}.{}",
        request.facility_id,
        request
            .inventory_owner_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}"))
    )
}

fn encode_reference_cursor(
    reference_id: i64,
    request: &LaborReferenceCandidatePageRequest,
) -> V1Result<OpaqueCursor> {
    OpaqueCursor::new(format!(
        "{REFERENCE_CURSOR_PREFIX}{}.{reference_id:016x}",
        reference_filter(request)
    ))
    .map_err(|_| V1Error::internal("generated an invalid labor reference cursor"))
}

fn decode_reference_cursor(
    cursor: &OpaqueCursor,
    request: &LaborReferenceCandidatePageRequest,
) -> V1Result<i64> {
    let encoded = cursor
        .as_str()
        .strip_prefix(REFERENCE_CURSOR_PREFIX)
        .ok_or_else(|| V1Error::invalid_cursor_for("labor reference candidates"))?;
    let (filter, id) = encoded
        .rsplit_once('.')
        .ok_or_else(|| V1Error::invalid_cursor_for("labor reference candidates"))?;
    if filter != reference_filter(request) {
        return Err(V1Error::invalid_cursor_for(
            "labor reference candidate filters",
        ));
    }
    let id = i64::from_str_radix(id, 16)
        .map_err(|_| V1Error::invalid_cursor_for("labor reference candidates"))?;
    if id <= 0 {
        return Err(V1Error::invalid_cursor_for("labor reference candidates"));
    }
    Ok(id)
}

fn reference_filter(request: &LaborReferenceCandidatePageRequest) -> String {
    format!(
        "{:016x}.{}.{:016x}.{}.{}",
        request.facility_id,
        request
            .inventory_owner_id
            .map_or_else(|| "-".to_owned(), |id| format!("{id:016x}")),
        request.employee_id,
        activity_kind_name(request.activity_kind),
        quantity_basis_name(request.quantity_basis),
    )
}

const fn reference_type_for(kind: LaborActivityKind) -> Option<ApiReferenceType> {
    match kind {
        LaborActivityKind::Receiving => Some(ApiReferenceType::InboundLoad),
        LaborActivityKind::Putaway
        | LaborActivityKind::Replenishment
        | LaborActivityKind::CycleCount
        | LaborActivityKind::InventoryRelocation
        | LaborActivityKind::CrossDock => Some(ApiReferenceType::WorkTask),
        LaborActivityKind::Picking => Some(ApiReferenceType::PickTask),
        LaborActivityKind::Packing => Some(ApiReferenceType::PackingSession),
        LaborActivityKind::Shipping => Some(ApiReferenceType::Shipment),
        LaborActivityKind::Yard => Some(ApiReferenceType::YardVisit),
        LaborActivityKind::CustomerReturn => Some(ApiReferenceType::CustomerReturn),
        LaborActivityKind::VendorReturn => Some(ApiReferenceType::VendorReturn),
        LaborActivityKind::ValueAddedWork => Some(ApiReferenceType::ValueAddedWorkOrder),
        LaborActivityKind::Break
        | LaborActivityKind::Meeting
        | LaborActivityKind::Training
        | LaborActivityKind::Maintenance
        | LaborActivityKind::Delay
        | LaborActivityKind::OtherIndirect => None,
    }
}

const fn activity_kind_name(value: ApiActivityKind) -> &'static str {
    match value {
        ApiActivityKind::Receiving => "receiving",
        ApiActivityKind::Putaway => "putaway",
        ApiActivityKind::Replenishment => "replenishment",
        ApiActivityKind::Picking => "picking",
        ApiActivityKind::Packing => "packing",
        ApiActivityKind::Shipping => "shipping",
        ApiActivityKind::CycleCount => "cycle_count",
        ApiActivityKind::InventoryRelocation => "inventory_relocation",
        ApiActivityKind::CrossDock => "cross_dock",
        ApiActivityKind::Yard => "yard",
        ApiActivityKind::CustomerReturn => "customer_return",
        ApiActivityKind::VendorReturn => "vendor_return",
        ApiActivityKind::ValueAddedWork => "value_added_work",
        ApiActivityKind::Break => "break",
        ApiActivityKind::Meeting => "meeting",
        ApiActivityKind::Training => "training",
        ApiActivityKind::Maintenance => "maintenance",
        ApiActivityKind::Delay => "delay",
        ApiActivityKind::OtherIndirect => "other_indirect",
    }
}

const fn quantity_basis_name(value: ApiQuantityBasis) -> &'static str {
    match value {
        ApiQuantityBasis::Unit => "unit",
        ApiQuantityBasis::Line => "line",
        ApiQuantityBasis::Container => "container",
        ApiQuantityBasis::Task => "task",
        ApiQuantityBasis::WeightGram => "weight_gram",
    }
}

#[cfg(test)]
mod tests {
    use wareboxes_api_contract::v1::PageLimit;

    use super::*;

    fn roster_request() -> LaborRosterPageRequest {
        LaborRosterPageRequest {
            facility_id: 7,
            inventory_owner_id: Some(9),
            limit: PageLimit::new(20).unwrap(),
            cursor: None,
        }
    }

    fn reference_request() -> LaborReferenceCandidatePageRequest {
        LaborReferenceCandidatePageRequest {
            facility_id: 7,
            inventory_owner_id: Some(9),
            employee_id: 11,
            activity_kind: ApiActivityKind::Picking,
            quantity_basis: ApiQuantityBasis::Unit,
            limit: PageLimit::new(20).unwrap(),
            cursor: None,
        }
    }

    #[test]
    fn labor_cursors_are_filter_bound() {
        let roster = roster_request();
        let cursor = encode_roster_cursor(EmployeeId::new(13).unwrap(), &roster).unwrap();
        assert_eq!(decode_roster_cursor(&cursor, &roster).unwrap().get(), 13);
        let mut changed_roster = roster_request();
        changed_roster.facility_id = 8;
        assert!(decode_roster_cursor(&cursor, &changed_roster).is_err());

        let reference = reference_request();
        let cursor = encode_reference_cursor(17, &reference).unwrap();
        assert_eq!(decode_reference_cursor(&cursor, &reference).unwrap(), 17);
        let mut changed_reference = reference_request();
        changed_reference.quantity_basis = ApiQuantityBasis::Line;
        assert!(decode_reference_cursor(&cursor, &changed_reference).is_err());
    }

    #[test]
    fn every_direct_kind_has_one_typed_reference() {
        for kind in [
            LaborActivityKind::Receiving,
            LaborActivityKind::Putaway,
            LaborActivityKind::Replenishment,
            LaborActivityKind::Picking,
            LaborActivityKind::Packing,
            LaborActivityKind::Shipping,
            LaborActivityKind::CycleCount,
            LaborActivityKind::InventoryRelocation,
            LaborActivityKind::CrossDock,
            LaborActivityKind::Yard,
            LaborActivityKind::CustomerReturn,
            LaborActivityKind::VendorReturn,
            LaborActivityKind::ValueAddedWork,
        ] {
            assert!(reference_type_for(kind).is_some());
        }
    }
}
