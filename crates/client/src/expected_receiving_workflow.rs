use chrono::DateTime;
use wareboxes_api_contract::v1::{
    ConfirmExpectedReceiptRequest, ErrorReason, ExpectedReceiptConfirmationResponse,
    ExpectedReceiptDisposition, ExpectedReceiptExceptionReason, ExpectedReceiptLine,
    ExpectedReceivingSessionResponse,
};

use crate::api::expected_receiving::ExpectedReceivingTransportFailure;
use crate::api::{
    ExpectedReceivingCommand, ExpectedReceivingRequest, ExpectedReceivingTransportEvent,
    ExpectedReceivingTransportOutcome,
};

const REQUEST_TIMEOUT_SECONDS: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedReceivingActivity {
    Uninitialized,
    Ready,
    Pending,
    Retryable,
    ReconcileRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedReceivingScanStage {
    LoadId,
    ItemBarcode,
    ReceivingLocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedReceivingApplyResult {
    Applied,
    Ignored,
    ReloadRequired(i64),
    Completed(String),
}

#[derive(Debug, Clone)]
pub struct ExpectedReceivingWorkflowState {
    activity: ExpectedReceivingActivity,
    session: Option<ExpectedReceivingSessionResponse>,
    pending: Option<ExpectedReceivingRequest>,
    pending_started_at: Option<f64>,
    clock_now: Option<f64>,
    selected_line_id: Option<i64>,
    disposition: ExpectedReceiptDisposition,
    verified_item_barcode: Option<String>,
    receiving_location_verified: bool,
    load_id_draft: String,
    scan_draft: String,
    quantity_draft: String,
    license_plate_draft: String,
    lot_draft: String,
    serial_draft: String,
    expiration_draft: String,
    reason: ExpectedReceiptExceptionReason,
    note_draft: String,
    scan_error: Option<String>,
    request_error: Option<String>,
    completion: Option<String>,
}

impl Default for ExpectedReceivingWorkflowState {
    fn default() -> Self {
        Self {
            activity: ExpectedReceivingActivity::Uninitialized,
            session: None,
            pending: None,
            pending_started_at: None,
            clock_now: None,
            selected_line_id: None,
            disposition: ExpectedReceiptDisposition::Received,
            verified_item_barcode: None,
            receiving_location_verified: false,
            load_id_draft: String::new(),
            scan_draft: String::new(),
            quantity_draft: "1".into(),
            license_plate_draft: String::new(),
            lot_draft: String::new(),
            serial_draft: String::new(),
            expiration_draft: String::new(),
            reason: ExpectedReceiptExceptionReason::Damaged,
            note_draft: String::new(),
            scan_error: None,
            request_error: None,
            completion: None,
        }
    }
}

impl ExpectedReceivingWorkflowState {
    pub fn activity(&self) -> ExpectedReceivingActivity {
        self.activity
    }

    /// Advances the reducer clock and fails a hung request into exact retry.
    pub fn tick(&mut self, now: f64) -> bool {
        if !now.is_finite() {
            return false;
        }
        if self.clock_now.is_some_and(|previous| now < previous) {
            self.pending_started_at = self
                .activity
                .eq(&ExpectedReceivingActivity::Pending)
                .then_some(now);
        }
        self.clock_now = Some(now);
        if self.activity != ExpectedReceivingActivity::Pending {
            self.pending_started_at = None;
            return false;
        }
        let started_at = *self.pending_started_at.get_or_insert(now);
        if now - started_at < REQUEST_TIMEOUT_SECONDS {
            return false;
        }

        self.activity = ExpectedReceivingActivity::Retryable;
        self.pending_started_at = None;
        self.request_error =
            Some("Expected receiving request timed out; retry the same command".into());
        true
    }

    pub fn session(&self) -> Option<&ExpectedReceivingSessionResponse> {
        self.session.as_ref()
    }

    pub fn active_line(&self) -> Option<&ExpectedReceiptLine> {
        let selected_line_id = self.selected_line_id?;
        self.session
            .as_ref()?
            .lines
            .iter()
            .find(|line| line.load_line_id == selected_line_id)
    }

    pub fn completion(&self) -> Option<&str> {
        self.completion.as_deref()
    }

    pub fn request_error(&self) -> Option<&str> {
        self.request_error.as_deref()
    }

    pub fn scan_error(&self) -> Option<&str> {
        self.scan_error.as_deref()
    }

    pub fn disposition(&self) -> ExpectedReceiptDisposition {
        self.disposition
    }

    pub fn select_disposition(&mut self, disposition: ExpectedReceiptDisposition) {
        if self.activity != ExpectedReceivingActivity::Ready || self.disposition == disposition {
            return;
        }
        self.disposition = disposition;
        self.clear_scan_verification();
        self.reason = ExpectedReceiptExceptionReason::Damaged;
        self.note_draft.clear();
        self.request_error = None;
    }

    pub fn selected_line_id(&self) -> Option<i64> {
        self.selected_line_id
    }

    pub fn select_line(&mut self, load_line_id: i64) -> bool {
        if self.activity != ExpectedReceivingActivity::Ready {
            return false;
        }
        let dimensions = self.session.as_ref().and_then(|session| {
            session
                .lines
                .iter()
                .find(|line| line.load_line_id == load_line_id)
                .map(|line| {
                    (
                        line.lot.clone().unwrap_or_default(),
                        line.serial.clone().unwrap_or_default(),
                        line.expiration.clone().unwrap_or_default(),
                    )
                })
        });
        let Some((lot, serial, expiration)) = dimensions else {
            self.request_error = Some("Expected receipt line is not available".into());
            return false;
        };

        self.selected_line_id = Some(load_line_id);
        self.clear_scan_verification();
        self.lot_draft = lot;
        self.serial_draft = serial;
        self.expiration_draft = expiration;
        self.quantity_draft = "1".into();
        self.license_plate_draft.clear();
        self.reason = ExpectedReceiptExceptionReason::Damaged;
        self.note_draft.clear();
        self.request_error = None;
        true
    }

    pub fn scan_stage(&self) -> Option<ExpectedReceivingScanStage> {
        if matches!(
            self.activity,
            ExpectedReceivingActivity::Pending
                | ExpectedReceivingActivity::Retryable
                | ExpectedReceivingActivity::ReconcileRequired
        ) {
            return None;
        }
        if self.session.is_none() {
            return Some(ExpectedReceivingScanStage::LoadId);
        }
        if self.disposition == ExpectedReceiptDisposition::Missing {
            return None;
        }
        if self.verified_item_barcode.is_none() {
            return Some(ExpectedReceivingScanStage::ItemBarcode);
        }
        if self.disposition == ExpectedReceiptDisposition::Received
            && !self.receiving_location_verified
        {
            return Some(ExpectedReceivingScanStage::ReceivingLocation);
        }
        None
    }

    pub fn load_id_draft_mut(&mut self) -> &mut String {
        &mut self.load_id_draft
    }

    pub fn scan_draft_mut(&mut self) -> &mut String {
        &mut self.scan_draft
    }

    pub fn quantity_draft_mut(&mut self) -> &mut String {
        &mut self.quantity_draft
    }

    pub fn license_plate_draft_mut(&mut self) -> &mut String {
        &mut self.license_plate_draft
    }

    pub fn lot_draft_mut(&mut self) -> &mut String {
        &mut self.lot_draft
    }

    pub fn serial_draft_mut(&mut self) -> &mut String {
        &mut self.serial_draft
    }

    pub fn expiration_draft_mut(&mut self) -> &mut String {
        &mut self.expiration_draft
    }

    pub fn note_draft_mut(&mut self) -> &mut String {
        &mut self.note_draft
    }

    pub fn reason(&self) -> ExpectedReceiptExceptionReason {
        self.reason
    }

    pub fn select_reason(&mut self, reason: ExpectedReceiptExceptionReason) {
        if self.activity == ExpectedReceivingActivity::Ready {
            self.reason = reason;
            self.request_error = None;
        }
    }

    pub fn submit_scan(
        &mut self,
        request_id: String,
        _idempotency_key: String,
    ) -> Option<ExpectedReceivingRequest> {
        match self.scan_stage()? {
            ExpectedReceivingScanStage::LoadId => self.begin_session(request_id),
            ExpectedReceivingScanStage::ItemBarcode => {
                self.submit_item_scan();
                None
            }
            ExpectedReceivingScanStage::ReceivingLocation => {
                self.submit_receiving_location_scan();
                None
            }
        }
    }

    pub fn begin_session(&mut self, request_id: String) -> Option<ExpectedReceivingRequest> {
        if !matches!(
            self.activity,
            ExpectedReceivingActivity::Uninitialized | ExpectedReceivingActivity::Ready
        ) || self.session.is_some()
        {
            return None;
        }
        let load_id = match self.load_id_draft.trim().parse::<i64>() {
            Ok(load_id) if load_id > 0 => load_id,
            _ => {
                self.request_error = Some("Load ID must be a positive whole number".into());
                return None;
            }
        };
        self.completion = None;
        Some(self.begin(ExpectedReceivingRequest {
            request_id,
            command: ExpectedReceivingCommand::LoadSession { load_id },
        }))
    }

    pub fn begin_confirmation(
        &mut self,
        request_id: String,
        idempotency_key: String,
    ) -> Option<ExpectedReceivingRequest> {
        if self.activity != ExpectedReceivingActivity::Ready {
            return None;
        }
        let line = self.active_line()?.clone();
        let quantity = self.parse_quantity(line.remaining_quantity)?;
        let body = self.build_confirmation(quantity)?;

        Some(self.begin(ExpectedReceivingRequest {
            request_id,
            command: ExpectedReceivingCommand::Confirm {
                load_line_id: line.load_line_id,
                body,
                idempotency_key,
            },
        }))
    }

    pub fn retry(&mut self, request_id: String) -> Option<ExpectedReceivingRequest> {
        if self.activity != ExpectedReceivingActivity::Retryable {
            return None;
        }
        let mut request = self.pending.clone()?;
        request.request_id = request_id;
        Some(self.begin(request))
    }

    pub fn reconcile(&mut self, request_id: String) -> Option<ExpectedReceivingRequest> {
        if !matches!(
            self.activity,
            ExpectedReceivingActivity::Ready | ExpectedReceivingActivity::ReconcileRequired
        ) {
            return None;
        }
        let load_id = self
            .session
            .as_ref()
            .map(|session| session.load_id)
            .or_else(|| pending_load_id(self.pending.as_ref()))?;
        Some(self.begin(ExpectedReceivingRequest {
            request_id,
            command: ExpectedReceivingCommand::LoadSession { load_id },
        }))
    }

    pub fn reset_for_next_load(&mut self) {
        if self.activity == ExpectedReceivingActivity::Pending {
            return;
        }
        *self = Self {
            activity: ExpectedReceivingActivity::Ready,
            ..Self::default()
        };
    }

    pub fn apply(
        &mut self,
        event: ExpectedReceivingTransportEvent,
    ) -> ExpectedReceivingApplyResult {
        let Some(pending) = self.pending.as_ref() else {
            return ExpectedReceivingApplyResult::Ignored;
        };
        if pending.request_id != event.request.request_id
            || pending.command != event.request.command
        {
            return ExpectedReceivingApplyResult::Ignored;
        }

        self.pending_started_at = None;
        let command = event.request.command;
        match event.outcome {
            Ok(outcome) => self.apply_success(command, outcome),
            Err(failure) => self.apply_failure(&command, failure),
        }
    }

    fn apply_failure(
        &mut self,
        command: &ExpectedReceivingCommand,
        failure: ExpectedReceivingTransportFailure,
    ) -> ExpectedReceivingApplyResult {
        let is_load_session = matches!(command, ExpectedReceivingCommand::LoadSession { .. });
        let has_active_session = self.session.is_some();
        let status_is_drift = matches!(failure.status, Some(404 | 409))
            || failure.error.as_ref().is_some_and(|error| {
                matches!(error.reason, ErrorReason::Conflict | ErrorReason::NotFound)
            });
        let malformed_success = failure
            .status
            .is_some_and(|status| (200..300).contains(&status))
            && failure.error.is_none();
        let reconcile = (status_is_drift && (!is_load_session || has_active_session))
            || (malformed_success && !is_load_session);
        let deterministic_client_error = failure
            .status
            .is_some_and(|status| (400..500).contains(&status) && !matches!(status, 408 | 429));

        if reconcile {
            self.activity = ExpectedReceivingActivity::ReconcileRequired;
        } else if deterministic_client_error {
            self.activity = ExpectedReceivingActivity::Ready;
            self.pending = None;
        } else {
            self.activity = ExpectedReceivingActivity::Retryable;
        }
        self.request_error = Some(failure.message);
        ExpectedReceivingApplyResult::Applied
    }

    fn submit_item_scan(&mut self) {
        let scanned = self.scan_draft.trim().to_owned();
        if scanned.is_empty() {
            self.reject_scan("Item barcode cannot be empty");
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };

        if let Some(selected_line_id) = self.selected_line_id {
            let matches_selected = session.lines.iter().any(|line| {
                line.load_line_id == selected_line_id
                    && line
                        .item_barcodes
                        .iter()
                        .any(|barcode| barcode.eq_ignore_ascii_case(&scanned))
            });
            if !matches_selected {
                self.reject_scan("Item barcode does not match the selected receipt line");
                return;
            }
        } else {
            let matching_lines = session
                .lines
                .iter()
                .filter(|line| {
                    line.item_barcodes
                        .iter()
                        .any(|barcode| barcode.eq_ignore_ascii_case(&scanned))
                })
                .map(|line| line.load_line_id)
                .collect::<Vec<_>>();
            match matching_lines.as_slice() {
                [] => {
                    self.reject_scan("Item barcode is not expected on this load");
                    return;
                }
                [load_line_id] => self.selected_line_id = Some(*load_line_id),
                _ => {
                    self.reject_scan(
                        "Item barcode matches multiple receipt lines; select a line explicitly",
                    );
                    return;
                }
            }
        }

        self.verified_item_barcode = Some(scanned);
        self.accept_scan();
        if let Some(line) = self.active_line().cloned() {
            self.lot_draft = line.lot.unwrap_or_default();
            self.serial_draft = line.serial.unwrap_or_default();
            self.expiration_draft = line.expiration.unwrap_or_default();
        }
    }

    fn submit_receiving_location_scan(&mut self) {
        let scanned = self.scan_draft.trim().to_owned();
        let Some(expected) = self
            .session
            .as_ref()
            .map(|session| session.receiving_location.barcode.clone())
        else {
            return;
        };
        if scanned != expected {
            self.reject_scan("Receiving location does not match the assigned dock");
            return;
        }
        self.receiving_location_verified = true;
        self.accept_scan();
    }

    fn parse_quantity(&mut self, remaining_quantity: i64) -> Option<i64> {
        match self.quantity_draft.trim().parse::<i64>() {
            Ok(quantity) if quantity > 0 && quantity <= remaining_quantity => Some(quantity),
            Ok(quantity) if quantity > remaining_quantity => {
                self.request_error = Some(format!(
                    "Quantity cannot exceed {remaining_quantity} remaining"
                ));
                None
            }
            _ => {
                self.request_error = Some("Quantity must be a positive whole number".into());
                None
            }
        }
    }

    fn build_confirmation(&mut self, quantity: i64) -> Option<ConfirmExpectedReceiptRequest> {
        let item_barcode = self.verified_item_barcode.clone();
        match self.disposition {
            ExpectedReceiptDisposition::Received => {
                let item_barcode = item_barcode.or_else(|| {
                    self.request_error = Some("Scan the expected item before receiving".into());
                    None
                })?;
                if !self.receiving_location_verified {
                    self.request_error = Some("Scan the assigned receiving location".into());
                    return None;
                }
                let receiving_location_barcode =
                    self.session.as_ref()?.receiving_location.barcode.clone();
                let expiration = optional_text(&self.expiration_draft);
                if expiration
                    .as_deref()
                    .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_err())
                {
                    self.request_error = Some("Expiration must be an RFC 3339 timestamp".into());
                    return None;
                }
                Some(ConfirmExpectedReceiptRequest::Received {
                    item_barcode,
                    receiving_location_barcode,
                    quantity,
                    license_plate_barcode: optional_text(&self.license_plate_draft),
                    lot: optional_text(&self.lot_draft),
                    serial: optional_text(&self.serial_draft),
                    expiration,
                })
            }
            ExpectedReceiptDisposition::Rejected => {
                let item_barcode = item_barcode.or_else(|| {
                    self.request_error = Some("Scan the rejected item before confirming".into());
                    None
                })?;
                let (reason, note) = self.validated_exception()?;
                Some(ConfirmExpectedReceiptRequest::Rejected {
                    item_barcode,
                    quantity,
                    reason,
                    note,
                })
            }
            ExpectedReceiptDisposition::Missing => {
                let (reason, note) = self.validated_exception()?;
                Some(ConfirmExpectedReceiptRequest::Missing {
                    quantity,
                    reason,
                    note,
                })
            }
        }
    }

    fn validated_exception(&mut self) -> Option<(ExpectedReceiptExceptionReason, Option<String>)> {
        let reason = self.reason;
        let note = optional_text(&self.note_draft);
        if reason == ExpectedReceiptExceptionReason::Other && note.is_none() {
            self.request_error = Some("Enter a note when the reason is Other".into());
            return None;
        }
        Some((reason, note))
    }

    fn apply_success(
        &mut self,
        command: ExpectedReceivingCommand,
        outcome: ExpectedReceivingTransportOutcome,
    ) -> ExpectedReceivingApplyResult {
        match (command, outcome) {
            (
                ExpectedReceivingCommand::LoadSession { load_id },
                ExpectedReceivingTransportOutcome::Session(session),
            ) if load_id == session.load_id => {
                self.pending = None;
                self.activity = ExpectedReceivingActivity::Ready;
                self.request_error = None;
                self.load_id_draft = load_id.to_string();
                self.session = Some(session);
                self.reset_active_work();
                ExpectedReceivingApplyResult::Applied
            }
            (
                ExpectedReceivingCommand::Confirm {
                    load_line_id, body, ..
                },
                ExpectedReceivingTransportOutcome::Confirmation(confirmation),
            ) if confirmation_matches(
                self.session.as_ref().map(|session| session.load_id),
                load_line_id,
                &body,
                &confirmation,
            ) =>
            {
                self.apply_confirmation(confirmation)
            }
            _ => {
                self.activity = ExpectedReceivingActivity::ReconcileRequired;
                self.request_error = Some(
                    "The server returned a response for different expected receiving work".into(),
                );
                ExpectedReceivingApplyResult::Applied
            }
        }
    }

    fn apply_confirmation(
        &mut self,
        confirmation: ExpectedReceiptConfirmationResponse,
    ) -> ExpectedReceivingApplyResult {
        let load_id = confirmation.load_id;
        if let Some(line) = self.session.as_mut().and_then(|session| {
            session
                .lines
                .iter_mut()
                .find(|line| line.load_line_id == confirmation.load_line_id)
        }) {
            line.received_quantity = confirmation.cumulative_received_quantity;
            line.rejected_quantity = confirmation.cumulative_rejected_quantity;
            line.missing_quantity = confirmation.cumulative_missing_quantity;
            line.remaining_quantity = confirmation.remaining_quantity;
        }
        self.pending = None;
        self.activity = ExpectedReceivingActivity::Ready;
        self.request_error = None;
        self.reset_active_work();
        let message = format!(
            "{} {} on load #{}",
            confirmation.quantity,
            disposition_label(confirmation.disposition),
            confirmation.load_id
        );
        self.completion = Some(message.clone());

        if confirmation.receive_completed {
            self.session = None;
            self.load_id_draft.clear();
            ExpectedReceivingApplyResult::Completed(message)
        } else {
            ExpectedReceivingApplyResult::ReloadRequired(load_id)
        }
    }

    fn begin(&mut self, request: ExpectedReceivingRequest) -> ExpectedReceivingRequest {
        self.activity = ExpectedReceivingActivity::Pending;
        self.pending = Some(request.clone());
        self.pending_started_at = self.clock_now;
        self.request_error = None;
        request
    }

    fn reset_active_work(&mut self) {
        self.selected_line_id = None;
        self.disposition = ExpectedReceiptDisposition::Received;
        self.clear_scan_verification();
        self.quantity_draft = "1".into();
        self.license_plate_draft.clear();
        self.lot_draft.clear();
        self.serial_draft.clear();
        self.expiration_draft.clear();
        self.reason = ExpectedReceiptExceptionReason::Damaged;
        self.note_draft.clear();
    }

    fn clear_scan_verification(&mut self) {
        self.verified_item_barcode = None;
        self.receiving_location_verified = false;
        self.scan_draft.clear();
        self.scan_error = None;
    }

    fn accept_scan(&mut self) {
        self.scan_draft.clear();
        self.scan_error = None;
        self.request_error = None;
    }

    fn reject_scan(&mut self, message: &str) {
        self.scan_draft.clear();
        self.scan_error = Some(message.into());
    }
}

fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn pending_load_id(pending: Option<&ExpectedReceivingRequest>) -> Option<i64> {
    match &pending?.command {
        ExpectedReceivingCommand::LoadSession { load_id } => Some(*load_id),
        ExpectedReceivingCommand::Confirm { .. } => None,
    }
}

fn confirmation_matches(
    expected_load_id: Option<i64>,
    load_line_id: i64,
    body: &ConfirmExpectedReceiptRequest,
    confirmation: &ExpectedReceiptConfirmationResponse,
) -> bool {
    let (disposition, quantity) = match body {
        ConfirmExpectedReceiptRequest::Received { quantity, .. } => {
            (ExpectedReceiptDisposition::Received, *quantity)
        }
        ConfirmExpectedReceiptRequest::Rejected { quantity, .. } => {
            (ExpectedReceiptDisposition::Rejected, *quantity)
        }
        ConfirmExpectedReceiptRequest::Missing { quantity, .. } => {
            (ExpectedReceiptDisposition::Missing, *quantity)
        }
    };
    Some(confirmation.load_id) == expected_load_id
        && confirmation.load_line_id == load_line_id
        && confirmation.disposition == disposition
        && confirmation.quantity == quantity
}

fn disposition_label(disposition: ExpectedReceiptDisposition) -> &'static str {
    match disposition {
        ExpectedReceiptDisposition::Received => "received",
        ExpectedReceiptDisposition::Rejected => "rejected",
        ExpectedReceiptDisposition::Missing => "marked missing",
    }
}

#[cfg(test)]
mod tests;
