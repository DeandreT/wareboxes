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
use crate::wire::{
    decode_claim_response, decode_command_response, decode_cycle_count_claim_response,
    decode_pick_claim_response, decode_receiving_session, decode_relocation_claim_response,
    decode_replenishment_claim_response,
};
use crate::workflow::{
    ClaimOperation, DurableCommandDraft, InventoryRelocationClaim, MovementWorkflow, PutawayClaim,
    RfCommand, WorkflowEffect,
};

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

enum CurrentMovementClaim {
    Putaway(Option<PutawayClaim>),
    InventoryRelocation(Option<InventoryRelocationClaim>),
    CycleCount(Option<crate::cycle_count::CycleCountClaim>),
    Picking(Option<crate::picking::PickClaim>),
    Replenishment(Option<crate::replenishment::ReplenishmentClaim>),
}

impl CurrentMovementClaim {
    fn is_none(&self) -> bool {
        match self {
            Self::Putaway(claim) => claim.is_none(),
            Self::InventoryRelocation(claim) => claim.is_none(),
            Self::CycleCount(claim) => claim.is_none(),
            Self::Picking(claim) => claim.is_none(),
            Self::Replenishment(claim) => claim.is_none(),
        }
    }

    fn task_id(&self) -> Option<i64> {
        match self {
            Self::Putaway(claim) => claim.as_ref().map(|claim| claim.details().task_id),
            Self::InventoryRelocation(claim) => claim.as_ref().map(|claim| claim.details().task_id),
            Self::CycleCount(claim) => claim.as_ref().map(|claim| claim.task_id),
            Self::Picking(claim) => claim.as_ref().map(|claim| claim.task_id),
            Self::Replenishment(claim) => claim.as_ref().map(|claim| claim.work_id),
        }
    }

    fn restore(
        self,
        workflow: &mut MovementWorkflow,
        cycle_count: &mut crate::cycle_count::CycleCountWorkflow,
        picking: &mut crate::picking::PickingWorkflow,
        replenishment: &mut crate::replenishment::ReplenishmentWorkflow,
    ) {
        match self {
            Self::Putaway(claim) => workflow.restore_current_putaway_claim(claim),
            Self::InventoryRelocation(claim) => {
                workflow.restore_current_inventory_relocation_claim(claim)
            }
            Self::CycleCount(claim) => cycle_count.restore_current_claim(claim),
            Self::Picking(claim) => picking.restore_current_claim(claim),
            Self::Replenishment(claim) => replenishment.restore_current_claim(claim),
        }
    }
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
                    operation,
                    request_id,
                    response,
                } => self.handle_current_claim_response(context, operation, &request_id, response),
                NetworkEvent::Heartbeat {
                    operation,
                    task_id,
                    request_id,
                    response,
                } => self.handle_heartbeat_response(
                    context,
                    operation,
                    task_id,
                    &request_id,
                    response,
                ),
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
                NetworkEvent::OutboundLoadLookup {
                    barcode,
                    request_id,
                    response,
                } => self.handle_outbound_load_lookup(&barcode, &request_id, response),
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
        if matches!(record.draft.command, RfCommand::CycleCount(_)) {
            self.restore_cycle_count_command(context, record);
            return;
        }
        if matches!(record.draft.command, RfCommand::Picking(_)) {
            self.restore_picking_command(context, record);
            return;
        }
        if matches!(record.draft.command, RfCommand::Replenishment(_)) {
            self.restore_replenishment_command(context, record);
            return;
        }
        if matches!(record.draft.command, RfCommand::OutboundLoad(_)) {
            self.restore_outbound_load_command(record);
            return;
        }
        if let Some(operation) = record.draft.command.movement_operation() {
            self.work_mode = super::WorkMode::from(operation);
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

    fn restore_cycle_count_command(
        &mut self,
        context: &egui::Context,
        record: DurableCommandRecord,
    ) {
        self.work_mode = super::WorkMode::Count;
        match record.status {
            CommandStatus::Persisted => {
                let transition = self
                    .cycle_count
                    .restore_ready_command(record.record_id, record.draft);
                self.emit_count_transition(transition);
            }
            CommandStatus::Ambiguous | CommandStatus::Retryable => {
                self.cycle_count.restore_ambiguous_command(
                    record.record_id,
                    record.draft,
                    "The server may have received the saved count. Check it before continuing.",
                );
            }
            CommandStatus::ResponseRecorded => {
                let response = record.response.clone();
                self.cycle_count.restore_ambiguous_command(
                    record.record_id,
                    record.draft.clone(),
                    "Applying the saved count result.",
                );
                match response {
                    Some(response) => {
                        let scope = record.scope.clone();
                        self.apply_recorded_response(&scope, record, response);
                        if self.cycle_count.activity()
                            != crate::workflow::Activity::ReconcileRequired
                        {
                            self.session_gate = SessionGate::Recovering;
                            self.request_current_claim_for_operation(
                                context,
                                ClaimOperation::CycleCount,
                            );
                        }
                    }
                    None => self
                        .cycle_count
                        .require_reconciliation("The saved count response is incomplete.".into()),
                }
            }
            CommandStatus::ReconcileRequired | CommandStatus::Dispatching => {
                self.cycle_count.require_reconciliation(
                    "Saved count work needs supervisor review before inventory changes.".into(),
                );
            }
            CommandStatus::Completed | CommandStatus::Rejected => {}
        }
    }

    fn restore_picking_command(&mut self, context: &egui::Context, record: DurableCommandRecord) {
        self.work_mode = super::WorkMode::Pick;
        match record.status {
            CommandStatus::Persisted => {
                let transition = self
                    .picking
                    .restore_ready_command(record.record_id, record.draft);
                self.emit_pick_transition(transition);
            }
            CommandStatus::Ambiguous | CommandStatus::Retryable => {
                self.picking.restore_ambiguous_command(
                    record.record_id,
                    record.draft,
                    "The server may have received the saved pick. Check it before continuing.",
                );
            }
            CommandStatus::ResponseRecorded => {
                let response = record.response.clone();
                self.picking.restore_ambiguous_command(
                    record.record_id,
                    record.draft.clone(),
                    "Applying the saved pick result.",
                );
                match response {
                    Some(response) => {
                        let scope = record.scope.clone();
                        self.apply_recorded_response(&scope, record, response);
                        if self.picking.activity() != crate::workflow::Activity::ReconcileRequired {
                            self.session_gate = SessionGate::Recovering;
                            self.request_current_claim_for_operation(
                                context,
                                ClaimOperation::Picking,
                            );
                        }
                    }
                    None => self
                        .picking
                        .require_reconciliation("The saved pick response is incomplete.".into()),
                }
            }
            CommandStatus::ReconcileRequired | CommandStatus::Dispatching => {
                self.picking.require_reconciliation(
                    "Saved pick work needs supervisor review before inventory changes.".into(),
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
        self.request_current_claim_for_operation(context, ClaimOperation::Putaway);
    }

    pub(super) fn request_current_claim_for_operation(
        &mut self,
        context: &egui::Context,
        operation: ClaimOperation,
    ) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let request_id = format!("rf-{}", uuid::Uuid::new_v4());
        let transport = AuthenticatedTransport {
            endpoint: &session.endpoint,
            token: &session.token,
            scope: &session.scope,
        };
        let request = match build_current_claim_request(&transport, operation, &request_id) {
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
            operation,
            request_id,
            self.network_tx.clone(),
            context.clone(),
        );
    }

    fn handle_current_claim_response(
        &mut self,
        context: &egui::Context,
        operation: ClaimOperation,
        request_id: &str,
        response: Result<NetworkResponse, String>,
    ) {
        if self.expected_claim_request_id.as_deref() != Some(request_id) {
            return;
        }
        self.expected_claim_request_id = None;
        let lease_check_task_id = self.lease_check_task_id.take();
        let lease_rejection_check = std::mem::take(&mut self.lease_rejection_check);
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                self.session_gate = SessionGate::Ready;
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
            self.session_gate = SessionGate::Ready;
            self.clear_claim_heartbeat();
            if let Some(task_id) = lease_check_task_id {
                self.require_reauthentication_for_task(task_id);
            } else {
                self.require_reauthentication(None);
            }
            return;
        }
        if !(200..300).contains(&response.status) {
            if lease_check_task_id.is_none()
                && response.status == 409
                && let Some(next_operation) = next_claim_operation_after_conflict(operation)
            {
                self.request_current_claim_for_operation(context, next_operation);
                return;
            }
            self.session_gate = SessionGate::Ready;
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
        let claim = match operation {
            ClaimOperation::Putaway => {
                decode_claim_response(&response.body).map(CurrentMovementClaim::Putaway)
            }
            ClaimOperation::InventoryRelocation => decode_relocation_claim_response(&response.body)
                .map(CurrentMovementClaim::InventoryRelocation),
            ClaimOperation::CycleCount => decode_cycle_count_claim_response(&response.body)
                .map(CurrentMovementClaim::CycleCount),
            ClaimOperation::Picking => {
                decode_pick_claim_response(&response.body).map(CurrentMovementClaim::Picking)
            }
            ClaimOperation::Replenishment => decode_replenishment_claim_response(&response.body)
                .map(CurrentMovementClaim::Replenishment),
        };
        match claim {
            Ok(claim) => {
                if lease_check_task_id.is_none()
                    && operation == ClaimOperation::Putaway
                    && claim.is_none()
                {
                    self.request_current_claim_for_operation(
                        context,
                        ClaimOperation::InventoryRelocation,
                    );
                    return;
                }
                if lease_check_task_id.is_none()
                    && operation == ClaimOperation::InventoryRelocation
                    && claim.is_none()
                {
                    self.request_current_claim_for_operation(context, ClaimOperation::CycleCount);
                    return;
                }
                if lease_check_task_id.is_none()
                    && operation == ClaimOperation::CycleCount
                    && claim.is_none()
                {
                    self.request_current_claim_for_operation(context, ClaimOperation::Picking);
                    return;
                }
                if lease_check_task_id.is_none()
                    && operation == ClaimOperation::Picking
                    && claim.is_none()
                {
                    self.request_current_claim_for_operation(
                        context,
                        ClaimOperation::Replenishment,
                    );
                    return;
                }
                self.session_gate = SessionGate::Ready;
                self.clear_claim_heartbeat();
                if let Some(expected_task_id) = lease_check_task_id {
                    if claim.task_id() == Some(expected_task_id) && !lease_rejection_check {
                        claim.restore(
                            &mut self.workflow,
                            &mut self.cycle_count,
                            &mut self.picking,
                            &mut self.replenishment,
                        );
                        self.work_mode = match operation {
                            ClaimOperation::Putaway => super::WorkMode::Putaway,
                            ClaimOperation::InventoryRelocation => super::WorkMode::Relocate,
                            ClaimOperation::CycleCount => super::WorkMode::Count,
                            ClaimOperation::Picking => super::WorkMode::Pick,
                            ClaimOperation::Replenishment => super::WorkMode::Replenish,
                        };
                    } else {
                        match operation {
                            ClaimOperation::Putaway => {
                                self.workflow.restore_current_putaway_claim(None)
                            }
                            ClaimOperation::InventoryRelocation => self
                                .workflow
                                .restore_current_inventory_relocation_claim(None),
                            ClaimOperation::CycleCount => {
                                self.cycle_count.restore_current_claim(None)
                            }
                            ClaimOperation::Picking => self.picking.restore_current_claim(None),
                            ClaimOperation::Replenishment => {
                                self.replenishment.restore_current_claim(None)
                            }
                        }
                        self.work_mode = match operation {
                            ClaimOperation::Putaway => super::WorkMode::Putaway,
                            ClaimOperation::InventoryRelocation => super::WorkMode::Relocate,
                            ClaimOperation::CycleCount => super::WorkMode::Count,
                            ClaimOperation::Picking => super::WorkMode::Pick,
                            ClaimOperation::Replenishment => super::WorkMode::Replenish,
                        };
                        match operation {
                            ClaimOperation::CycleCount => self.cycle_count.require_reconciliation(
                                "This count is no longer assigned. Contact a supervisor.".into(),
                            ),
                            ClaimOperation::Picking => self.picking.require_reconciliation(
                                "This pick is no longer assigned. Do not move its inventory. Contact a supervisor."
                                    .into(),
                            ),
                            ClaimOperation::Replenishment => self
                                .replenishment
                                .require_reconciliation(
                                    "This replenishment is no longer assigned. Do not move its inventory. Contact a supervisor."
                                        .into(),
                                ),
                            ClaimOperation::Putaway | ClaimOperation::InventoryRelocation => {
                                self.workflow.require_reconciliation(
                                    "This task is no longer assigned. Do not move its inventory. Contact a supervisor."
                                        .into(),
                                );
                            }
                        }
                    }
                } else if !claim.is_none() {
                    claim.restore(
                        &mut self.workflow,
                        &mut self.cycle_count,
                        &mut self.picking,
                        &mut self.replenishment,
                    );
                    self.work_mode = match operation {
                        ClaimOperation::Putaway => super::WorkMode::Putaway,
                        ClaimOperation::InventoryRelocation => super::WorkMode::Relocate,
                        ClaimOperation::CycleCount => super::WorkMode::Count,
                        ClaimOperation::Picking => super::WorkMode::Pick,
                        ClaimOperation::Replenishment => super::WorkMode::Replenish,
                    };
                } else {
                    self.workflow.restore_current_putaway_claim(None);
                    self.cycle_count.restore_current_claim(None);
                    self.picking.restore_current_claim(None);
                    self.replenishment.restore_current_claim(None);
                    self.work_mode = super::WorkMode::Putaway;
                }
                self.connectivity_notice = None;
                self.reauth_scope = None;
                self.reauth_notice = None;
            }
            Err(_) => {
                self.session_gate = SessionGate::Ready;
                self.clear_claim_heartbeat();
                match operation {
                    ClaimOperation::CycleCount => self.cycle_count.require_reconciliation(
                        "The server returned invalid current-count data.".into(),
                    ),
                    ClaimOperation::Picking => self.picking.require_reconciliation(
                        "The server returned invalid current-pick data.".into(),
                    ),
                    ClaimOperation::Replenishment => self.replenishment.require_reconciliation(
                        "The server returned invalid current-replenishment data.".into(),
                    ),
                    ClaimOperation::Putaway | ClaimOperation::InventoryRelocation => {
                        self.workflow.require_reconciliation(
                            "The server returned invalid current-work data.".into(),
                        )
                    }
                }
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
        let operation = if self.work_mode == super::WorkMode::Pick || self.picking.claim().is_some()
        {
            ClaimOperation::Picking
        } else if self.work_mode == super::WorkMode::Replenish
            || self.replenishment.claim().is_some()
        {
            ClaimOperation::Replenishment
        } else if self.work_mode == super::WorkMode::Count || self.cycle_count.claim().is_some() {
            ClaimOperation::CycleCount
        } else {
            self.workflow.operation().into()
        };
        self.request_current_claim_for_operation(context, operation);
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
            let is_cycle_count = matches!(draft.command, RfCommand::CycleCount(_));
            let is_picking = matches!(draft.command, RfCommand::Picking(_));
            let is_replenishment = matches!(draft.command, RfCommand::Replenishment(_));
            let is_outbound_load = matches!(draft.command, RfCommand::OutboundLoad(_));
            let Some(store) = self.command_store.as_mut() else {
                if is_outbound_load {
                    self.outbound_load
                        .require_reconciliation("Durable device storage is unavailable".into());
                } else if is_replenishment {
                    self.replenishment
                        .require_reconciliation("Durable device storage is unavailable".into());
                } else {
                    self.workflow
                        .require_reconciliation("Durable device storage is unavailable".into());
                }
                continue;
            };
            let Some(scope) = self.execution_scope.as_ref() else {
                if is_outbound_load {
                    self.outbound_load.require_reconciliation(
                        "The carton move cannot be stored without an authenticated device scope"
                            .into(),
                    );
                } else if is_replenishment {
                    self.replenishment.require_reconciliation(
                        "The replenishment cannot be stored without an authenticated device scope"
                            .into(),
                    );
                } else {
                    self.workflow.require_reconciliation(
                        "The command cannot be stored without an authenticated device scope".into(),
                    );
                }
                continue;
            };
            let command_id = draft.command_id.clone();
            match store.persist(scope, draft) {
                Ok(record) => {
                    if is_cycle_count {
                        let transition = self
                            .cycle_count
                            .command_persisted(&command_id, record.record_id);
                        self.emit_count_transition(transition);
                    } else if is_picking {
                        let transition = self
                            .picking
                            .command_persisted(&command_id, record.record_id);
                        self.emit_pick_transition(transition);
                    } else if is_replenishment {
                        let transition = self
                            .replenishment
                            .command_persisted(&command_id, record.record_id);
                        self.emit_replenishment_transition(transition);
                    } else if is_outbound_load {
                        let transition = self
                            .outbound_load
                            .command_persisted(&command_id, record.record_id);
                        self.emit_outbound_load_transition(transition);
                    } else {
                        let transition = self
                            .workflow
                            .command_persisted(&command_id, record.record_id);
                        self.emit_transition(transition);
                    }
                }
                Err(error) => {
                    if is_cycle_count {
                        self.cycle_count.require_reconciliation(format!(
                            "The count could not be stored durably: {error}"
                        ));
                    } else if is_picking {
                        self.picking.require_reconciliation(format!(
                            "The pick could not be stored durably: {error}"
                        ));
                    } else if is_replenishment {
                        self.replenishment.require_reconciliation(format!(
                            "The replenishment could not be stored durably: {error}"
                        ));
                    } else if is_outbound_load {
                        self.outbound_load.require_reconciliation(format!(
                            "The carton move could not be stored durably: {error}"
                        ));
                    } else {
                        self.workflow.require_reconciliation(format!(
                            "The command could not be stored durably: {error}"
                        ));
                    }
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
                    if self.picking.owns_record(record_id) {
                        self.picking.require_reconciliation(
                            "The saved pick could not enter network dispatch.".into(),
                        );
                    } else if self.cycle_count.owns_record(record_id) {
                        self.cycle_count.require_reconciliation(
                            "The saved count could not enter network dispatch.".into(),
                        );
                    } else if self.replenishment.owns_record(record_id) {
                        self.replenishment.require_reconciliation(
                            "The saved replenishment could not enter network dispatch.".into(),
                        );
                    } else if self.outbound_load.owns_record(record_id) {
                        self.outbound_load.require_reconciliation(
                            "The saved carton move could not enter network dispatch.".into(),
                        );
                    } else {
                        self.workflow.require_reconciliation(
                            "The saved command could not enter network dispatch.".into(),
                        );
                    }
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
                    if self.picking.owns_record(record_id) {
                        self.picking.require_reconciliation(
                            "The saved pick failed its integrity check.".into(),
                        );
                    } else if self.cycle_count.owns_record(record_id) {
                        self.cycle_count.require_reconciliation(
                            "The saved count failed its integrity check.".into(),
                        );
                    } else if self.replenishment.owns_record(record_id) {
                        self.replenishment.require_reconciliation(
                            "The saved replenishment failed its integrity check.".into(),
                        );
                    } else if self.outbound_load.owns_record(record_id) {
                        self.outbound_load.require_reconciliation(
                            "The saved carton move failed its integrity check.".into(),
                        );
                    } else {
                        self.workflow.require_reconciliation(
                            "The saved command failed its integrity check.".into(),
                        );
                    }
                    continue;
                }
            };
            if matches!(attempt.command.draft.command, RfCommand::CycleCount(_)) {
                self.cycle_count.dispatch_started(record_id);
            } else if matches!(attempt.command.draft.command, RfCommand::Picking(_)) {
                self.picking.dispatch_started(record_id);
            } else if matches!(attempt.command.draft.command, RfCommand::Replenishment(_)) {
                self.replenishment.dispatch_started(record_id);
            } else if matches!(attempt.command.draft.command, RfCommand::OutboundLoad(_)) {
                self.outbound_load.dispatch_started(record_id);
            } else {
                self.workflow.dispatch_started(record_id);
            }
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
            } else if self.picking.owns_record(record_id) {
                self.picking.require_reconciliation(
                    "The authenticated device scope was lost during pick dispatch.".into(),
                );
            } else if self.cycle_count.owns_record(record_id) {
                self.cycle_count.require_reconciliation(
                    "The authenticated device scope was lost during count dispatch.".into(),
                );
            } else if self.replenishment.owns_record(record_id) {
                self.replenishment.require_reconciliation(
                    "The authenticated device scope was lost during replenishment dispatch.".into(),
                );
            } else if self.outbound_load.owns_record(record_id) {
                self.outbound_load.require_reconciliation(
                    "The authenticated device scope was lost during carton dispatch.".into(),
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
                    } else if matches!(stored.draft.command, RfCommand::CycleCount(_)) {
                        self.cycle_count.dispatch_ambiguous(
                            record_id,
                            "The server may have received the count. Check it before continuing.",
                        );
                    } else if matches!(stored.draft.command, RfCommand::Picking(_)) {
                        self.picking.dispatch_ambiguous(
                            record_id,
                            "The server may have received the pick. Check it before continuing.",
                        );
                    } else if matches!(stored.draft.command, RfCommand::Replenishment(_)) {
                        self.replenishment.dispatch_ambiguous(
                            record_id,
                            "The server may have received the replenishment. Check it before continuing.",
                        );
                    } else if matches!(stored.draft.command, RfCommand::OutboundLoad(_)) {
                        self.outbound_load.dispatch_ambiguous(
                            record_id,
                            "The server may have received the carton move. Check it before continuing.",
                        );
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
                    } else if self.picking.owns_record(record_id) {
                        self.picking.require_reconciliation(
                            "The interrupted pick could not be saved for recovery.".into(),
                        );
                    } else if self.cycle_count.owns_record(record_id) {
                        self.cycle_count.require_reconciliation(
                            "The interrupted count could not be saved for recovery.".into(),
                        );
                    } else if self.replenishment.owns_record(record_id) {
                        self.replenishment.require_reconciliation(
                            "The interrupted replenishment could not be saved for recovery.".into(),
                        );
                    } else if self.outbound_load.owns_record(record_id) {
                        self.outbound_load.require_reconciliation(
                            "The interrupted carton move could not be saved for recovery.".into(),
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
                } else if self.cycle_count.owns_record(record_id) {
                    self.cycle_count.require_reconciliation(
                        "The retryable count result could not be saved.".into(),
                    );
                } else if self.picking.owns_record(record_id) {
                    self.picking.require_reconciliation(
                        "The retryable pick result could not be saved.".into(),
                    );
                } else if self.replenishment.owns_record(record_id) {
                    self.replenishment.require_reconciliation(
                        "The retryable replenishment result could not be saved.".into(),
                    );
                } else if self.outbound_load.owns_record(record_id) {
                    self.outbound_load.require_reconciliation(
                        "The retryable carton-move result could not be saved.".into(),
                    );
                } else {
                    self.workflow.require_reconciliation(
                        "The retryable server result could not be saved.".into(),
                    );
                }
                return;
            };
            let is_receiving = stored.operation == CommandOperation::ExpectedReceiptConfirmation;
            let is_cycle_count = matches!(stored.draft.command, RfCommand::CycleCount(_));
            let is_picking = matches!(stored.draft.command, RfCommand::Picking(_));
            let is_replenishment = matches!(stored.draft.command, RfCommand::Replenishment(_));
            let is_outbound_load = matches!(stored.draft.command, RfCommand::OutboundLoad(_));
            if response.status == 401 {
                if is_receiving {
                    if let Some(runtime) = self.receiving_command.as_mut() {
                        runtime.phase = ReceivingCommandPhase::Ambiguous;
                        runtime.message = Some(
                            "Sign in with the same operator account to recover this receipt."
                                .into(),
                        );
                    }
                } else if is_cycle_count {
                    self.cycle_count.restore_ambiguous_command(
                        record_id,
                        stored.draft,
                        "Sign in with the same operator account to recover this saved count.",
                    );
                } else if is_picking {
                    self.picking.restore_ambiguous_command(
                        record_id,
                        stored.draft,
                        "Sign in with the same operator account to recover this saved pick.",
                    );
                } else if is_replenishment {
                    self.replenishment.restore_ambiguous_command(
                        record_id,
                        stored.draft,
                        "Sign in with the same operator account to recover this saved replenishment.",
                    );
                } else if is_outbound_load {
                    self.outbound_load.restore_ambiguous_command(
                        record_id,
                        stored.draft,
                        "Sign in with the same operator account to recover this saved carton move.",
                    );
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
                } else if is_cycle_count {
                    self.cycle_count.dispatch_ambiguous(
                        record_id,
                        "The service is temporarily unavailable. Check the saved count again.",
                    );
                } else if is_picking {
                    self.picking.dispatch_ambiguous(
                        record_id,
                        "The service is temporarily unavailable. Check the saved pick again.",
                    );
                } else if is_replenishment {
                    self.replenishment.dispatch_ambiguous(
                        record_id,
                        "The service is temporarily unavailable. Check the saved replenishment again.",
                    );
                } else if is_outbound_load {
                    self.outbound_load.dispatch_ambiguous(
                        record_id,
                        "The service is temporarily unavailable. Check the saved carton move again.",
                    );
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
                } else if self.picking.owns_record(record_id) {
                    self.picking.require_reconciliation(
                        "The pick result could not be stored durably.".into(),
                    );
                } else if self.cycle_count.owns_record(record_id) {
                    self.cycle_count.require_reconciliation(
                        "The count result could not be stored durably.".into(),
                    );
                } else if self.replenishment.owns_record(record_id) {
                    self.replenishment.require_reconciliation(
                        "The replenishment result could not be stored durably.".into(),
                    );
                } else if self.outbound_load.owns_record(record_id) {
                    self.outbound_load.require_reconciliation(
                        "The carton-move result could not be stored durably.".into(),
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

    pub(super) fn apply_recorded_response(
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
                    let is_cycle_count = matches!(
                        &outcome,
                        crate::workflow::CommandOutcome::CycleCountClaimed(_)
                            | crate::workflow::CommandOutcome::CycleCountConfirmed { .. }
                            | crate::workflow::CommandOutcome::CycleCountReleased { .. }
                    );
                    let is_picking = matches!(
                        &outcome,
                        crate::workflow::CommandOutcome::PickClaimed(_)
                            | crate::workflow::CommandOutcome::PickConfirmed { .. }
                            | crate::workflow::CommandOutcome::PickShortageReported(_)
                            | crate::workflow::CommandOutcome::PickReleased { .. }
                    );
                    let is_replenishment = matches!(
                        &outcome,
                        crate::workflow::CommandOutcome::ReplenishmentClaimed(_)
                            | crate::workflow::CommandOutcome::ReplenishmentConfirmed(_)
                            | crate::workflow::CommandOutcome::ReplenishmentReleased { .. }
                    );
                    let is_outbound_load = matches!(
                        &outcome,
                        crate::workflow::CommandOutcome::OutboundCartonMoved(_)
                    );
                    if is_replenishment
                        && (!self.replenishment.accepts_outcome(record_id, &outcome)
                            || matches!(
                                &outcome,
                                crate::workflow::CommandOutcome::ReplenishmentConfirmed(result)
                                    if result.confirmed_by != scope.operator_id
                            ))
                    {
                        self.require_replenishment_record_reconciliation(
                            scope,
                            record_id,
                            "The replenishment result conflicts with the saved command or task.",
                        );
                        return;
                    }
                    if is_outbound_load
                        && (!self.outbound_load.accepts_outcome(record_id, &outcome)
                            || matches!(
                                &outcome,
                                crate::workflow::CommandOutcome::OutboundCartonMoved(result)
                                    if result.movement.moved_by != scope.operator_id
                            ))
                    {
                        self.require_outbound_load_record_reconciliation(
                            scope,
                            record_id,
                            "The carton-move result conflicts with the saved load or scans.",
                        );
                        return;
                    }
                    if self
                        .finalize_record(scope, record_id, CommandStatus::Completed, None)
                        .is_some()
                    {
                        if is_cycle_count {
                            self.cycle_count
                                .durable_outcome_recorded(record_id, outcome);
                        } else if is_picking {
                            self.picking.durable_outcome_recorded(record_id, outcome);
                        } else if is_replenishment {
                            self.replenishment
                                .durable_outcome_recorded(record_id, outcome);
                        } else if is_outbound_load {
                            self.outbound_load
                                .durable_outcome_recorded(record_id, outcome);
                        } else {
                            self.workflow.durable_outcome_recorded(record_id, outcome);
                        }
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
                    } else if matches!(recorded.draft.command, RfCommand::Picking(_)) {
                        self.require_pick_record_reconciliation(
                            scope,
                            record_id,
                            "The server returned an invalid pick result.",
                        );
                    } else if matches!(recorded.draft.command, RfCommand::CycleCount(_)) {
                        self.require_count_record_reconciliation(
                            scope,
                            record_id,
                            "The server returned an invalid count result.",
                        );
                    } else if matches!(recorded.draft.command, RfCommand::Replenishment(_)) {
                        self.require_replenishment_record_reconciliation(
                            scope,
                            record_id,
                            "The server returned an invalid replenishment result.",
                        );
                    } else if matches!(recorded.draft.command, RfCommand::OutboundLoad(_)) {
                        self.require_outbound_load_record_reconciliation(
                            scope,
                            record_id,
                            "The server returned an invalid carton-move result.",
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
                if matches!(recorded.draft.command, RfCommand::CycleCount(_)) {
                    self.cycle_count
                        .durable_rejection_recorded(record_id, message);
                } else if matches!(recorded.draft.command, RfCommand::Picking(_)) {
                    self.picking.durable_rejection_recorded(record_id, message);
                } else if matches!(recorded.draft.command, RfCommand::Replenishment(_)) {
                    self.replenishment
                        .durable_rejection_recorded(record_id, message);
                } else if matches!(recorded.draft.command, RfCommand::OutboundLoad(_)) {
                    self.outbound_load
                        .durable_rejection_recorded(record_id, message);
                } else {
                    self.workflow.durable_rejection_recorded(record_id, message);
                }
            }
        } else {
            let message = support_message(
                "Work needs review. Do not move or scan inventory.",
                request_id,
            );
            if recorded.operation == CommandOperation::ExpectedReceiptConfirmation {
                self.require_receiving_record_reconciliation(scope, record_id, &message);
            } else if matches!(recorded.draft.command, RfCommand::CycleCount(_)) {
                self.require_count_record_reconciliation(scope, record_id, &message);
            } else if matches!(recorded.draft.command, RfCommand::Picking(_)) {
                self.require_pick_record_reconciliation(scope, record_id, &message);
            } else if matches!(recorded.draft.command, RfCommand::Replenishment(_)) {
                self.require_replenishment_record_reconciliation(scope, record_id, &message);
            } else if matches!(recorded.draft.command, RfCommand::OutboundLoad(_)) {
                self.require_outbound_load_record_reconciliation(scope, record_id, &message);
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
                } else if self.cycle_count.owns_record(record_id) {
                    self.cycle_count.require_reconciliation(
                        "The durable count command could not be finalized.".into(),
                    );
                } else if self.picking.owns_record(record_id) {
                    self.picking.require_reconciliation(
                        "The durable pick command could not be finalized.".into(),
                    );
                } else if self.replenishment.owns_record(record_id) {
                    self.replenishment.require_reconciliation(
                        "The durable replenishment command could not be finalized.".into(),
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
            } else if self.picking.owns_record(record_id) {
                self.picking.require_reconciliation(message.into());
            } else if self.cycle_count.owns_record(record_id) {
                self.cycle_count.require_reconciliation(message.into());
            } else if self.replenishment.owns_record(record_id) {
                self.replenishment.require_reconciliation(message.into());
            } else if self.outbound_load.owns_record(record_id) {
                self.outbound_load.require_reconciliation(message.into());
            } else {
                self.workflow.require_reconciliation(message.into());
            }
        }
    }

    fn require_count_record_reconciliation(
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
            self.cycle_count.require_reconciliation(message.into());
        }
    }

    fn require_pick_record_reconciliation(
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
            self.picking.require_reconciliation(message.into());
        }
    }

    fn require_replenishment_record_reconciliation(
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
            self.replenishment.require_reconciliation(message.into());
        }
    }

    fn require_outbound_load_record_reconciliation(
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
            self.outbound_load.require_reconciliation(message.into());
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

pub(super) const fn next_claim_operation_after_conflict(
    operation: ClaimOperation,
) -> Option<ClaimOperation> {
    match operation {
        ClaimOperation::Putaway => Some(ClaimOperation::InventoryRelocation),
        ClaimOperation::InventoryRelocation => Some(ClaimOperation::CycleCount),
        ClaimOperation::CycleCount | ClaimOperation::Picking | ClaimOperation::Replenishment => {
            None
        }
    }
}
