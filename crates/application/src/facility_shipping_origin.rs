//! Replay-safe facility shipping-origin configuration contracts.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{
    AddressId, FacilityId, FacilityRevision, FacilityShippingOrigin,
    FacilityShippingOriginConfigurationId, Timestamp, UserId,
};

pub const FACILITY_SHIPPING_ORIGIN_CONFIGURE_OPERATION: &str =
    "facility.shipping_origin.configure.v1";

/// Replaces one facility's carrier-facing origin at an observed revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigureFacilityShippingOriginCommand {
    facility_id: FacilityId,
    expected_revision: FacilityRevision,
    origin: FacilityShippingOrigin,
}

impl ConfigureFacilityShippingOriginCommand {
    pub const fn new(
        facility_id: FacilityId,
        expected_revision: FacilityRevision,
        origin: FacilityShippingOrigin,
    ) -> Self {
        Self {
            facility_id,
            expected_revision,
            origin,
        }
    }

    pub const fn facility_id(&self) -> FacilityId {
        self.facility_id
    }

    pub const fn expected_revision(&self) -> FacilityRevision {
        self.expected_revision
    }

    pub const fn origin(&self) -> &FacilityShippingOrigin {
        &self.origin
    }
}

/// Stable result retained for exact command replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureFacilityShippingOriginResult {
    pub configuration_id: FacilityShippingOriginConfigurationId,
    pub facility_id: FacilityId,
    pub address_id: AddressId,
    pub revision: FacilityRevision,
    pub origin: FacilityShippingOrigin,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn origin() -> FacilityShippingOrigin {
        FacilityShippingOrigin::new(
            Some("West shipping office".into()),
            Some("Wareboxes Fulfillment".into()),
            "100 Distribution Way".into(),
            Some("Dock 20".into()),
            "Reno".into(),
            Some("NV".into()),
            "89502".into(),
            "US".into(),
            Some("+1 775 555 0100".into()),
            Some("shipping@example.com".into()),
        )
        .unwrap()
    }

    #[test]
    fn command_hash_shape_includes_path_identity_revision_and_complete_origin() {
        let command = ConfigureFacilityShippingOriginCommand::new(
            FacilityId::new(7).unwrap(),
            FacilityRevision::new(3).unwrap(),
            origin(),
        );
        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "facility_id": 7,
                "expected_revision": 3,
                "origin": {
                    "name": "West shipping office",
                    "company": "Wareboxes Fulfillment",
                    "line1": "100 Distribution Way",
                    "line2": "Dock 20",
                    "city": "Reno",
                    "state": "NV",
                    "postal_code": "89502",
                    "country": "US",
                    "phone": "+1 775 555 0100",
                    "email": "shipping@example.com"
                }
            })
        );
    }
}
