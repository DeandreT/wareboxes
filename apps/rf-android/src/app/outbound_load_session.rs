use eframe::egui;
use wareboxes_api_contract::v1::{ErrorResponse, OutboundLoadResponse};

use crate::command_store::{CommandStatus, DurableCommandRecord};
use crate::transport::{
    AuthenticatedTransport, NetworkResponse, build_outbound_load_lookup_request,
    send_outbound_load_lookup,
};
use crate::workflow::Activity;

use super::{RfApp, SessionGate, WorkMode, rejected_command_message};

impl RfApp {
    pub(super) fn begin_outbound_load_lookup(&mut self, context: &egui::Context) {
        let barcode = self.outbound_load_barcode_draft.trim().to_owned();
        if barcode.is_empty() || self.expected_outbound_load_request_id.is_some() {
            return;
        }
        let (Some(session), Some(scope)) = (self.session.as_ref(), self.execution_scope.as_ref())
        else {
            self.outbound_load
                .load_lookup_failed("Sign in before scanning a load");
            return;
        };
        let request_id = format!("rf-{}", uuid::Uuid::new_v4());
        let transport = AuthenticatedTransport {
            endpoint: &session.endpoint,
            token: &session.token,
            scope,
        };
        let request = match build_outbound_load_lookup_request(&transport, &barcode, &request_id) {
            Ok(request) => request,
            Err(error) => {
                self.outbound_load.load_lookup_failed(error.to_string());
                return;
            }
        };
        self.expected_outbound_load_request_id = Some(request_id.clone());
        send_outbound_load_lookup(
            request,
            barcode,
            request_id,
            self.network_tx.clone(),
            context.clone(),
        );
    }

    pub(super) fn handle_outbound_load_lookup(
        &mut self,
        barcode: &str,
        request_id: &str,
        response: Result<NetworkResponse, String>,
    ) {
        if self.expected_outbound_load_request_id.as_deref() != Some(request_id) {
            return;
        }
        self.expected_outbound_load_request_id = None;
        match response {
            Ok(response) if (200..300).contains(&response.status) => {
                match serde_json::from_slice::<OutboundLoadResponse>(&response.body) {
                    Ok(load) if load.load_barcode.eq_ignore_ascii_case(barcode) => {
                        self.outbound_load_barcode_draft.clear();
                        self.outbound_load.resolve_load(load);
                        self.outbound_load_scan_focus = None;
                    }
                    _ => self.outbound_load.require_reconciliation(
                        "Outbound-load lookup returned mismatched execution state".into(),
                    ),
                }
            }
            Ok(response) => {
                let error = serde_json::from_slice::<ErrorResponse>(&response.body).ok();
                self.outbound_load
                    .load_lookup_failed(rejected_command_message(error.as_ref()));
            }
            Err(_) => self
                .outbound_load
                .load_lookup_failed("Can't reach the warehouse service. Check Wi-Fi and retry."),
        }
    }

    pub(super) fn restore_outbound_load_command(&mut self, record: DurableCommandRecord) {
        self.work_mode = WorkMode::OutboundLoad;
        self.session_gate = SessionGate::Ready;
        match record.status {
            CommandStatus::Persisted => {
                let transition = self
                    .outbound_load
                    .restore_ready_command(record.record_id, record.draft);
                self.emit_outbound_load_transition(transition);
            }
            CommandStatus::Ambiguous | CommandStatus::Retryable => {
                self.outbound_load.restore_ambiguous_command(
                    record.record_id,
                    record.draft,
                    "The server may have received the saved carton move. Check it before continuing.",
                );
            }
            CommandStatus::ResponseRecorded => {
                let response = record.response.clone();
                self.outbound_load.restore_ambiguous_command(
                    record.record_id,
                    record.draft.clone(),
                    "Applying the saved carton-move result.",
                );
                match response {
                    Some(response) => {
                        let scope = record.scope.clone();
                        self.apply_recorded_response(&scope, record, response);
                    }
                    None => self.outbound_load.require_reconciliation(
                        "The saved carton-move response is incomplete".into(),
                    ),
                }
            }
            CommandStatus::ReconcileRequired | CommandStatus::Dispatching => {
                self.outbound_load.require_reconciliation(
                    "Saved carton work needs supervisor review before inventory moves".into(),
                );
            }
            CommandStatus::Completed | CommandStatus::Rejected => {}
        }
        if self.outbound_load.activity() == Activity::Active {
            self.outbound_load_scan_focus = None;
        }
    }
}
