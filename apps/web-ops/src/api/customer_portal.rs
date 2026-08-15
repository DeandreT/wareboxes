use wareboxes_api_contract::v1::CustomerPortalWorkspaceResponse;

use super::ApiError;

#[derive(Clone, Default)]
pub struct CustomerPortalFilters {
    pub inventory_owner_id: Option<i64>,
    pub facility_id: Option<i64>,
    pub search: Option<String>,
    pub include_history: bool,
}

#[cfg(target_arch = "wasm32")]
pub async fn customer_portal_workspace(
    filters: CustomerPortalFilters,
) -> Result<CustomerPortalWorkspaceResponse, ApiError> {
    super::browser::get(&customer_portal_path(&filters)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn customer_portal_workspace(
    _filters: CustomerPortalFilters,
) -> Result<CustomerPortalWorkspaceResponse, ApiError> {
    Err(ApiError::unavailable())
}

pub fn customer_portal_inventory_report_path(filters: &CustomerPortalFilters) -> String {
    filter_path("/api/v1/portal/reports/inventory.csv", filters)
}

#[cfg(any(target_arch = "wasm32", test))]
fn customer_portal_path(filters: &CustomerPortalFilters) -> String {
    filter_path("/api/v1/portal/workspace", filters)
}

fn filter_path(root: &str, filters: &CustomerPortalFilters) -> String {
    let mut parameters = vec![format!("include_history={}", filters.include_history)];
    if let Some(owner_id) = filters.inventory_owner_id {
        parameters.push(format!("inventory_owner_id={owner_id}"));
    }
    if let Some(facility_id) = filters.facility_id {
        parameters.push(format!("facility_id={facility_id}"));
    }
    if let Some(search) = filters.search.as_deref().filter(|value| !value.is_empty()) {
        parameters.push(format!("search={}", urlencoding::encode(search)));
    }
    format!("{root}?{}", parameters.join("&"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_bind_scope_history_and_encoded_search() {
        let filters = CustomerPortalFilters {
            inventory_owner_id: Some(5),
            facility_id: Some(8),
            search: Some("SO 10/2".into()),
            include_history: true,
        };
        assert_eq!(
            customer_portal_path(&filters),
            "/api/v1/portal/workspace?include_history=true&inventory_owner_id=5&facility_id=8&search=SO%2010%2F2"
        );
        assert!(customer_portal_inventory_report_path(&filters)
            .starts_with("/api/v1/portal/reports/inventory.csv?include_history=true"));
    }
}
