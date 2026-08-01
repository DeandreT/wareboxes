use serde::{Deserialize, Serialize};

/// A facility or inventory owner available in the active access scope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AccessScopeResource {
    pub id: i64,
    pub name: String,
}

/// Facilities and inventory owners visible in the active tenant context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct AccessScopeWorkspace {
    pub facilities: Vec<AccessScopeResource>,
    pub inventory_owners: Vec<AccessScopeResource>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_workspace_has_an_exact_web_contract() {
        let workspace = AccessScopeWorkspace {
            facilities: vec![AccessScopeResource {
                id: 17,
                name: "Reno DC".into(),
            }],
            inventory_owners: vec![AccessScopeResource {
                id: 23,
                name: "Northwind".into(),
            }],
        };

        let json = serde_json::to_string(&workspace).unwrap();
        assert_eq!(
            json,
            r#"{"facilities":[{"id":17,"name":"Reno DC"}],"inventory_owners":[{"id":23,"name":"Northwind"}]}"#
        );
        assert_eq!(
            serde_json::from_str::<AccessScopeWorkspace>(&json).unwrap(),
            workspace
        );
    }

    #[test]
    fn access_workspace_rejects_unknown_fields() {
        assert!(serde_json::from_str::<AccessScopeWorkspace>(
            r#"{"facilities":[],"inventory_owners":[],"tenant_id":7}"#,
        )
        .is_err());
        assert!(serde_json::from_str::<AccessScopeWorkspace>(
            r#"{"facilities":[{"id":17,"name":"Reno DC","deleted":null}],"inventory_owners":[]}"#,
        )
        .is_err());
    }
}
