use crate::cross_dock::CrossDockCommand;

use super::CommandOperation;

pub(super) const fn command_operation(command: &CrossDockCommand) -> CommandOperation {
    match command {
        CrossDockCommand::ClaimNext => CommandOperation::ClaimNext,
        CrossDockCommand::ClaimById { .. } => CommandOperation::ClaimById,
        CrossDockCommand::Confirm { .. } => CommandOperation::CrossDockConfirmation,
        CrossDockCommand::Release { .. } => CommandOperation::Release,
    }
}
