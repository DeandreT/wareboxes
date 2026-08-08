use serde::{Deserialize, Serialize};

use super::Revision;

/// Complete carrier-facing origin accepted for one facility revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureFacilityShippingOriginRequest {
    pub expected_revision: Revision,
    pub name: Option<String>,
    pub company: Option<String>,
    pub line1: String,
    pub line2: Option<String>,
    pub city: String,
    pub state: Option<String>,
    pub postal_code: String,
    pub country: String,
    pub phone: Option<String>,
    pub email: Option<String>,
}

/// Snapshotted facility origin returned by configuration commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FacilityShippingOriginResponse {
    pub name: Option<String>,
    pub company: Option<String>,
    pub line1: String,
    pub line2: Option<String>,
    pub city: String,
    pub state: Option<String>,
    pub postal_code: String,
    pub country: String,
    pub phone: Option<String>,
    pub email: Option<String>,
}

/// Replay-stable facility shipping-origin configuration result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureFacilityShippingOriginResponse {
    pub configuration_id: i64,
    pub facility_id: i64,
    pub address_id: i64,
    pub revision: Revision,
    pub origin: FacilityShippingOriginResponse,
    pub configured_by: i64,
    pub configured_at: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn request_is_strict_and_requires_a_positive_revision() {
        let value = json!({
            "expected_revision": 1,
            "name": null,
            "company": "Wareboxes Fulfillment",
            "line1": "100 Distribution Way",
            "line2": null,
            "city": "Reno",
            "state": null,
            "postal_code": "89502",
            "country": "US",
            "phone": null,
            "email": null
        });
        assert_eq!(
            serde_json::from_value::<ConfigureFacilityShippingOriginRequest>(value)
                .unwrap()
                .expected_revision
                .get(),
            1
        );
        assert!(
            serde_json::from_value::<ConfigureFacilityShippingOriginRequest>(json!({
                "expected_revision": 0,
                "name": "Shipping",
                "company": null,
                "line1": "100 Distribution Way",
                "line2": null,
                "city": "Reno",
                "state": null,
                "postal_code": "89502",
                "country": "US",
                "phone": null,
                "email": null
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ConfigureFacilityShippingOriginRequest>(json!({
                "expected_revision": 1,
                "name": "Shipping",
                "company": null,
                "line1": "100 Distribution Way",
                "line2": null,
                "city": "Reno",
                "state": null,
                "postal_code": "89502",
                "country": "US",
                "phone": null,
                "email": null,
                "force": true
            }))
            .is_err()
        );
    }
}
