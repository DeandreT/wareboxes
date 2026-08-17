//! Durable carrier-gateway configuration and manifest-job contracts.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;
use wareboxes_domain::{
    CarrierAccountId, CarrierAccountKey, CarrierAccountName, CarrierAccountStatus, CarrierCode,
    CarrierFailureCode, CarrierFailureMessage, CarrierManifestId, CarrierManifestJobId,
    CarrierManifestJobStatus, CarrierServiceCode, CartonId, FacilityId, InventoryOwnerId,
    ManifestReference, ShipmentId, ShipmentRevision, TenantId, Timestamp, TrackingNumber, UserId,
};

pub const CREATE_CARRIER_ACCOUNT_OPERATION: &str = "carrier.account.create.v1";
pub const RECONFIGURE_CARRIER_ACCOUNT_OPERATION: &str = "carrier.account.reconfigure.v1";
pub const CHANGE_CARRIER_ACCOUNT_STATUS_OPERATION: &str = "carrier.account.status.change.v1";
pub const QUEUE_CARRIER_MANIFEST_OPERATION: &str = "carrier.manifest.queue.v1";
pub const CANCEL_CARRIER_MANIFEST_OPERATION: &str = "carrier.manifest.cancel.v1";
pub const RETRY_CARRIER_MANIFEST_OPERATION: &str = "carrier.manifest.retry.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierAccountReadModel {
    pub account_id: CarrierAccountId,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub display_name: CarrierAccountName,
    pub carrier_code: CarrierCode,
    pub account_key: CarrierAccountKey,
    pub status: CarrierAccountStatus,
    pub revision: u32,
    pub configured_by: UserId,
    pub configured_at: Timestamp,
    pub updated_by: UserId,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateCarrierAccountCommand {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub display_name: CarrierAccountName,
    pub carrier_code: CarrierCode,
    pub account_key: CarrierAccountKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReconfigureCarrierAccountCommand {
    pub account_id: CarrierAccountId,
    pub display_name: CarrierAccountName,
    pub account_key: CarrierAccountKey,
    pub expected_revision: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangeCarrierAccountStatusCommand {
    pub account_id: CarrierAccountId,
    pub status: CarrierAccountStatus,
    pub expected_revision: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierAddressSnapshot {
    pub name: Option<String>,
    pub company: Option<String>,
    pub line1: String,
    pub line2: Option<String>,
    pub postal_code: String,
    pub country: String,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub state: Option<String>,
    pub county: Option<String>,
    pub city: String,
    pub territory: Option<String>,
    pub district: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierPackageSnapshot {
    pub carton_id: CartonId,
    pub carton_barcode: String,
    pub weight_grams: i64,
    pub length_millimeters: Option<i64>,
    pub width_millimeters: Option<i64>,
    pub height_millimeters: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierManifestAdapterRequest {
    pub schema_version: u16,
    pub request_key: String,
    pub tenant_id: TenantId,
    pub account_key: CarrierAccountKey,
    pub carrier_code: CarrierCode,
    pub service_code: Option<CarrierServiceCode>,
    pub shipment_id: ShipmentId,
    pub origin: CarrierAddressSnapshot,
    pub destination: CarrierAddressSnapshot,
    pub packages: Vec<CarrierPackageSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierPackageManifestResult {
    pub carton_id: CartonId,
    pub tracking_number: TrackingNumber,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierManifestAdapterResponse {
    pub schema_version: u16,
    pub request_key: String,
    pub manifest_reference: ManifestReference,
    pub packages: Vec<CarrierPackageManifestResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueueCarrierManifestCommand {
    pub shipment_id: ShipmentId,
    pub account_id: CarrierAccountId,
    pub service_code: Option<CarrierServiceCode>,
    pub expected_shipment_revision: ShipmentRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelCarrierManifestCommand {
    pub shipment_id: ShipmentId,
    pub job_id: CarrierManifestJobId,
    pub expected_revision: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetryCarrierManifestCommand {
    pub shipment_id: ShipmentId,
    pub job_id: CarrierManifestJobId,
    pub expected_revision: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierManifestJobReadModel {
    pub job_id: CarrierManifestJobId,
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub shipment_id: ShipmentId,
    pub account_id: CarrierAccountId,
    pub account_revision: u32,
    pub account_key: CarrierAccountKey,
    pub carrier_code: CarrierCode,
    pub service_code: Option<CarrierServiceCode>,
    pub request_key: String,
    pub request_sha256: String,
    pub status: CarrierManifestJobStatus,
    pub revision: u32,
    pub attempt_count: u32,
    pub next_attempt_at: Option<Timestamp>,
    pub last_error_code: Option<CarrierFailureCode>,
    pub last_error_message: Option<CarrierFailureMessage>,
    pub manifest_id: Option<CarrierManifestId>,
    pub manifest_reference: Option<ManifestReference>,
    pub requested_by: UserId,
    pub requested_at: Timestamp,
    pub completed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueCarrierManifestResult {
    pub job: CarrierManifestJobReadModel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierManifestClaim {
    pub job: CarrierManifestJobReadModel,
    pub claim_version: u32,
    pub request: CarrierManifestAdapterRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CarrierAdapterValidationError {
    #[error("carrier response uses an unsupported schema version")]
    UnsupportedSchema,
    #[error("carrier response request key does not match the immutable request")]
    RequestKeyMismatch,
    #[error("carrier response package set does not exactly match the shipment cartons")]
    PackageSetMismatch,
    #[error("carrier response reuses a tracking number")]
    DuplicateTrackingNumber,
}

pub fn validate_carrier_response(
    request: &CarrierManifestAdapterRequest,
    response: &CarrierManifestAdapterResponse,
) -> Result<(), CarrierAdapterValidationError> {
    if request.schema_version != 1 || response.schema_version != 1 {
        return Err(CarrierAdapterValidationError::UnsupportedSchema);
    }
    if response.request_key != request.request_key {
        return Err(CarrierAdapterValidationError::RequestKeyMismatch);
    }
    let requested = request
        .packages
        .iter()
        .map(|package| package.carton_id)
        .collect::<HashSet<_>>();
    let returned = response
        .packages
        .iter()
        .map(|package| package.carton_id)
        .collect::<HashSet<_>>();
    if requested.len() != request.packages.len()
        || returned.len() != response.packages.len()
        || requested != returned
    {
        return Err(CarrierAdapterValidationError::PackageSetMismatch);
    }
    let tracking = response
        .packages
        .iter()
        .map(|package| package.tracking_number.as_str())
        .collect::<HashSet<_>>();
    if tracking.len() != response.packages.len() {
        return Err(CarrierAdapterValidationError::DuplicateTrackingNumber);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CarrierManifestAdapterRequest {
        CarrierManifestAdapterRequest {
            schema_version: 1,
            request_key: "request-1".into(),
            tenant_id: TenantId::new(1).unwrap(),
            account_key: CarrierAccountKey::new("account-1").unwrap(),
            carrier_code: CarrierCode::new("CARRIER").unwrap(),
            service_code: None,
            shipment_id: ShipmentId::new(2).unwrap(),
            origin: CarrierAddressSnapshot {
                name: Some("Origin".into()),
                company: None,
                line1: "1 Origin".into(),
                line2: None,
                postal_code: "11111".into(),
                country: "US".into(),
                phone: None,
                email: None,
                state: Some("CA".into()),
                county: None,
                city: "Origin".into(),
                territory: None,
                district: None,
            },
            destination: CarrierAddressSnapshot {
                name: Some("Destination".into()),
                company: None,
                line1: "2 Destination".into(),
                line2: None,
                postal_code: "22222".into(),
                country: "US".into(),
                phone: None,
                email: None,
                state: Some("OR".into()),
                county: None,
                city: "Destination".into(),
                territory: None,
                district: None,
            },
            packages: vec![CarrierPackageSnapshot {
                carton_id: CartonId::new(3).unwrap(),
                carton_barcode: "C-3".into(),
                weight_grams: 100,
                length_millimeters: None,
                width_millimeters: None,
                height_millimeters: None,
            }],
        }
    }

    #[test]
    fn adapter_response_must_cover_each_carton_once() {
        let request = request();
        let response = CarrierManifestAdapterResponse {
            schema_version: 1,
            request_key: request.request_key.clone(),
            manifest_reference: ManifestReference::new("M-1").unwrap(),
            packages: vec![CarrierPackageManifestResult {
                carton_id: CartonId::new(3).unwrap(),
                tracking_number: TrackingNumber::new("T-1").unwrap(),
            }],
        };
        assert!(validate_carrier_response(&request, &response).is_ok());
        let mut duplicate = response;
        duplicate.packages.push(duplicate.packages[0].clone());
        assert_eq!(
            validate_carrier_response(&request, &duplicate),
            Err(CarrierAdapterValidationError::PackageSetMismatch)
        );
    }
}
