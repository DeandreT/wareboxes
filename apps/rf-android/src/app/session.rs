use eframe::egui;
use wareboxes_api_contract::v1::{
    CreateRfSessionRequest, CreateRfSessionResponse, ErrorReason, ErrorResponse,
};

use crate::command_store::{
    CommandOperation, CommandStatus, DurableCommandRecord, DurableHttpResponse, ExecutionScope,
    is_retryable_http_status,
};
use crate::expected_receiving::{
    ConfirmationFailure, ConfirmationId, LoadBarcode, LoadResolutionFailure, LoadResolutionId,
    ReceivingEffect, ReceivingTransition, ReconciliationReason, RefreshFailure, RefreshId,
};
use crate::transport::{
    AuthenticatedTransport, NetworkEvent, NetworkResponse, ServerEndpoint, build_command_request,
    build_current_claim_request, build_expected_receiving_barcode_lookup_request,
    build_expected_receiving_session_request, build_session_request, send_command,
    send_current_claim, send_expected_receiving_barcode_lookup, send_expected_receiving_session,
    send_session,
};
use crate::wire::{decode_claim_response, decode_command_response, decode_receiving_session};
use crate::workflow::{DurableCommandDraft, RfCommand, WorkflowEffect};

use super::{
    RF_SESSION_PATH, RfApp, RfSession, SessionGate, rejected_command_message,
    session_error_message, support_message, valid_email,
};

mod receiving;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReceivingRequestKind {
    Resolve {
        resolution_id: LoadResolutionId,
        barcode: LoadBarcode,
    },
    Refresh {
        refresh_id: RefreshId,
        load_id: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReceivingRequest {
    request_id: String,
    kind: ReceivingRequestKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReceivingCommandPhase {
    Ready,
    InFlight,
    Ambiguous,
    ReconcileRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReceivingCommandRuntime {
    record_id: i64,
    confirmation_id: ConfirmationId,
    phase: ReceivingCommandPhase,
    message: Option<String>,
}

impl ReceivingCommandRuntime {
    pub(super) const fn phase(&self) -> ReceivingCommandPhase {
        self.phase
    }

    pub(super) fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    pub(super) fn debug_ambiguous(
        confirmation_id: ConfirmationId,
        message: impl Into<String>,
    ) -> Self {
        Self {
            record_id: 1,
            confirmation_id,
            phase: ReceivingCommandPhase::Ambiguous,
            message: Some(message.into()),
        }
    }
}

impl RfApp {
    pub(super) fn begin_sign_in(&mut self, context: &egui::Context) {
        self.auth_error = None;
        let email = self.email.trim();
        if !valid_email(email) {
            self.auth_error = Some("Enter a valid email address.".into());
            return;
        }
        if self.password.is_empty() {
            self.auth_error = Some("Enter your password.".into());
            return;
        }
        let email = email.to_owned();
        let endpoint = match ServerEndpoint::parse(&self.server_url) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                self.auth_error = Some(format!("{error}."));
                self.edit_server = true;
                self.field_focus_pending = true;
                return;
            }
        };
        if !self.persist_server_endpoint(&endpoint) {
            return;
        }
        let body = match serde_json::to_vec(&CreateRfSessionRequest {
            email,
            password: self.password.clone(),
        }) {
            Ok(body) => body,
            Err(_) => {
                self.auth_error = Some("Sign in could not be started.".into());
                return;
            }
        };
        let request_id = format!("rf-{}", uuid::Uuid::new_v4());
        let request = match build_session_request(&endpoint, RF_SESSION_PATH, &request_id, body) {
            Ok(request) => request,
            Err(error) => {
                self.auth_error = Some(format!("{error}."));
                return;
            }
        };
        self.session_gate = SessionGate::SigningIn;
        self.expected_auth_request_id = Some(request_id.clone());
        send_session(
            request,
            request_id,
            self.network_tx.clone(),
            context.clone(),
        );
    }

    pub(super) fn process_network_events(&mut self, context: &egui::Context) {
        while let Ok(event) = self.network_rx.try_recv() {
            match event {
                NetworkEvent::Session {
                    request_id,
                    response,
                } => self.handle_session_response(context, &request_id, response),
                NetworkEvent::CurrentClaim {
                    request_id,
                    response,
                } => self.handle_current_claim_response(&request_id, response),
                NetworkEvent::Heartbeat {
                    task_id,
                    request_id,
                    response,
                } => self.handle_heartbeat_response(context, task_id, &request_id, response),
                NetworkEvent::ExpectedReceivingSession {
                    load_id,
                    request_id,
                    response,
                } => {
                    self.handle_receiving_session_response(context, load_id, &request_id, response)
                }
                NetworkEvent::ExpectedReceivingBarcodeLookup {
                    barcode,
                    request_id,
                    response,
                } => {
                    self.handle_receiving_barcode_response(context, &barcode, &request_id, response)
                }
                NetworkEvent::Command {
                    record_id,
                    attempt_id,
                    response,
                } => self.handle_command_response(record_id, &attempt_id, response),
            }
        }
    }

    fn handle_session_response(
        &mut self,
        context: &egui::Context,
        request_id: &str,
        response: Result<NetworkResponse, String>,
    ) {
        if self.expected_auth_request_id.as_deref() != Some(request_id) {
            return;
        }
        self.expected_auth_request_id = None;
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                self.session_gate = SessionGate::SignedOut;
                self.auth_error = Some("Can't reach the server. Check Wi-Fi and try again.".into());
                return;
            }
        };
        if !(200..300).contains(&response.status) {
            self.session_gate = SessionGate::SignedOut;
            self.auth_error = Some(session_error_message(response.status, &response.body));
            if response.status == 401 {
                self.password.clear();
                self.field_focus_pending = true;
            }
            return;
        }
        let contract = match serde_json::from_slice::<CreateRfSessionResponse>(&response.body) {
            Ok(contract)
                if !contract.token.is_empty()
                    && contract.operator_id > 0
                    && contract.tenant.tenant_id > 0 =>
            {
                contract
            }
            _ => {
                self.session_gate = SessionGate::SignedOut;
                self.auth_error = Some("The server returned an invalid sign-in response.".into());
                return;
            }
        };
        let endpoint = match ServerEndpoint::parse(&self.server_url) {
            Ok(endpoint) => endpoint,
            Err(_) => {
                self.session_gate = SessionGate::SignedOut;
                self.auth_error = Some("The server address is no longer valid.".into());
                return;
            }
        };
        let scope = ExecutionScope {
            tenant_id: contract.tenant.tenant_id,
            operator_id: contract.operator_id,
            device_id: self.device_id.clone(),
        };
        if self
            .reauth_scope
            .as_ref()
            .is_some_and(|expected| expected != &scope)
        {
            self.session_gate = SessionGate::SignedOut;
            self.auth_error =
                Some("Wrong operator. Sign in with the account that saved this work.".into());
            self.password.clear();
            self.field_focus_pending = true;
            return;
        }

        self.email = self.email.trim().to_owned();
        self.password.clear();
        self.reveal_password = false;
        self.auth_error = None;
        self.connectivity_notice = None;
        self.execution_scope = Some(scope.clone());
        self.session = Some(RfSession {
            endpoint,
            token: contract.token,
            tenant_name: contract.tenant.name,
            scope,
        });
        self.session_gate = SessionGate::Recovering;
        self.recover_after_sign_in(context);
    }

    fn recover_after_sign_in(&mut self, context: &egui::Context) {
        let Some(scope) = self.execution_scope.clone() else {
            return;
        };
        let unresolved = match self
            .command_store
            .as_ref()
            .map(|store| store.unresolved_for_device(&scope.device_id))
        {
            Some(Ok(records)) => records,
            Some(Err(_)) | None => {
                self.workflow
                    .require_reconciliation("Device storage could not load saved work.".into());
                self.session_gate = SessionGate::Ready;
                return;
            }
        };
        match unresolved.as_slice() {
            [] => self.request_current_claim(context),
            [record] if record.scope == scope => self.restore_command(context, record.clone()),
            [record] => {
                self.reauth_scope = Some(record.scope.clone());
                self.reauth_notice = Some(
                    "Your saved scan is still on this device. Use the same operator account."
                        .into(),
                );
                self.session = None;
                self.execution_scope = None;
                self.session_gate = SessionGate::SignedOut;
                self.password.clear();
                self.reveal_password = false;
                self.field_focus_pending = true;
                self.auth_error =
                    Some("Wrong operator. Sign in with the account that saved this work.".into());
            }
            _ => {
                self.workflow.require_reconciliation(
                    "More than one saved command requires supervisor review.".into(),
                );
                self.session_gate = SessionGate::Ready;
            }
        }
    }

    fn restore_command(&mut self, context: &egui::Context, record: DurableCommandRecord) {
        self.session_gate = SessionGate::Ready;
        if record.operation == CommandOperation::ExpectedReceiptConfirmation {
            self.restore_receiving_command(record);
            return;
        }
        match record.status {
            CommandStatus::Persisted => {
                let transition = self
                    .workflow
                    .restore_ready_command(record.record_id, record.draft);
                self.emit_transition(transition);
            }
            CommandStatus::Ambiguous | CommandStatus::Retryable => {
                self.workflow.restore_ambiguous_command(
                    record.record_id,
                    record.draft,
                    "The server may have received the saved scan. Check it before continuing.",
                );
            }
            CommandStatus::ResponseRecorded => {
                let response = record.response.clone();
                self.workflow.restore_ambiguous_command(
                    record.record_id,
                    record.draft.clone(),
                    "Applying the saved server result.",
                );
                match response {
                    Some(response) => {
                        let scope = record.scope.clone();
                        self.apply_recorded_response(&scope, record, response);
                        if self.workflow.activity() != crate::workflow::Activity::ReconcileRequired
                        {
                            self.session_gate = SessionGate::Recovering;
                            self.request_current_claim(context);
                        }
                    }
                    None => self
                        .workflow
                        .require_reconciliation("The saved server response is incomplete.".into()),
                }
            }
            CommandStatus::ReconcileRequired | CommandStatus::Dispatching => {
                self.workflow.require_reconciliation(
                    "Saved work needs supervisor review before inventory moves.".into(),
                );
            }
            CommandStatus::Completed | CommandStatus::Rejected => {}
        }
    }

    fn restore_receiving_command(&mut self, record: DurableCommandRecord) {
        let RfCommand::ExpectedReceipt(intent) = &record.draft.command else {
            self.require_receiving_reconciliation(
                "The saved receipt does not contain a receiving command.",
            );
            return;
        };
        let confirmation_id = if let Some(runtime) = self
            .receiving_command
            .as_ref()
            .filter(|runtime| runtime.record_id == record.record_id)
        {
            runtime.confirmation_id
        } else {
            match self
                .receiving
                .restore_pending_confirmation((**intent).clone())
            {
                Ok(confirmation_id) => confirmation_id,
                Err(_) => {
                    self.require_receiving_reconciliation(
                        "The saved receipt recovery snapshot is invalid.",
                    );
                    return;
                }
            }
        };
        self.work_mode = super::WorkMode::Receive;
        let phase = match record.status {
            CommandStatus::Persisted => ReceivingCommandPhase::Ready,
            CommandStatus::Ambiguous | CommandStatus::Retryable => ReceivingCommandPhase::Ambiguous,
            CommandStatus::ResponseRecorded => ReceivingCommandPhase::InFlight,
            CommandStatus::ReconcileRequired | CommandStatus::Dispatching => {
                ReceivingCommandPhase::ReconcileRequired
            }
            CommandStatus::Completed | CommandStatus::Rejected => return,
        };
        self.receiving_command = Some(ReceivingCommandRuntime {
            record_id: record.record_id,
            confirmation_id,
            phase,
            message: match phase {
                ReceivingCommandPhase::Ambiguous => Some(
                    "The server may have received this receipt. Check it before continuing.".into(),
                ),
                ReceivingCommandPhase::ReconcileRequired => {
                    Some("Saved receiving work needs supervisor review.".into())
                }
                ReceivingCommandPhase::Ready | ReceivingCommandPhase::InFlight => None,
            },
        });
        match record.status {
            CommandStatus::ResponseRecorded => match record.response.clone() {
                Some(response) => {
                    let scope = record.scope.clone();
                    self.apply_recorded_response(&scope, record, response);
                }
                None => self
                    .require_receiving_reconciliation("The saved receipt response is incomplete."),
            },
            CommandStatus::ReconcileRequired | CommandStatus::Dispatching => {
                self.require_receiving_reconciliation(
                    "Saved receiving work needs supervisor review before inventory moves.",
                );
            }
            CommandStatus::Persisted
            | CommandStatus::Ambiguous
            | CommandStatus::Retryable
            | CommandStatus::Completed
            | CommandStatus::Rejected => {}
        }
    }

    pub(super) fn request_current_claim(&mut self, context: &egui::Context) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let request_id = format!("rf-{}", uuid::Uuid::new_v4());
        let transport = AuthenticatedTransport {
            endpoint: &session.endpoint,
            token: &session.token,
            scope: &session.scope,
        };
        let request = match build_current_claim_request(&transport, &request_id) {
            Ok(request) => request,
            Err(_) => {
                self.session_gate = SessionGate::Ready;
                if self.lease_check_task_id.is_none() {
                    self.connectivity_notice =
                        Some("Saved work could not be checked. Try again.".into());
                }
                return;
            }
        };
        self.expected_claim_request_id = Some(request_id.clone());
        send_current_claim(
            request,
            request_id,
            self.network_tx.clone(),
            context.clone(),
        );
    }

    fn handle_current_claim_response(
        &mut self,
        request_id: &str,
        response: Result<NetworkResponse, String>,
    ) {
        if self.expected_claim_request_id.as_deref() != Some(request_id) {
            return;
        }
        self.expected_claim_request_id = None;
        let lease_check_task_id = self.lease_check_task_id.take();
        let lease_rejection_check = std::mem::take(&mut self.lease_rejection_check);
        self.session_gate = SessionGate::Ready;
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                if let Some(task_id) = lease_check_task_id {
                    self.lease_check_task_id = Some(task_id);
                    self.lease_rejection_check = lease_rejection_check;
                } else {
                    self.connectivity_notice =
                        Some("Connection lost. Saved work could not be checked.".into());
                }
                return;
            }
        };
        if response.status == 401 {
            self.clear_claim_heartbeat();
            if let Some(task_id) = lease_check_task_id {
                self.require_reauthentication_for_task(task_id);
            } else {
                self.require_reauthentication(None);
            }
            return;
        }
        if !(200..300).contains(&response.status) {
            if let Some(task_id) = lease_check_task_id {
                self.lease_check_task_id = Some(task_id);
                self.lease_rejection_check = lease_rejection_check;
            } else {
                self.connectivity_notice = Some(
                    "The current task could not be checked. Try again before claiming work.".into(),
                );
            }
            return;
        }
        match decode_claim_response(&response.body) {
            Ok(claim) => {
                self.clear_claim_heartbeat();
                if let Some(expected_task_id) = lease_check_task_id {
                    match claim {
                        Some(claim)
                            if claim.task_id == expected_task_id && !lease_rejection_check =>
                        {
                            self.workflow.restore_current_claim(Some(claim));
                        }
                        Some(_) | None => {
                            self.workflow.restore_current_claim(None);
                            self.workflow.require_reconciliation(
                                "This task is no longer assigned. Do not move its inventory. Contact a supervisor."
                                    .into(),
                            );
                        }
                    }
                } else {
                    self.workflow.restore_current_claim(claim);
                }
                self.connectivity_notice = None;
                self.reauth_scope = None;
                self.reauth_notice = None;
            }
            Err(_) => {
                self.clear_claim_heartbeat();
                self.workflow.require_reconciliation(
                    "The server returned invalid current-work data.".into(),
                );
            }
        }
    }

    pub(super) fn request_current_claim_for_lease(
        &mut self,
        context: &egui::Context,
        task_id: i64,
    ) {
        if task_id <= 0 || self.expected_claim_request_id.is_some() {
            return;
        }
        self.clear_claim_heartbeat();
        self.lease_check_task_id = Some(task_id);
        self.lease_rejection_check = false;
        self.request_current_claim(context);
    }

    pub(super) fn request_current_claim_after_rejection(
        &mut self,
        context: &egui::Context,
        task_id: i64,
    ) {
        self.request_current_claim_for_lease(context, task_id);
        if self.lease_check_task_id == Some(task_id) {
            self.lease_rejection_check = true;
        }
    }

    pub(super) fn persist_queued_commands(&mut self) {
        let queued = self.effects.len();
        for _ in 0..queued {
            let Some(effect) = self.effects.pop_front() else {
                break;
            };
            let WorkflowEffect::PersistCommand(draft) = effect else {
                self.effects.push_back(effect);
                continue;
            };
            let Some(store) = self.command_store.as_mut() else {
                self.workflow
                    .require_reconciliation("Durable device storage is unavailable".into());
                continue;
            };
            let Some(scope) = self.execution_scope.as_ref() else {
                self.workflow.require_reconciliation(
                    "The command cannot be stored without an authenticated device scope".into(),
                );
                continue;
            };
            let command_id = draft.command_id.clone();
            match store.persist(scope, draft) {
                Ok(record) => {
                    let transition = self
                        .workflow
                        .command_persisted(&command_id, record.record_id);
                    self.emit_transition(transition);
                }
                Err(error) => {
                    self.workflow.require_reconciliation(format!(
                        "The command could not be stored durably: {error}"
                    ));
                }
            }
        }
    }

    pub(super) fn dispatch_queued_commands(&mut self, context: &egui::Context) {
        let queued = self.effects.len();
        for _ in 0..queued {
            let Some(effect) = self.effects.pop_front() else {
                break;
            };
            let WorkflowEffect::DispatchPersistedCommand { record_id } = effect else {
                self.effects.push_back(effect);
                continue;
            };
            let (Some(session), Some(scope), Some(store)) = (
                self.session.as_ref(),
                self.execution_scope.as_ref(),
                self.command_store.as_mut(),
            ) else {
                self.effects
                    .push_front(WorkflowEffect::DispatchPersistedCommand { record_id });
                break;
            };
            let attempt = match store.begin_attempt(scope, record_id) {
                Ok(attempt) => attempt,
                Err(_) => {
                    self.workflow.require_reconciliation(
                        "The saved command could not enter network dispatch.".into(),
                    );
                    continue;
                }
            };
            let transport = AuthenticatedTransport {
                endpoint: &session.endpoint,
                token: &session.token,
                scope,
            };
            let request = match build_command_request(&transport, &attempt) {
                Ok(request) => request,
                Err(error) => {
                    let _ = store.mark_ambiguous(
                        scope,
                        record_id,
                        &attempt.attempt_id,
                        &error.to_string(),
                    );
                    self.workflow.require_reconciliation(
                        "The saved command failed its integrity check.".into(),
                    );
                    continue;
                }
            };
            self.workflow.dispatch_started(record_id);
            send_command(
                request,
                record_id,
                attempt.attempt_id,
                self.network_tx.clone(),
                context.clone(),
            );
        }
    }

    fn handle_command_response(
        &mut self,
        record_id: i64,
        attempt_id: &str,
        response: Result<DurableHttpResponse, String>,
    ) {
        let Some(scope) = self.execution_scope.clone() else {
            if self
                .receiving_command
                .as_ref()
                .is_some_and(|runtime| runtime.record_id == record_id)
            {
                self.require_receiving_reconciliation(
                    "The authenticated device scope was lost during dispatch.",
                );
            } else {
                self.workflow.require_reconciliation(
                    "The authenticated device scope was lost during dispatch.".into(),
                );
            }
            return;
        };
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                let stored = self.command_store.as_mut().and_then(|store| {
                    store
                        .mark_ambiguous(
                            &scope,
                            record_id,
                            attempt_id,
                            "connection ended before the result was confirmed",
                        )
                        .ok()
                });
                if let Some(stored) = stored {
                    if stored.operation == CommandOperation::ExpectedReceiptConfirmation {
                        if let Some(runtime) = self.receiving_command.as_mut() {
                            runtime.phase = ReceivingCommandPhase::Ambiguous;
                            runtime.message = Some(
                                "The server may have received the receipt. Check it before continuing."
                                    .into(),
                            );
                        }
                    } else {
                        self.workflow.dispatch_ambiguous(
                            record_id,
                            "The server may have received the scan. Check it before continuing.",
                        );
                    }
                    self.connectivity_notice = Some("Connection lost.".into());
                } else {
                    if self
                        .receiving_command
                        .as_ref()
                        .is_some_and(|runtime| runtime.record_id == record_id)
                    {
                        self.require_receiving_reconciliation(
                            "The interrupted receipt could not be saved for recovery.",
                        );
                    } else {
                        self.workflow.require_reconciliation(
                            "The interrupted command could not be saved for recovery.".into(),
                        );
                    }
                }
                return;
            }
        };

        if is_retryable_http_status(response.status) {
            let stored = self.command_store.as_mut().and_then(|store| {
                store
                    .record_retryable_response(&scope, record_id, attempt_id, &response)
                    .ok()
            });
            let Some(stored) = stored else {
                if self
                    .receiving_command
                    .as_ref()
                    .is_some_and(|runtime| runtime.record_id == record_id)
                {
                    self.require_receiving_reconciliation(
                        "The retryable receipt result could not be saved.",
                    );
                } else {
                    self.workflow.require_reconciliation(
                        "The retryable server result could not be saved.".into(),
                    );
                }
                return;
            };
            let is_receiving = stored.operation == CommandOperation::ExpectedReceiptConfirmation;
            if response.status == 401 {
                if is_receiving {
                    if let Some(runtime) = self.receiving_command.as_mut() {
                        runtime.phase = ReceivingCommandPhase::Ambiguous;
                        runtime.message = Some(
                            "Sign in with the same operator account to recover this receipt."
                                .into(),
                        );
                    }
                } else {
                    self.workflow.restore_ambiguous_command(
                        record_id,
                        stored.draft,
                        "Sign in with the same operator account to recover this saved scan.",
                    );
                }
                self.require_reauthentication_with_message(
                    Some(scope),
                    "Session expired. Sign in to recover the saved scan.",
                    Some(
                        "Your saved scan is still on this device. Use the same operator account."
                            .into(),
                    ),
                );
            } else {
                if is_receiving {
                    if let Some(runtime) = self.receiving_command.as_mut() {
                        runtime.phase = ReceivingCommandPhase::Ambiguous;
                        runtime.message = Some(
                            "The service is temporarily unavailable. Check the saved receipt again."
                                .into(),
                        );
                    }
                } else {
                    self.workflow.dispatch_ambiguous(
                        record_id,
                        "The service is temporarily unavailable. Check the saved scan again.",
                    );
                }
                self.connectivity_notice = Some("Server unavailable.".into());
            }
            return;
        }

        let recorded = match self
            .command_store
            .as_mut()
            .map(|store| store.record_response(&scope, record_id, attempt_id, &response))
        {
            Some(Ok(record)) => record,
            Some(Err(_)) | None => {
                if self
                    .receiving_command
                    .as_ref()
                    .is_some_and(|runtime| runtime.record_id == record_id)
                {
                    self.require_receiving_reconciliation(
                        "The receipt result could not be stored durably.",
                    );
                } else {
                    self.workflow.require_reconciliation(
                        "The server result could not be stored durably.".into(),
                    );
                }
                return;
            }
        };
        self.apply_recorded_response(&scope, recorded, response);
    }

    fn apply_recorded_response(
        &mut self,
        scope: &ExecutionScope,
        recorded: DurableCommandRecord,
        response: DurableHttpResponse,
    ) {
        let record_id = recorded.record_id;
        if (200..300).contains(&response.status) {
            match decode_command_response(
                recorded.request.response_kind,
                response.status,
                &response.body,
            ) {
                Ok(crate::workflow::CommandOutcome::ExpectedReceipt(result)) => {
                    self.apply_receiving_success(scope, record_id, result);
                }
                Ok(outcome) => {
                    if self
                        .finalize_record(scope, record_id, CommandStatus::Completed, None)
                        .is_some()
                    {
                        self.workflow.durable_outcome_recorded(record_id, outcome);
                        self.connectivity_notice = None;
                    }
                }
                Err(_) => {
                    if recorded.operation == CommandOperation::ExpectedReceiptConfirmation {
                        self.require_receiving_record_reconciliation(
                            scope,
                            record_id,
                            "The server returned an invalid receipt result.",
                        );
                    } else {
                        self.require_record_reconciliation(
                            scope,
                            record_id,
                            "The server returned an invalid command result.",
                        );
                    }
                }
            }
            return;
        }

        let error = serde_json::from_slice::<ErrorResponse>(&response.body).ok();
        let request_id = error
            .as_ref()
            .map(|error| error.request_id.as_str())
            .or(response.server_request_id.as_deref());
        let reason = error.as_ref().map(|error| error.reason);
        if matches!(response.status, 400 | 422)
            && !matches!(reason, Some(ErrorReason::IdempotencyKeyReused))
        {
            let message = rejected_command_message(error.as_ref());
            if recorded.operation == CommandOperation::ExpectedReceiptConfirmation {
                self.apply_receiving_rejection(scope, record_id, &message);
                return;
            }
            if self
                .finalize_record(scope, record_id, CommandStatus::Rejected, Some(&message))
                .is_some()
            {
                self.workflow.durable_rejection_recorded(record_id, message);
            }
        } else {
            let message = support_message(
                "Work needs review. Do not move or scan inventory.",
                request_id,
            );
            if recorded.operation == CommandOperation::ExpectedReceiptConfirmation {
                self.require_receiving_record_reconciliation(scope, record_id, &message);
            } else {
                self.require_record_reconciliation(scope, record_id, &message);
            }
        }
    }

    fn apply_receiving_rejection(&mut self, scope: &ExecutionScope, record_id: i64, message: &str) {
        let Some(confirmation_id) = self.receiving_command.as_ref().and_then(|runtime| {
            (runtime.record_id == record_id).then_some(runtime.confirmation_id)
        }) else {
            self.require_receiving_record_reconciliation(
                scope,
                record_id,
                "The rejected receipt does not match the saved workflow.",
            );
            return;
        };
        let mut next = self.receiving.clone();
        let transition = next.confirmation_failed(confirmation_id, ConfirmationFailure::Rejected);
        if !matches!(transition, ReceivingTransition::Applied) {
            self.require_receiving_record_reconciliation(
                scope,
                record_id,
                "The rejected receipt does not match the saved workflow.",
            );
            return;
        }
        if self
            .finalize_receiving_record(scope, record_id, CommandStatus::Rejected, Some(message))
            .is_some()
        {
            self.receiving = next;
            self.receiving_command = None;
            self.emit_receiving_transition(transition);
        }
    }

    fn apply_receiving_success(
        &mut self,
        scope: &ExecutionScope,
        record_id: i64,
        result: crate::expected_receiving::ConfirmationResult,
    ) {
        let Some(confirmation_id) = self.receiving_command.as_ref().and_then(|runtime| {
            (runtime.record_id == record_id).then_some(runtime.confirmation_id)
        }) else {
            self.require_receiving_record_reconciliation(
                scope,
                record_id,
                "The receipt result does not match the saved workflow.",
            );
            return;
        };
        let mut next = self.receiving.clone();
        let transition = next.confirmation_succeeded(confirmation_id, result);
        if matches!(transition, ReceivingTransition::ReconciliationRequired(_)) {
            let message = "The receipt result conflicts with the saved pre-command state.";
            if self
                .finalize_receiving_record(
                    scope,
                    record_id,
                    CommandStatus::ReconcileRequired,
                    Some(message),
                )
                .is_some()
            {
                self.receiving = next;
                if let Some(runtime) = self.receiving_command.as_mut() {
                    runtime.phase = ReceivingCommandPhase::ReconcileRequired;
                    runtime.message = Some(message.into());
                }
            }
            return;
        }
        if !matches!(
            transition,
            ReceivingTransition::Applied | ReceivingTransition::Effect(_)
        ) {
            self.require_receiving_record_reconciliation(
                scope,
                record_id,
                "The receipt result does not match the saved workflow.",
            );
            return;
        }
        if self
            .finalize_receiving_record(scope, record_id, CommandStatus::Completed, None)
            .is_some()
        {
            self.receiving = next;
            self.receiving_command = None;
            self.emit_receiving_transition(transition);
            self.connectivity_notice = None;
        }
    }

    fn finalize_receiving_record(
        &mut self,
        scope: &ExecutionScope,
        record_id: i64,
        status: CommandStatus,
        message: Option<&str>,
    ) -> Option<DurableCommandRecord> {
        match self
            .command_store
            .as_mut()
            .map(|store| store.finalize(scope, record_id, status, message))
        {
            Some(Ok(record)) => Some(record),
            Some(Err(_)) | None => {
                self.require_receiving_reconciliation(
                    "The durable receipt could not be finalized.",
                );
                None
            }
        }
    }

    fn finalize_record(
        &mut self,
        scope: &ExecutionScope,
        record_id: i64,
        status: CommandStatus,
        message: Option<&str>,
    ) -> Option<DurableCommandRecord> {
        match self
            .command_store
            .as_mut()
            .map(|store| store.finalize(scope, record_id, status, message))
        {
            Some(Ok(record)) => Some(record),
            Some(Err(_)) | None => {
                if self
                    .receiving_command
                    .as_ref()
                    .is_some_and(|runtime| runtime.record_id == record_id)
                {
                    self.require_receiving_reconciliation(
                        "The durable receipt could not be finalized.",
                    );
                } else {
                    self.workflow.require_reconciliation(
                        "The durable command could not be finalized.".into(),
                    );
                }
                None
            }
        }
    }

    fn require_record_reconciliation(
        &mut self,
        scope: &ExecutionScope,
        record_id: i64,
        message: &str,
    ) {
        if self
            .finalize_record(
                scope,
                record_id,
                CommandStatus::ReconcileRequired,
                Some(message),
            )
            .is_some()
        {
            if self
                .receiving_command
                .as_ref()
                .is_some_and(|runtime| runtime.record_id == record_id)
            {
                self.require_receiving_reconciliation(message);
            } else {
                self.workflow.require_reconciliation(message.into());
            }
        }
    }

    fn require_receiving_record_reconciliation(
        &mut self,
        scope: &ExecutionScope,
        record_id: i64,
        message: &str,
    ) {
        if self
            .finalize_receiving_record(
                scope,
                record_id,
                CommandStatus::ReconcileRequired,
                Some(message),
            )
            .is_some()
        {
            self.require_receiving_reconciliation(message);
        }
    }

    fn require_reauthentication(&mut self, scope: Option<ExecutionScope>) {
        self.require_reauthentication_with_message(scope, "Session expired. Sign in again.", None);
    }

    pub(super) fn require_reauthentication_for_task(&mut self, task_id: i64) {
        self.clear_claim_heartbeat();
        self.require_reauthentication_with_message(
            self.execution_scope.clone(),
            &format!("Session expired. Sign in to continue task {task_id}."),
            Some(format!(
                "Task {task_id} must be checked with the same operator account."
            )),
        );
    }

    fn require_reauthentication_with_message(
        &mut self,
        scope: Option<ExecutionScope>,
        message: &str,
        notice: Option<String>,
    ) {
        let scope = scope.or_else(|| self.execution_scope.clone());
        self.reauth_scope = scope;
        self.reauth_notice = notice;
        self.session = None;
        self.session_gate = SessionGate::SignedOut;
        self.password.clear();
        self.reveal_password = false;
        self.field_focus_pending = true;
        self.auth_error = Some(message.into());
    }

    pub(super) fn can_execute(&self) -> bool {
        self.command_store.is_some()
            && self.execution_scope.is_some()
            && self.session.is_some()
            && self.session_gate == SessionGate::Ready
            && self.connectivity_notice.is_none()
    }
}
