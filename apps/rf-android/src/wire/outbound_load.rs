use wareboxes_api_contract::v1::{
    LoadOutboundCartonRequest, MovePackedCartonResponse, StageOutboundCartonRequest,
    UnloadOutboundCartonRequest, UnstageOutboundCartonRequest,
};

use crate::outbound_load::OutboundLoadCommand;
use crate::workflow::CommandOutcome;

use super::{ResponseKind, WireRequestError, WireResponseError};

pub(super) fn build_command_parts(
    command: &OutboundLoadCommand,
) -> Result<(String, Vec<u8>, ResponseKind), WireRequestError> {
    let expected = command.expectation();
    let load_id = expected.load.outbound_load_id;
    let carton = expected
        .load
        .cartons
        .iter()
        .find(|carton| carton.carton_id == expected.carton_id)
        .ok_or(WireRequestError::InvalidOutboundLoadCommand)?;
    if load_id <= 0 || expected.carton_id <= 0 {
        return Err(WireRequestError::InvalidOutboundLoadCommand);
    }
    let path = |suffix: &str| {
        format!(
            "/api/v1/outbound-loads/{load_id}/cartons/{}/{suffix}",
            expected.carton_id
        )
    };
    let (path, body) = match command {
        OutboundLoadCommand::Stage {
            source_location_barcode,
            carton_barcode,
            staging_location_barcode,
            ..
        } => (
            path("staging-movements"),
            serde_json::to_vec(&StageOutboundCartonRequest {
                expected_load_revision: expected.load.revision,
                expected_position_revision: carton.position_revision,
                source_location_barcode: source_location_barcode.clone(),
                carton_barcode: carton_barcode.clone(),
                staging_location_barcode: staging_location_barcode.clone(),
            })?,
        ),
        OutboundLoadCommand::Load {
            staging_location_barcode,
            carton_barcode,
            trailer_number,
            ..
        } => (
            path("loading-movements"),
            serde_json::to_vec(&LoadOutboundCartonRequest {
                expected_load_revision: expected.load.revision,
                expected_position_revision: carton.position_revision,
                staging_location_barcode: staging_location_barcode.clone(),
                carton_barcode: carton_barcode.clone(),
                trailer_number: trailer_number.clone(),
            })?,
        ),
        OutboundLoadCommand::Unload {
            trailer_number,
            carton_barcode,
            staging_location_barcode,
            ..
        } => (
            path("unloading-movements"),
            serde_json::to_vec(&UnloadOutboundCartonRequest {
                expected_load_revision: expected.load.revision,
                expected_position_revision: carton.position_revision,
                trailer_number: trailer_number.clone(),
                carton_barcode: carton_barcode.clone(),
                staging_location_barcode: staging_location_barcode.clone(),
            })?,
        ),
        OutboundLoadCommand::Unstage {
            staging_location_barcode,
            carton_barcode,
            return_location_barcode,
            ..
        } => (
            path("unstaging-movements"),
            serde_json::to_vec(&UnstageOutboundCartonRequest {
                expected_load_revision: expected.load.revision,
                expected_position_revision: carton.position_revision,
                staging_location_barcode: staging_location_barcode.clone(),
                carton_barcode: carton_barcode.clone(),
                return_location_barcode: return_location_barcode.clone(),
            })?,
        ),
    };
    Ok((path, body, ResponseKind::OutboundCartonMovement))
}

pub(super) fn decode_response(body: &[u8]) -> Result<CommandOutcome, WireResponseError> {
    Ok(CommandOutcome::OutboundCartonMoved(Box::new(
        serde_json::from_slice::<MovePackedCartonResponse>(body)?,
    )))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::outbound_load::{
        OutboundCartonMovementExpectation, OutboundCartonOperation, example_outbound_load,
    };
    use crate::workflow::{DurableCommandDraft, RfCommand};

    use super::*;

    #[test]
    fn durable_stage_request_contains_only_exact_scans_and_revisions() {
        let load = example_outbound_load();
        let command = OutboundLoadCommand::Stage {
            expected: Box::new(OutboundCartonMovementExpectation {
                load: Box::new(load),
                carton_id: 601,
                operation: OutboundCartonOperation::Stage,
            }),
            source_location_barcode: "PACK-01".into(),
            carton_barcode: "CTN-00601".into(),
            staging_location_barcode: "STAGE-04".into(),
        };
        let draft = DurableCommandDraft {
            schema_version: 1,
            command_id: "command-1".into(),
            idempotency_key: "key-1".into(),
            command: RfCommand::OutboundLoad(command),
        };
        let request = super::super::build_durable_request(&draft)
            .unwrap_or_else(|error| panic!("valid durable request: {error}"));
        let restored = serde_json::from_slice::<DurableCommandDraft>(
            &serde_json::to_vec(&draft)
                .unwrap_or_else(|error| panic!("valid durable command: {error}")),
        )
        .unwrap_or_else(|error| panic!("durable command must restore: {error}"));
        let rebuilt = super::super::build_durable_request(&restored)
            .unwrap_or_else(|error| panic!("restored durable request: {error}"));

        assert_eq!(
            request.path,
            "/api/v1/outbound-loads/44/cartons/601/staging-movements"
        );
        assert_eq!(request.body, rebuilt.body);
        assert_eq!(request.body_sha256, rebuilt.body_sha256);
        assert_eq!(request.response_kind, ResponseKind::OutboundCartonMovement);
        let body: Value = serde_json::from_slice(&request.body)
            .unwrap_or_else(|error| panic!("request body must be JSON: {error}"));
        assert_eq!(
            body,
            serde_json::json!({
                "expected_load_revision": 3,
                "expected_position_revision": 1,
                "source_location_barcode": "PACK-01",
                "carton_barcode": "CTN-00601",
                "staging_location_barcode": "STAGE-04"
            })
        );
        assert!(body.get("quantity").is_none());
    }
}
