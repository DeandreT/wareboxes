use serde::{Deserialize, Serialize};

/// Credentials used to create an authenticated RF operator session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRfSessionRequest {
    pub email: String,
    pub password: String,
}

/// Facilities the RF operator may access in the selected tenant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RfSessionSiteScope {
    pub all_facilities: bool,
    pub facility_ids: Vec<i64>,
}

/// Inventory owners the RF operator may access in the selected tenant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RfSessionOwnerScope {
    pub all_inventory_owners: bool,
    pub inventory_owner_ids: Vec<i64>,
}

/// Active tenant context established for an RF operator session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RfSessionTenant {
    pub tenant_id: i64,
    pub name: String,
    pub site_scope: RfSessionSiteScope,
    pub owner_scope: RfSessionOwnerScope,
}

/// Opaque bearer credential and authorization context for an RF session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRfSessionResponse {
    pub token: String,
    pub operator_id: i64,
    pub tenant: RfSessionTenant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rf_session_response_has_an_exact_public_contract() {
        let response = CreateRfSessionResponse {
            token: "opaque-session-token".into(),
            operator_id: 17,
            tenant: RfSessionTenant {
                tenant_id: 23,
                name: "Northwest Operations".into(),
                site_scope: RfSessionSiteScope {
                    all_facilities: false,
                    facility_ids: vec![31, 32],
                },
                owner_scope: RfSessionOwnerScope {
                    all_inventory_owners: false,
                    inventory_owner_ids: vec![41],
                },
            },
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            serde_json::json!({
                "token": "opaque-session-token",
                "operator_id": 17,
                "tenant": {
                    "tenant_id": 23,
                    "name": "Northwest Operations",
                    "site_scope": {
                        "all_facilities": false,
                        "facility_ids": [31, 32],
                    },
                    "owner_scope": {
                        "all_inventory_owners": false,
                        "inventory_owner_ids": [41],
                    },
                },
            })
        );
    }

    #[test]
    fn rf_session_request_rejects_unknown_fields() {
        let request = serde_json::json!({
            "email": "operator@example.com",
            "password": "secret",
            "tenant_id": 23,
        });

        assert!(serde_json::from_value::<CreateRfSessionRequest>(request).is_err());
    }
}
