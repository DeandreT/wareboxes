use crate::wire::ResponseKind;

use super::CommandStoreError;

pub(super) const fn name(kind: ResponseKind) -> &'static str {
    match kind {
        ResponseKind::OptionalClaim => "optional_claim",
        ResponseKind::Claim => "claim",
        ResponseKind::LooseConfirmation => "loose_confirmation",
        ResponseKind::LicensePlateConfirmation => "license_plate_confirmation",
        ResponseKind::Release => "release",
        ResponseKind::RelocationOptionalClaim => "relocation_optional_claim",
        ResponseKind::RelocationClaim => "relocation_claim",
        ResponseKind::RelocationConfirmation => "relocation_confirmation",
        ResponseKind::RelocationRelease => "relocation_release",
        ResponseKind::CycleCountOptionalClaim => "cycle_count_optional_claim",
        ResponseKind::CycleCountClaim => "cycle_count_claim",
        ResponseKind::CycleCountConfirmation => "cycle_count_confirmation",
        ResponseKind::CycleCountRelease => "cycle_count_release",
        ResponseKind::PickOptionalClaim => "pick_optional_claim",
        ResponseKind::PickClaim => "pick_claim",
        ResponseKind::PickConfirmation => "pick_confirmation",
        ResponseKind::PickShortageReport => "pick_shortage_report",
        ResponseKind::PickRelease => "pick_release",
        ResponseKind::ReplenishmentOptionalClaim => "replenishment_optional_claim",
        ResponseKind::ReplenishmentClaim => "replenishment_claim",
        ResponseKind::ReplenishmentConfirmation => "replenishment_confirmation",
        ResponseKind::ReplenishmentRelease => "replenishment_release",
        ResponseKind::OutboundCartonMovement => "outbound_carton_movement",
        ResponseKind::ExpectedReceiptConfirmation => "expected_receipt_confirmation",
        ResponseKind::UnexpectedReceiptConfirmation => "unexpected_receipt_confirmation",
    }
}

pub(super) fn parse(value: &str) -> Result<ResponseKind, CommandStoreError> {
    match value {
        "optional_claim" => Ok(ResponseKind::OptionalClaim),
        "claim" => Ok(ResponseKind::Claim),
        "loose_confirmation" => Ok(ResponseKind::LooseConfirmation),
        "license_plate_confirmation" => Ok(ResponseKind::LicensePlateConfirmation),
        "release" => Ok(ResponseKind::Release),
        "relocation_optional_claim" => Ok(ResponseKind::RelocationOptionalClaim),
        "relocation_claim" => Ok(ResponseKind::RelocationClaim),
        "relocation_confirmation" => Ok(ResponseKind::RelocationConfirmation),
        "relocation_release" => Ok(ResponseKind::RelocationRelease),
        "cycle_count_optional_claim" => Ok(ResponseKind::CycleCountOptionalClaim),
        "cycle_count_claim" => Ok(ResponseKind::CycleCountClaim),
        "cycle_count_confirmation" => Ok(ResponseKind::CycleCountConfirmation),
        "cycle_count_release" => Ok(ResponseKind::CycleCountRelease),
        "pick_optional_claim" => Ok(ResponseKind::PickOptionalClaim),
        "pick_claim" => Ok(ResponseKind::PickClaim),
        "pick_confirmation" => Ok(ResponseKind::PickConfirmation),
        "pick_shortage_report" => Ok(ResponseKind::PickShortageReport),
        "pick_release" => Ok(ResponseKind::PickRelease),
        "replenishment_optional_claim" => Ok(ResponseKind::ReplenishmentOptionalClaim),
        "replenishment_claim" => Ok(ResponseKind::ReplenishmentClaim),
        "replenishment_confirmation" => Ok(ResponseKind::ReplenishmentConfirmation),
        "replenishment_release" => Ok(ResponseKind::ReplenishmentRelease),
        "outbound_carton_movement" => Ok(ResponseKind::OutboundCartonMovement),
        "expected_receipt_confirmation" => Ok(ResponseKind::ExpectedReceiptConfirmation),
        "unexpected_receipt_confirmation" => Ok(ResponseKind::UnexpectedReceiptConfirmation),
        _ => Err(CommandStoreError::CorruptRecord(
            "unknown response kind".into(),
        )),
    }
}
