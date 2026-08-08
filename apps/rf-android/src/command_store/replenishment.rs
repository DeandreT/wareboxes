use crate::replenishment::ReplenishmentCommand;

use super::CommandOperation;

pub(super) const fn command_operation(command: &ReplenishmentCommand) -> CommandOperation {
    match command {
        ReplenishmentCommand::ClaimNext => CommandOperation::ClaimNext,
        ReplenishmentCommand::ClaimById { .. } => CommandOperation::ClaimById,
        ReplenishmentCommand::Confirm { .. } => CommandOperation::ReplenishmentConfirmation,
        ReplenishmentCommand::Release { .. } => CommandOperation::Release,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_replenishment_commands_to_stable_store_operations() {
        assert_eq!(
            command_operation(&ReplenishmentCommand::ClaimNext),
            CommandOperation::ClaimNext
        );
        assert_eq!(
            command_operation(&ReplenishmentCommand::ClaimById { work_id: 9 }),
            CommandOperation::ClaimById
        );
        assert_eq!(
            command_operation(&ReplenishmentCommand::Confirm {
                work_id: 9,
                expected: Box::new(crate::replenishment::ReplenishmentConfirmationExpectation {
                    plan_id: 2,
                    policy_id: 3,
                    source_inventory_balance_id: 4,
                    item_batch_id: 5,
                    item_id: 6,
                    uom: "each".into(),
                    lot: None,
                    serial: None,
                    source_location_id: 7,
                    destination_pick_face_location_id: 8,
                    quantity: 9,
                }),
                source_location_barcode: "RES-01".into(),
                item_barcode: "ITEM-01".into(),
                lot_scan: None,
                serial_scan: None,
                destination_pick_face_barcode: "PICK-01".into(),
            }),
            CommandOperation::ReplenishmentConfirmation
        );
        assert_eq!(
            command_operation(&ReplenishmentCommand::Release {
                work_id: 9,
                reason: crate::replenishment::ReplenishmentReleaseReason::WorkInterrupted,
                note: None,
            }),
            CommandOperation::Release
        );
    }
}
