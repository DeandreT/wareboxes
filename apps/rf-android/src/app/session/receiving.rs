use super::*;

impl RfApp {
    pub(in crate::app) fn emit_receiving_transition(&mut self, transition: ReceivingTransition) {
        if let ReceivingTransition::Effect(effect) = transition {
            self.receiving_effects.push_back(effect);
        }
    }

    pub(in crate::app) fn process_receiving_effects(&mut self, context: &egui::Context) {
        let queued = self.receiving_effects.len();
        for _ in 0..queued {
            let Some(effect) = self.receiving_effects.pop_front() else {
                break;
            };
            match effect {
                ReceivingEffect::ResolveLoad {
                    resolution_id,
                    barcode,
                } => self.resolve_receiving_load(context, resolution_id, barcode),
                ReceivingEffect::PersistConfirmation {
                    confirmation_id,
                    intent,
                } => self.persist_receiving_confirmation(confirmation_id, *intent),
                ReceivingEffect::RefreshSession {
                    refresh_id,
                    load_id,
                } => self.refresh_receiving_session(context, refresh_id, load_id.get()),
            }
        }
        self.dispatch_receiving_command(context);
    }

    fn resolve_receiving_load(
        &mut self,
        context: &egui::Context,
        resolution_id: LoadResolutionId,
        barcode: LoadBarcode,
    ) {
        if self.receiving_request.is_some() {
            let transition = self
                .receiving
                .load_resolution_failed(resolution_id, LoadResolutionFailure::Retryable);
            self.emit_receiving_transition(transition);
            return;
        }
        let Some(session) = self.session.as_ref() else {
            let transition = self
                .receiving
                .load_resolution_failed(resolution_id, LoadResolutionFailure::Retryable);
            self.emit_receiving_transition(transition);
            return;
        };
        let request_id = format!("rf-{}", uuid::Uuid::new_v4());
        let transport = AuthenticatedTransport {
            endpoint: &session.endpoint,
            token: &session.token,
            scope: &session.scope,
        };
        let request = match build_expected_receiving_barcode_lookup_request(
            &transport,
            barcode.as_str(),
            &request_id,
        ) {
            Ok(request) => request,
            Err(_) => {
                let transition = self
                    .receiving
                    .load_resolution_failed(resolution_id, LoadResolutionFailure::InvalidResponse);
                self.emit_receiving_transition(transition);
                return;
            }
        };
        self.receiving_request = Some(ReceivingRequest {
            request_id: request_id.clone(),
            kind: ReceivingRequestKind::Resolve {
                resolution_id,
                barcode: barcode.clone(),
            },
        });
        send_expected_receiving_barcode_lookup(
            request,
            barcode.as_str().to_owned(),
            request_id,
            self.network_tx.clone(),
            context.clone(),
        );
    }

    fn refresh_receiving_session(
        &mut self,
        context: &egui::Context,
        refresh_id: RefreshId,
        load_id: i64,
    ) {
        if self.receiving_request.is_some() {
            let transition = self
                .receiving
                .refresh_failed(refresh_id, RefreshFailure::Retryable);
            self.emit_receiving_transition(transition);
            return;
        }
        let Some(session) = self.session.as_ref() else {
            let transition = self
                .receiving
                .refresh_failed(refresh_id, RefreshFailure::Retryable);
            self.emit_receiving_transition(transition);
            return;
        };
        let request_id = format!("rf-{}", uuid::Uuid::new_v4());
        let transport = AuthenticatedTransport {
            endpoint: &session.endpoint,
            token: &session.token,
            scope: &session.scope,
        };
        let request =
            match build_expected_receiving_session_request(&transport, load_id, &request_id) {
                Ok(request) => request,
                Err(_) => {
                    let transition = self
                        .receiving
                        .refresh_failed(refresh_id, RefreshFailure::InvalidResponse);
                    self.emit_receiving_transition(transition);
                    return;
                }
            };
        self.receiving_request = Some(ReceivingRequest {
            request_id: request_id.clone(),
            kind: ReceivingRequestKind::Refresh {
                refresh_id,
                load_id,
            },
        });
        send_expected_receiving_session(
            request,
            load_id,
            request_id,
            self.network_tx.clone(),
            context.clone(),
        );
    }

    pub(super) fn handle_receiving_barcode_response(
        &mut self,
        _context: &egui::Context,
        barcode: &str,
        request_id: &str,
        response: Result<NetworkResponse, String>,
    ) {
        let Some(ReceivingRequest {
            request_id: expected_request_id,
            kind:
                ReceivingRequestKind::Resolve {
                    resolution_id,
                    barcode: expected_barcode,
                },
        }) = self.receiving_request.as_ref()
        else {
            return;
        };
        if expected_request_id != request_id || expected_barcode.as_str() != barcode {
            return;
        }
        let resolution_id = *resolution_id;
        self.receiving_request = None;
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                let transition = self
                    .receiving
                    .load_resolution_failed(resolution_id, LoadResolutionFailure::Retryable);
                self.emit_receiving_transition(transition);
                return;
            }
        };
        if response.status == 401 {
            let transition = self
                .receiving
                .load_resolution_failed(resolution_id, LoadResolutionFailure::Retryable);
            self.emit_receiving_transition(transition);
            self.require_reauthentication(self.execution_scope.clone());
            return;
        }
        let transition = match response.status {
            200..=299 => match decode_receiving_session(None, response.status, &response.body) {
                Ok(session) => self.receiving.load_resolved(resolution_id, session),
                Err(_) => self
                    .receiving
                    .load_resolution_failed(resolution_id, LoadResolutionFailure::InvalidResponse),
            },
            404 => self
                .receiving
                .load_resolution_failed(resolution_id, LoadResolutionFailure::NotFound),
            409 => self
                .receiving
                .load_resolution_failed(resolution_id, LoadResolutionFailure::NotReady),
            408 | 429 | 500..=599 => self
                .receiving
                .load_resolution_failed(resolution_id, LoadResolutionFailure::Retryable),
            _ => self
                .receiving
                .load_resolution_failed(resolution_id, LoadResolutionFailure::InvalidResponse),
        };
        self.emit_receiving_transition(transition);
    }

    pub(super) fn handle_receiving_session_response(
        &mut self,
        _context: &egui::Context,
        load_id: i64,
        request_id: &str,
        response: Result<NetworkResponse, String>,
    ) {
        let Some(ReceivingRequest {
            request_id: expected_request_id,
            kind:
                ReceivingRequestKind::Refresh {
                    refresh_id,
                    load_id: expected_load_id,
                },
        }) = self.receiving_request.as_ref()
        else {
            return;
        };
        if expected_request_id != request_id || *expected_load_id != load_id {
            return;
        }
        let refresh_id = *refresh_id;
        self.receiving_request = None;
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                let transition = self
                    .receiving
                    .refresh_failed(refresh_id, RefreshFailure::Retryable);
                self.emit_receiving_transition(transition);
                return;
            }
        };
        if response.status == 401 {
            let transition = self
                .receiving
                .refresh_failed(refresh_id, RefreshFailure::Retryable);
            self.emit_receiving_transition(transition);
            self.require_reauthentication(self.execution_scope.clone());
            return;
        }
        let transition = match response.status {
            200..=299 => {
                match decode_receiving_session(Some(load_id), response.status, &response.body) {
                    Ok(session) => self.receiving.refresh_succeeded(refresh_id, session),
                    Err(_) => self
                        .receiving
                        .refresh_failed(refresh_id, RefreshFailure::InvalidResponse),
                }
            }
            404 | 409 => self
                .receiving
                .refresh_failed(refresh_id, RefreshFailure::NotFoundOrConflict),
            408 | 429 | 500..=599 => self
                .receiving
                .refresh_failed(refresh_id, RefreshFailure::Retryable),
            _ => self
                .receiving
                .refresh_failed(refresh_id, RefreshFailure::InvalidResponse),
        };
        self.emit_receiving_transition(transition);
    }

    fn persist_receiving_confirmation(
        &mut self,
        confirmation_id: ConfirmationId,
        intent: crate::expected_receiving::ConfirmationIntent,
    ) {
        let Some(scope) = self.execution_scope.as_ref() else {
            let transition = self
                .receiving
                .require_reconciliation(ReconciliationReason::CommandIntegrityFailure);
            self.emit_receiving_transition(transition);
            return;
        };
        let Some(store) = self.command_store.as_mut() else {
            let transition = self
                .receiving
                .require_reconciliation(ReconciliationReason::CommandIntegrityFailure);
            self.emit_receiving_transition(transition);
            return;
        };
        let (command_id, idempotency_key) = Self::command_identity("expected-receipt");
        let draft = DurableCommandDraft {
            schema_version: 1,
            command_id,
            idempotency_key,
            command: RfCommand::ExpectedReceipt(Box::new(intent)),
        };
        match store.persist(scope, draft) {
            Ok(record) => {
                self.receiving_command = Some(ReceivingCommandRuntime {
                    record_id: record.record_id,
                    confirmation_id,
                    phase: ReceivingCommandPhase::Ready,
                    message: None,
                });
            }
            Err(_) => {
                let transition = self
                    .receiving
                    .require_reconciliation(ReconciliationReason::CommandIntegrityFailure);
                self.emit_receiving_transition(transition);
            }
        }
    }

    fn dispatch_receiving_command(&mut self, context: &egui::Context) {
        let Some(runtime) = self.receiving_command.as_ref() else {
            return;
        };
        if runtime.phase != ReceivingCommandPhase::Ready {
            return;
        }
        let record_id = runtime.record_id;
        let (Some(session), Some(scope), Some(store)) = (
            self.session.as_ref(),
            self.execution_scope.as_ref(),
            self.command_store.as_mut(),
        ) else {
            return;
        };
        let attempt = match store.begin_attempt(scope, record_id) {
            Ok(attempt) => attempt,
            Err(_) => {
                self.require_receiving_reconciliation(
                    "The saved receipt could not enter network dispatch.",
                );
                return;
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
                let _ =
                    store.mark_ambiguous(scope, record_id, &attempt.attempt_id, &error.to_string());
                self.require_receiving_reconciliation(
                    "The saved receipt failed its integrity check.",
                );
                return;
            }
        };
        if let Some(runtime) = self.receiving_command.as_mut() {
            runtime.phase = ReceivingCommandPhase::InFlight;
            runtime.message = None;
        }
        send_command(
            request,
            record_id,
            attempt.attempt_id,
            self.network_tx.clone(),
            context.clone(),
        );
    }

    pub(in crate::app) fn retry_receiving_command(&mut self) {
        let Some(runtime) = self.receiving_command.as_mut() else {
            return;
        };
        if runtime.phase == ReceivingCommandPhase::Ambiguous {
            runtime.phase = ReceivingCommandPhase::Ready;
            runtime.message = None;
        }
    }

    pub(super) fn require_receiving_reconciliation(&mut self, message: &str) {
        self.work_mode = crate::app::WorkMode::Receive;
        if let Some(runtime) = self.receiving_command.as_mut() {
            runtime.phase = ReceivingCommandPhase::ReconcileRequired;
            runtime.message = Some(message.to_owned());
        }
        let transition = self
            .receiving
            .require_reconciliation(ReconciliationReason::CommandIntegrityFailure);
        self.emit_receiving_transition(transition);
    }
}
