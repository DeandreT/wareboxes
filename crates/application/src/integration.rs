use wareboxes_domain::{FacilityId, InventoryOwnerId, TenantId, Timestamp};

pub struct NewIntegrationInboxReceipt<'a> {
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub source_key: &'a str,
    pub deduplication_key: &'a str,
    pub content_type: &'a str,
    pub raw_payload: &'a [u8],
    pub request_id: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegrationInboxReadScope {
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationInboxReceipt {
    pub id: i64,
    pub tenant_id: TenantId,
    pub inventory_owner_id: Option<InventoryOwnerId>,
    pub facility_id: Option<FacilityId>,
    pub received_at: Timestamp,
    pub source_key: String,
    pub deduplication_key: String,
    pub content_type: String,
    pub raw_payload: Vec<u8>,
    pub payload_sha256: Vec<u8>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveIntegrationInboxResult {
    pub receipt: IntegrationInboxReceipt,
    pub replayed: bool,
}
