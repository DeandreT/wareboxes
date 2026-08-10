use wareboxes_api_contract::v1::{
    ConfigureItemStoragePolicyRequest, ItemStoragePolicyPage, ItemStoragePolicyResponse,
    ItemStoragePolicyStatus, OpaqueCursor, RetireItemStoragePolicyRequest, StorageZonePurpose,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn item_storage_policies(
    inventory_owner_id: Option<i64>,
    facility_id: Option<i64>,
    item_id: Option<i64>,
    purpose: Option<StorageZonePurpose>,
    status: ItemStoragePolicyStatus,
    cursor: Option<&OpaqueCursor>,
) -> Result<ItemStoragePolicyPage, ApiError> {
    super::browser::get(&page_path(
        inventory_owner_id,
        facility_id,
        item_id,
        purpose,
        status,
        cursor,
    ))
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn item_storage_policies(
    _inventory_owner_id: Option<i64>,
    _facility_id: Option<i64>,
    _item_id: Option<i64>,
    _purpose: Option<StorageZonePurpose>,
    _status: ItemStoragePolicyStatus,
    _cursor: Option<&OpaqueCursor>,
) -> Result<ItemStoragePolicyPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn configure_item_storage_policy(
    request: &ConfigureItemStoragePolicyRequest,
    idempotency_key: &str,
) -> Result<ItemStoragePolicyResponse, ApiError> {
    super::browser::post("/api/v1/item-storage-policies", request, idempotency_key).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn configure_item_storage_policy(
    _request: &ConfigureItemStoragePolicyRequest,
    _idempotency_key: &str,
) -> Result<ItemStoragePolicyResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn retire_item_storage_policy(
    item_storage_policy_id: i64,
    request: &RetireItemStoragePolicyRequest,
    idempotency_key: &str,
) -> Result<ItemStoragePolicyResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/item-storage-policies/{item_storage_policy_id}/retirements"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn retire_item_storage_policy(
    _item_storage_policy_id: i64,
    _request: &RetireItemStoragePolicyRequest,
    _idempotency_key: &str,
) -> Result<ItemStoragePolicyResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn page_path(
    inventory_owner_id: Option<i64>,
    facility_id: Option<i64>,
    item_id: Option<i64>,
    purpose: Option<StorageZonePurpose>,
    status: ItemStoragePolicyStatus,
    cursor: Option<&OpaqueCursor>,
) -> String {
    let mut path = format!(
        "/api/v1/item-storage-policies?limit=100&status={}",
        status_wire(status)
    );
    for (name, value) in [
        ("inventory_owner_id", inventory_owner_id),
        ("facility_id", facility_id),
        ("item_id", item_id),
    ] {
        if let Some(value) = value {
            path.push_str(&format!("&{name}={value}"));
        }
    }
    if let Some(purpose) = purpose {
        path.push_str("&purpose=");
        path.push_str(purpose_wire(purpose));
    }
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(cursor.as_str());
    }
    path
}

#[cfg(any(target_arch = "wasm32", test))]
const fn status_wire(value: ItemStoragePolicyStatus) -> &'static str {
    match value {
        ItemStoragePolicyStatus::Active => "active",
        ItemStoragePolicyStatus::Retired => "retired",
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn purpose_wire(value: StorageZonePurpose) -> &'static str {
    match value {
        StorageZonePurpose::Receiving => "receiving",
        StorageZonePurpose::Reserve => "reserve",
        StorageZonePurpose::Pick => "pick",
        StorageZonePurpose::Staging => "staging",
        StorageZonePurpose::Packing => "packing",
        StorageZonePurpose::Shipping => "shipping",
        StorageZonePurpose::Quarantine => "quarantine",
        StorageZonePurpose::Damage => "damage",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_path_binds_every_filter_to_the_cursor_request() {
        let cursor = OpaqueCursor::new("isp1.filter.0000000000000001").unwrap();
        assert_eq!(
            page_path(
                Some(2),
                Some(3),
                Some(4),
                Some(StorageZonePurpose::Pick),
                ItemStoragePolicyStatus::Retired,
                Some(&cursor),
            ),
            "/api/v1/item-storage-policies?limit=100&status=retired&inventory_owner_id=2&facility_id=3&item_id=4&purpose=pick&cursor=isp1.filter.0000000000000001"
        );
    }
}
