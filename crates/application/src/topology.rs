use wareboxes_domain::{TenantId, Timestamp};

/// Facility data projected for application queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacilityReadModel {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub name: Option<String>,
    pub address_id: Option<i64>,
    pub revision: i64,
}

/// Location data projected for application queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationReadModel {
    pub id: i64,
    pub tenant_id: TenantId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub facility_id: i64,
    pub facility_name: Option<String>,
    pub parent_location_id: Option<i64>,
    pub barcode: Option<String>,
    pub name: Option<String>,
    pub r#type: String,
    pub active: bool,
    pub pickable: bool,
    pub receivable: bool,
    pub storage_zone_id: Option<i64>,
    pub storage_zone_code: Option<String>,
    pub storage_zone_name: Option<String>,
    pub storage_zone_purpose: Option<String>,
    pub storage_zone_travel_sequence: Option<i64>,
}
