use wareboxes_api_contract::v1::{
    CreateInboundAsnRequest, CreateInboundAsnResponse, InboundAsnDetailResponse, InboundAsnPage,
    InboundAsnStatus, OpaqueCursor, PlanInboundAsnLoadRequest, PlanInboundAsnLoadResponse,
};

use super::ApiError;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct InboundAsnFilters {
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub status: Option<InboundAsnStatus>,
    pub search: Option<String>,
}

#[cfg(target_arch = "wasm32")]
pub async fn inbound_asns(
    filters: InboundAsnFilters,
    cursor: Option<&OpaqueCursor>,
) -> Result<InboundAsnPage, ApiError> {
    super::browser::get(&inbound_asn_page_path(&filters, cursor)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn inbound_asns(
    _filters: InboundAsnFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<InboundAsnPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn inbound_asn_detail(asn_id: i64) -> Result<InboundAsnDetailResponse, ApiError> {
    super::browser::get(&format!("/api/v1/inbound-asns/{asn_id}")).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn inbound_asn_detail(_asn_id: i64) -> Result<InboundAsnDetailResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_inbound_asn(
    request: &CreateInboundAsnRequest,
    idempotency_key: &str,
) -> Result<CreateInboundAsnResponse, ApiError> {
    super::browser::post("/api/v1/inbound-asns", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_inbound_asn(
    _request: &CreateInboundAsnRequest,
    _idempotency_key: &str,
) -> Result<CreateInboundAsnResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn plan_inbound_asn_load(
    asn_id: i64,
    request: &PlanInboundAsnLoadRequest,
    idempotency_key: &str,
) -> Result<PlanInboundAsnLoadResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/inbound-asns/{asn_id}/load-plans"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn plan_inbound_asn_load(
    _asn_id: i64,
    _request: &PlanInboundAsnLoadRequest,
    _idempotency_key: &str,
) -> Result<PlanInboundAsnLoadResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn inbound_asn_page_path(filters: &InboundAsnFilters, cursor: Option<&OpaqueCursor>) -> String {
    let mut path = "/api/v1/inbound-asns?limit=100".to_owned();
    append_id(&mut path, "facility_id", filters.facility_id);
    append_id(&mut path, "inventory_owner_id", filters.inventory_owner_id);
    if let Some(status) = filters.status {
        path.push_str("&status=");
        path.push_str(match status {
            InboundAsnStatus::Open => "open",
            InboundAsnStatus::Planned => "planned",
        });
    }
    if let Some(search) = filters.search.as_deref().filter(|value| !value.is_empty()) {
        path.push_str("&search=");
        path.push_str(&urlencoding::encode(search));
    }
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn append_id(path: &mut String, name: &str, value: Option<i64>) {
    if let Some(value) = value {
        path.push('&');
        path.push_str(name);
        path.push('=');
        path.push_str(&value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_path_binds_filters_and_cursor() {
        let cursor = OpaqueCursor::new("ia1.cursor+/=").unwrap();
        let path = inbound_asn_page_path(
            &InboundAsnFilters {
                facility_id: Some(7),
                inventory_owner_id: Some(9),
                status: Some(InboundAsnStatus::Open),
                search: Some("ASN 100/2".into()),
            },
            Some(&cursor),
        );
        assert_eq!(
            path,
            "/api/v1/inbound-asns?limit=100&facility_id=7&inventory_owner_id=9&status=open&search=ASN%20100%2F2&cursor=ia1.cursor%2B%2F%3D"
        );
    }
}
