use eframe::egui;

use crate::command_store::{CommandStatus, DurableCommandRecord};
use crate::workflow::{Activity, ClaimOperation};

use super::{RfApp, SessionGate, WorkMode};

impl RfApp {
    pub(super) fn restore_cross_dock_command(
        &mut self,
        context: &egui::Context,
        record: DurableCommandRecord,
    ) {
        self.work_mode = WorkMode::CrossDock;
        match record.status {
            CommandStatus::Persisted => {
                let transition = self
                    .cross_dock
                    .restore_ready_command(record.record_id, record.draft);
                self.emit_cross_dock_transition(transition);
            }
            CommandStatus::Ambiguous | CommandStatus::Retryable => {
                self.cross_dock.restore_ambiguous_command(
                    record.record_id,
                    record.draft,
                    "The server may have received the saved cross-dock action. Check it before continuing.",
                );
            }
            CommandStatus::ResponseRecorded => {
                let response = record.response.clone();
                self.cross_dock.restore_ambiguous_command(
                    record.record_id,
                    record.draft.clone(),
                    "Applying the saved cross-dock result.",
                );
                match response {
                    Some(response) => {
                        let scope = record.scope.clone();
                        self.apply_recorded_response(&scope, record, response);
                        if self.cross_dock.activity() != Activity::ReconcileRequired {
                            self.session_gate = SessionGate::Recovering;
                            self.request_current_claim_for_operation(
                                context,
                                ClaimOperation::CrossDock,
                            );
                        }
                    }
                    None => self.cross_dock.require_reconciliation(
                        "The saved cross-dock response is incomplete.".into(),
                    ),
                }
            }
            CommandStatus::ReconcileRequired | CommandStatus::Dispatching => {
                self.cross_dock.require_reconciliation(
                    "Saved cross-dock work needs supervisor review before inventory changes."
                        .into(),
                );
            }
            CommandStatus::Completed | CommandStatus::Rejected => {}
        }
    }
}
