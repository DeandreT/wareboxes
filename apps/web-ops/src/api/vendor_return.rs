use wareboxes_api_contract::v1::{
    CreateVendorReturnRequest, OpaqueCursor, VendorReturnLifecycleRequest,
    VendorReturnPageResponse, VendorReturnResponse, VendorReturnStatus,
};

use super::ApiError;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct VendorReturnFilters {
    pub facility_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
    pub status: Option<VendorReturnStatus>,
}

#[cfg(target_arch = "wasm32")]
pub async fn vendor_returns(
    filters: VendorReturnFilters,
    cursor: Option<&OpaqueCursor>,
) -> Result<VendorReturnPageResponse, ApiError> {
    super::browser::get(&page_path(filters, cursor)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn vendor_returns(
    _filters: VendorReturnFilters,
    _cursor: Option<&OpaqueCursor>,
) -> Result<VendorReturnPageResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn vendor_return_detail(id: i64) -> Result<VendorReturnResponse, ApiError> {
    super::browser::get(&format!("/api/v1/vendor-returns/{id}")).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn vendor_return_detail(_id: i64) -> Result<VendorReturnResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn create_vendor_return(
    request: &CreateVendorReturnRequest,
    key: &str,
) -> Result<VendorReturnResponse, ApiError> {
    super::browser::post("/api/v1/vendor-returns", request, key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn create_vendor_return(
    _request: &CreateVendorReturnRequest,
    _key: &str,
) -> Result<VendorReturnResponse, ApiError> {
    Err(ApiError::unavailable())
}

macro_rules! lifecycle {
    ($name:ident, $suffix:literal) => {
        #[cfg(target_arch = "wasm32")]
        pub async fn $name(
            id: i64,
            request: &VendorReturnLifecycleRequest,
            key: &str,
        ) -> Result<VendorReturnResponse, ApiError> {
            super::browser::post(
                &format!("/api/v1/vendor-returns/{id}/{}", $suffix),
                request,
                key,
            )
            .await
        }

        #[cfg(not(target_arch = "wasm32"))]
        pub async fn $name(
            _id: i64,
            _request: &VendorReturnLifecycleRequest,
            _key: &str,
        ) -> Result<VendorReturnResponse, ApiError> {
            Err(ApiError::unavailable())
        }
    };
}

lifecycle!(release_vendor_return, "releases");
lifecycle!(ship_vendor_return, "shipments");
lifecycle!(cancel_vendor_return, "cancellations");

#[cfg(any(target_arch = "wasm32", test))]
fn page_path(filters: VendorReturnFilters, cursor: Option<&OpaqueCursor>) -> String {
    let mut path = "/api/v1/vendor-returns?limit=100".to_owned();
    if let Some(owner_id) = filters.inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={owner_id}"));
    }
    if let Some(facility_id) = filters.facility_id {
        path.push_str(&format!("&facility_id={facility_id}"));
    }
    if let Some(status) = filters.status {
        path.push_str("&status=");
        path.push_str(status_wire(status));
    }
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
const fn status_wire(value: VendorReturnStatus) -> &'static str {
    match value {
        VendorReturnStatus::Draft => "draft",
        VendorReturnStatus::Released => "released",
        VendorReturnStatus::Shipped => "shipped",
        VendorReturnStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_path_binds_scope_status_and_cursor() {
        let cursor = OpaqueCursor::new("vr1.scope+/=").unwrap();
        assert_eq!(
            page_path(
                VendorReturnFilters {
                    facility_id: Some(7),
                    inventory_owner_id: Some(9),
                    status: Some(VendorReturnStatus::Released),
                },
                Some(&cursor),
            ),
            "/api/v1/vendor-returns?limit=100&inventory_owner_id=9&facility_id=7&status=released&cursor=vr1.scope%2B%2F%3D"
        );
    }
}
