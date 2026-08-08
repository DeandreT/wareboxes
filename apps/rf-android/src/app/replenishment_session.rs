use eframe::egui;

use crate::command_store::{CommandStatus, DurableCommandRecord};
use crate::workflow::{Activity, ClaimOperation};

use super::{RfApp, SessionGate, WorkMode};

impl RfApp {
    pub(super) fn restore_replenishment_command(
        &mut self,
        context: &egui::Context,
        record: DurableCommandRecord,
    ) {
        self.work_mode = WorkMode::Replenish;
        match record.status {
            CommandStatus::Persisted => {
                let transition = self
                    .replenishment
                    .restore_ready_command(record.record_id, record.draft);
                self.emit_replenishment_transition(transition);
            }
            CommandStatus::Ambiguous | CommandStatus::Retryable => {
                self.replenishment.restore_ambiguous_command(
                    record.record_id,
                    record.draft,
                    "The server may have received the saved replenishment. Check it before continuing.",
                );
            }
            CommandStatus::ResponseRecorded => {
                let response = record.response.clone();
                self.replenishment.restore_ambiguous_command(
                    record.record_id,
                    record.draft.clone(),
                    "Applying the saved replenishment result.",
                );
                match response {
                    Some(response) => {
                        let scope = record.scope.clone();
                        self.apply_recorded_response(&scope, record, response);
                        if self.replenishment.activity() != Activity::ReconcileRequired {
                            self.session_gate = SessionGate::Recovering;
                            self.request_current_claim_for_operation(
                                context,
                                ClaimOperation::Replenishment,
                            );
                        }
                    }
                    None => self.replenishment.require_reconciliation(
                        "The saved replenishment response is incomplete.".into(),
                    ),
                }
            }
            CommandStatus::ReconcileRequired | CommandStatus::Dispatching => {
                self.replenishment.require_reconciliation(
                    "Saved replenishment needs supervisor review before inventory changes.".into(),
                );
            }
            CommandStatus::Completed | CommandStatus::Rejected => {}
        }
    }
}
