use chrono::DateTime;
use wareboxes_api_contract::v1::{
    ErrorReason, PutawayClaimHeartbeatResponse, PutawayClaimReleaseReason, PutawayClaimResponse,
    PutawayClaimWork, PutawayWorkflow,
};

use crate::api::{PutawayCommand, PutawayRequest, PutawayTransportEvent, PutawayTransportOutcome};

const HEARTBEAT_ATTEMPT_TIMEOUT_SECONDS: f64 = 10.0;
const HEARTBEAT_LEASE_SAFETY_SECONDS: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutawayActivity {
    Uninitialized,
    Ready,
    Pending,
    Retryable,
    ReconcileRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PutawayScanStage {
    SourceLocation,
    LicensePlate,
    DestinationLocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PutawayHeartbeatActivity {
    Idle,
    Pending,
    Retryable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutawayApplyResult {
    Applied,
    Ignored,
    Completed(String),
}

#[derive(Debug, Clone)]
pub struct PutawayWorkflowState {
    activity: PutawayActivity,
    selected_workflow: PutawayWorkflow,
    claim: Option<PutawayClaimResponse>,
    pending: Option<PutawayRequest>,
    source_verified: bool,
    license_plate_scan: Option<String>,
    scan_draft: String,
    task_id_draft: String,
    scan_error: Option<String>,
    request_error: Option<String>,
    no_work: bool,
    completion: Option<String>,
    heartbeat_activity: PutawayHeartbeatActivity,
    heartbeat_pending: Option<PutawayRequest>,
    heartbeat_started_at: Option<f64>,
    heartbeat_attempt_started_at: Option<f64>,
    heartbeat_retry_at: Option<f64>,
    next_heartbeat_at: Option<f64>,
    lease_deadline: Option<f64>,
    heartbeat_failures: u32,
    heartbeat_error: Option<String>,
}

impl Default for PutawayWorkflowState {
    fn default() -> Self {
        Self {
            activity: PutawayActivity::Uninitialized,
            selected_workflow: PutawayWorkflow::Loose,
            claim: None,
            pending: None,
            source_verified: false,
            license_plate_scan: None,
            scan_draft: String::new(),
            task_id_draft: String::new(),
            scan_error: None,
            request_error: None,
            no_work: false,
            completion: None,
            heartbeat_activity: PutawayHeartbeatActivity::Idle,
            heartbeat_pending: None,
            heartbeat_started_at: None,
            heartbeat_attempt_started_at: None,
            heartbeat_retry_at: None,
            next_heartbeat_at: None,
            lease_deadline: None,
            heartbeat_failures: 0,
            heartbeat_error: None,
        }
    }
}

impl PutawayWorkflowState {
    pub fn activity(&self) -> PutawayActivity {
        self.activity
    }

    pub fn selected_workflow(&self) -> PutawayWorkflow {
        self.selected_workflow
    }

    pub fn select_workflow(&mut self, workflow: PutawayWorkflow) {
        if self.activity == PutawayActivity::Ready && self.claim.is_none() {
            self.selected_workflow = workflow;
            self.no_work = false;
            self.completion = None;
        }
    }

    pub fn claim(&self) -> Option<&PutawayClaimResponse> {
        self.claim.as_ref()
    }

    pub fn scan_draft_mut(&mut self) -> &mut String {
        &mut self.scan_draft
    }

    pub fn task_id_draft_mut(&mut self) -> &mut String {
        &mut self.task_id_draft
    }

    pub fn scan_error(&self) -> Option<&str> {
        self.scan_error.as_deref()
    }

    pub fn request_error(&self) -> Option<&str> {
        self.request_error.as_deref()
    }

    pub fn no_work(&self) -> bool {
        self.no_work
    }

    pub fn completion(&self) -> Option<&str> {
        self.completion.as_deref()
    }

    pub fn heartbeat_error(&self) -> Option<&str> {
        self.heartbeat_error.as_deref()
    }

    pub fn begin_current(&mut self, request_id: String) -> Option<PutawayRequest> {
        if self.activity == PutawayActivity::Pending {
            return None;
        }
        self.reset_heartbeat();
        Some(self.begin(PutawayRequest {
            request_id,
            command: PutawayCommand::LoadCurrent,
        }))
    }

    pub fn begin_release(
        &mut self,
        request_id: String,
        idempotency_key: String,
        reason: PutawayClaimReleaseReason,
        note: Option<String>,
    ) -> Option<PutawayRequest> {
        if self.activity != PutawayActivity::Ready {
            return None;
        }
        let task_id = self.claim.as_ref()?.task_id;
        Some(self.begin(PutawayRequest {
            request_id,
            command: PutawayCommand::Release {
                task_id,
                reason,
                note,
                idempotency_key,
            },
        }))
    }

    pub fn poll_heartbeat(
        &mut self,
        now: f64,
        request_id: String,
        new_idempotency_key: String,
    ) -> Option<PutawayRequest> {
        if !self.foreground_allows_heartbeat() {
            return None;
        }
        if self
            .lease_deadline
            .is_some_and(|deadline| now >= deadline - HEARTBEAT_LEASE_SAFETY_SECONDS)
        {
            self.require_reconciliation("The putaway claim lease must be reconciled".into());
            return None;
        }
        self.expire_heartbeat_attempt(now);
        if self.activity == PutawayActivity::ReconcileRequired {
            return None;
        }
        let task_id = self.claim.as_ref()?.task_id;
        match self.heartbeat_activity {
            PutawayHeartbeatActivity::Idle
                if self.next_heartbeat_at.is_some_and(|due| now >= due) =>
            {
                let request = PutawayRequest {
                    request_id,
                    command: PutawayCommand::Heartbeat {
                        task_id,
                        idempotency_key: new_idempotency_key,
                    },
                };
                self.heartbeat_activity = PutawayHeartbeatActivity::Pending;
                self.heartbeat_pending = Some(request.clone());
                self.heartbeat_started_at = Some(now);
                self.heartbeat_attempt_started_at = Some(now);
                self.next_heartbeat_at = None;
                Some(request)
            }
            PutawayHeartbeatActivity::Retryable
                if self.heartbeat_retry_at.is_some_and(|due| now >= due) =>
            {
                let mut request = self.heartbeat_pending.clone()?;
                request.request_id = request_id;
                self.heartbeat_activity = PutawayHeartbeatActivity::Pending;
                self.heartbeat_retry_at = None;
                self.heartbeat_attempt_started_at = Some(now);
                self.heartbeat_pending = Some(request.clone());
                Some(request)
            }
            _ => None,
        }
    }

    pub fn begin_claim(
        &mut self,
        request_id: String,
        idempotency_key: String,
    ) -> Option<PutawayRequest> {
        if self.activity != PutawayActivity::Ready || self.claim.is_some() {
            return None;
        }
        self.no_work = false;
        self.completion = None;
        Some(self.begin(PutawayRequest {
            request_id,
            command: PutawayCommand::ClaimNext {
                workflow: self.selected_workflow,
                idempotency_key,
            },
        }))
    }

    pub fn begin_claim_by_id(
        &mut self,
        request_id: String,
        idempotency_key: String,
    ) -> Option<PutawayRequest> {
        if self.activity != PutawayActivity::Ready || self.claim.is_some() {
            return None;
        }
        let task_id = match self.task_id_draft.trim().parse::<i64>() {
            Ok(task_id) if task_id > 0 => task_id,
            _ => {
                self.request_error = Some("Task ID must be a positive whole number".into());
                return None;
            }
        };
        self.no_work = false;
        self.completion = None;
        Some(self.begin(PutawayRequest {
            request_id,
            command: PutawayCommand::ClaimById {
                task_id,
                idempotency_key,
            },
        }))
    }

    pub fn expected_scan(&self) -> Option<PutawayScanStage> {
        if self.activity != PutawayActivity::Ready {
            return None;
        }
        let claim = self.claim.as_ref()?;
        if claim.source_location.barcode.is_some() && !self.source_verified {
            return Some(PutawayScanStage::SourceLocation);
        }
        if matches!(claim.work, PutawayClaimWork::LicensePlate { .. })
            && self.license_plate_scan.is_none()
        {
            return Some(PutawayScanStage::LicensePlate);
        }
        Some(PutawayScanStage::DestinationLocation)
    }

    pub fn submit_scan(
        &mut self,
        request_id: String,
        idempotency_key: String,
    ) -> Option<PutawayRequest> {
        let stage = self.expected_scan()?;
        let scanned = self.scan_draft.trim().to_owned();
        if scanned.is_empty() {
            self.scan_error = Some("Scan cannot be empty".into());
            return None;
        }
        let claim = self.claim.as_ref()?;

        match stage {
            PutawayScanStage::SourceLocation => {
                let expected = claim.source_location.barcode.as_deref().unwrap_or_default();
                if scanned != expected {
                    self.reject_scan("Source location does not match the assigned work");
                    return None;
                }
                self.source_verified = true;
                self.accept_scan();
                None
            }
            PutawayScanStage::LicensePlate => {
                let PutawayClaimWork::LicensePlate {
                    license_plate_barcode,
                    ..
                } = &claim.work
                else {
                    self.reject_scan("The active work does not require a license plate");
                    return None;
                };
                if scanned != *license_plate_barcode {
                    self.reject_scan("License plate does not match the assigned work");
                    return None;
                }
                self.license_plate_scan = Some(scanned);
                self.accept_scan();
                None
            }
            PutawayScanStage::DestinationLocation => {
                if scanned != claim.destination_location.barcode {
                    self.reject_scan("Destination location does not match the assignment");
                    return None;
                }
                let command = match &claim.work {
                    PutawayClaimWork::Loose { .. } => PutawayCommand::ConfirmLoose {
                        task_id: claim.task_id,
                        destination_location_barcode: scanned,
                        idempotency_key,
                    },
                    PutawayClaimWork::LicensePlate { .. } => {
                        let license_plate_barcode = self.license_plate_scan.clone()?;
                        PutawayCommand::ConfirmLicensePlate {
                            task_id: claim.task_id,
                            license_plate_barcode,
                            destination_location_barcode: scanned,
                            idempotency_key,
                        }
                    }
                };
                self.accept_scan();
                Some(self.begin(PutawayRequest {
                    request_id,
                    command,
                }))
            }
        }
    }

    pub fn retry(&mut self, request_id: String) -> Option<PutawayRequest> {
        if self.activity != PutawayActivity::Retryable {
            return None;
        }
        let mut request = self.pending.clone()?;
        request.request_id = request_id;
        Some(self.begin(request))
    }

    #[cfg(test)]
    pub fn apply(&mut self, event: PutawayTransportEvent) -> PutawayApplyResult {
        self.apply_at(event, 0.0)
    }

    pub fn apply_at(&mut self, event: PutawayTransportEvent, now: f64) -> PutawayApplyResult {
        if matches!(&event.request.command, PutawayCommand::Heartbeat { .. }) {
            return self.apply_heartbeat(event, now);
        }
        let Some(pending) = self.pending.as_ref() else {
            return PutawayApplyResult::Ignored;
        };
        if pending.request_id != event.request.request_id
            || pending.command != event.request.command
        {
            return PutawayApplyResult::Ignored;
        }

        match event.outcome {
            Ok(outcome) => self.apply_success(event.request.command, outcome, now),
            Err(failure) => {
                let reconcile = failure.error.as_ref().is_some_and(|error| {
                    matches!(error.reason, ErrorReason::Conflict | ErrorReason::NotFound)
                });
                self.activity = if reconcile {
                    PutawayActivity::ReconcileRequired
                } else {
                    PutawayActivity::Retryable
                };
                if reconcile {
                    self.reset_heartbeat();
                }
                self.request_error = Some(failure.message);
                PutawayApplyResult::Applied
            }
        }
    }

    fn apply_success(
        &mut self,
        command: PutawayCommand,
        outcome: PutawayTransportOutcome,
        now: f64,
    ) -> PutawayApplyResult {
        match (command, outcome) {
            (PutawayCommand::LoadCurrent, PutawayTransportOutcome::Current(claim)) => {
                self.pending = None;
                self.request_error = None;
                self.no_work = false;
                self.activate(claim, now);
                PutawayApplyResult::Applied
            }
            (
                PutawayCommand::ClaimNext { .. } | PutawayCommand::ClaimById { .. },
                PutawayTransportOutcome::Claimed(claim),
            ) => {
                self.pending = None;
                self.request_error = None;
                self.no_work = claim.is_none();
                self.activate(claim, now);
                PutawayApplyResult::Applied
            }
            (
                PutawayCommand::Release { task_id, .. },
                PutawayTransportOutcome::Released(release),
            ) if task_id == release.task_id => {
                let message = format!("Putaway #{} released to the queue", release.task_id);
                self.complete(message)
            }
            (
                PutawayCommand::ConfirmLoose { task_id, .. },
                PutawayTransportOutcome::LooseConfirmed(confirmation),
            ) if task_id == confirmation.task_id => {
                let message = format!(
                    "Putaway #{} completed: {} {} moved",
                    confirmation.task_id, confirmation.quantity, confirmation.inventory_status
                );
                self.complete(message)
            }
            (
                PutawayCommand::ConfirmLicensePlate { task_id, .. },
                PutawayTransportOutcome::LicensePlateConfirmed(confirmation),
            ) if task_id == confirmation.task_id => {
                let message = format!(
                    "Putaway #{} completed: {} balances moved",
                    confirmation.task_id, confirmation.moved_balance_count
                );
                self.complete(message)
            }
            _ => {
                self.activity = PutawayActivity::Retryable;
                self.request_error =
                    Some("The server returned a response for a different putaway command".into());
                PutawayApplyResult::Applied
            }
        }
    }

    fn begin(&mut self, request: PutawayRequest) -> PutawayRequest {
        self.activity = PutawayActivity::Pending;
        self.request_error = None;
        self.pending = Some(request.clone());
        request
    }

    fn activate(&mut self, claim: Option<PutawayClaimResponse>, now: f64) {
        self.activity = PutawayActivity::Ready;
        self.claim = claim;
        self.source_verified = false;
        self.license_plate_scan = None;
        self.scan_draft.clear();
        self.task_id_draft.clear();
        self.scan_error = None;
        self.reset_heartbeat();
        if self.claim.is_some() {
            self.next_heartbeat_at = Some(now);
        }
    }

    fn complete(&mut self, message: String) -> PutawayApplyResult {
        self.pending = None;
        self.activity = PutawayActivity::Ready;
        self.claim = None;
        self.source_verified = false;
        self.license_plate_scan = None;
        self.scan_draft.clear();
        self.scan_error = None;
        self.request_error = None;
        self.no_work = false;
        self.completion = Some(message.clone());
        self.reset_heartbeat();
        PutawayApplyResult::Completed(message)
    }

    fn accept_scan(&mut self) {
        self.scan_draft.clear();
        self.scan_error = None;
    }

    fn reject_scan(&mut self, message: &str) {
        self.scan_draft.clear();
        self.scan_error = Some(message.into());
    }

    fn apply_heartbeat(&mut self, event: PutawayTransportEvent, now: f64) -> PutawayApplyResult {
        let Some(pending) = self.heartbeat_pending.as_ref() else {
            return PutawayApplyResult::Ignored;
        };
        if pending.request_id != event.request.request_id
            || pending.command != event.request.command
        {
            return PutawayApplyResult::Ignored;
        }

        match event.outcome {
            Ok(PutawayTransportOutcome::Heartbeat(heartbeat))
                if matches!(
                    &event.request.command,
                    PutawayCommand::Heartbeat { task_id, .. } if *task_id == heartbeat.task_id
                ) =>
            {
                self.accept_heartbeat(heartbeat, now)
            }
            Ok(_) => {
                self.require_reconciliation(
                    "The server returned a response for a different heartbeat command".into(),
                );
                PutawayApplyResult::Applied
            }
            Err(failure) => {
                let conflict = failure.error.as_ref().is_some_and(|error| {
                    matches!(error.reason, ErrorReason::Conflict | ErrorReason::NotFound)
                });
                let transient = failure.status.is_none()
                    || failure.status == Some(429)
                    || failure.status.is_some_and(|status| status >= 500);
                if conflict || !transient {
                    self.require_reconciliation(failure.message);
                } else {
                    self.schedule_heartbeat_retry(now, failure.message, true);
                }
                PutawayApplyResult::Applied
            }
        }
    }

    fn accept_heartbeat(
        &mut self,
        heartbeat: PutawayClaimHeartbeatResponse,
        now: f64,
    ) -> PutawayApplyResult {
        let Some(ttl) = heartbeat_ttl_seconds(&heartbeat) else {
            self.require_reconciliation("The server returned invalid putaway lease timing".into());
            return PutawayApplyResult::Applied;
        };
        let started_at = self.heartbeat_started_at.unwrap_or(now);
        let elapsed = (now - started_at).max(0.0);
        let remaining = ttl - elapsed;
        self.lease_deadline = Some(started_at + ttl);
        self.heartbeat_activity = PutawayHeartbeatActivity::Idle;
        self.heartbeat_pending = None;
        self.heartbeat_started_at = None;
        self.heartbeat_attempt_started_at = None;
        self.heartbeat_retry_at = None;
        self.heartbeat_failures = 0;
        self.heartbeat_error = None;
        let normal_interval = (ttl / 3.0).clamp(5.0, 300.0);
        self.next_heartbeat_at = Some(if remaining <= 10.0 {
            now
        } else {
            now + normal_interval.min(remaining / 2.0)
        });
        PutawayApplyResult::Applied
    }

    fn require_reconciliation(&mut self, message: String) {
        self.activity = PutawayActivity::ReconcileRequired;
        self.request_error = Some(message);
        self.reset_heartbeat();
    }

    fn foreground_allows_heartbeat(&self) -> bool {
        match self.activity {
            PutawayActivity::Ready => true,
            PutawayActivity::Pending | PutawayActivity::Retryable => {
                self.pending.as_ref().is_some_and(|request| {
                    matches!(
                        &request.command,
                        PutawayCommand::Release { .. }
                            | PutawayCommand::ConfirmLoose { .. }
                            | PutawayCommand::ConfirmLicensePlate { .. }
                    )
                })
            }
            PutawayActivity::Uninitialized | PutawayActivity::ReconcileRequired => false,
        }
    }

    fn expire_heartbeat_attempt(&mut self, now: f64) {
        if self.heartbeat_activity != PutawayHeartbeatActivity::Pending
            || !self
                .heartbeat_attempt_started_at
                .is_some_and(|started| now - started >= HEARTBEAT_ATTEMPT_TIMEOUT_SECONDS)
        {
            return;
        }
        self.schedule_heartbeat_retry(
            now,
            "Putaway claim renewal timed out; retrying the same command".into(),
            false,
        );
    }

    fn schedule_heartbeat_retry(&mut self, now: f64, message: String, with_backoff: bool) {
        self.heartbeat_failures = self.heartbeat_failures.saturating_add(1);
        let delay = if with_backoff {
            2_f64.powi(self.heartbeat_failures.min(4) as i32)
        } else {
            0.0
        };
        let lease_exhausted = self
            .lease_deadline
            .is_some_and(|deadline| now + delay >= deadline - HEARTBEAT_LEASE_SAFETY_SECONDS);
        if lease_exhausted || (self.lease_deadline.is_none() && self.heartbeat_failures >= 5) {
            self.require_reconciliation(message);
            return;
        }
        self.heartbeat_activity = PutawayHeartbeatActivity::Retryable;
        self.heartbeat_attempt_started_at = None;
        self.heartbeat_retry_at = Some(now + delay);
        self.heartbeat_error = Some(message);
    }

    fn reset_heartbeat(&mut self) {
        self.heartbeat_activity = PutawayHeartbeatActivity::Idle;
        self.heartbeat_pending = None;
        self.heartbeat_started_at = None;
        self.heartbeat_attempt_started_at = None;
        self.heartbeat_retry_at = None;
        self.next_heartbeat_at = None;
        self.lease_deadline = None;
        self.heartbeat_failures = 0;
        self.heartbeat_error = None;
    }
}

fn heartbeat_ttl_seconds(heartbeat: &PutawayClaimHeartbeatResponse) -> Option<f64> {
    let heartbeat_at = DateTime::parse_from_rfc3339(&heartbeat.heartbeat_at).ok()?;
    let lease_expires_at = DateTime::parse_from_rfc3339(&heartbeat.lease_expires_at).ok()?;
    let milliseconds = lease_expires_at
        .signed_duration_since(heartbeat_at)
        .num_milliseconds();
    (milliseconds > 0).then_some(milliseconds as f64 / 1_000.0)
}

#[cfg(test)]
mod tests {
    use wareboxes_api_contract::v1::{
        InventoryBalanceStatus, PutawayClaimDestinationLocation, PutawayClaimHeartbeatResponse,
        PutawayClaimReleaseReason, PutawayClaimReleaseResponse, PutawayClaimSourceLocation,
        PutawayConfirmationResponse,
    };

    use super::*;
    use crate::api::putaway::PutawayTransportFailure;

    fn loose_claim() -> PutawayClaimResponse {
        PutawayClaimResponse {
            task_id: 11,
            inventory_owner_id: 22,
            facility_id: 33,
            priority: 50,
            instructions: None,
            due_at: None,
            lease_expires_at: "2026-07-27T01:00:00Z".into(),
            source_location: PutawayClaimSourceLocation {
                location_id: 44,
                barcode: Some("RECV-01".into()),
                name: Some("Receiving".into()),
            },
            destination_location: PutawayClaimDestinationLocation {
                location_id: 55,
                barcode: "A-01-01".into(),
                name: Some("A-01-01".into()),
            },
            work: PutawayClaimWork::Loose {
                source_inventory_balance_id: 66,
                item_batch_id: 77,
                item_id: 88,
                item_description: Some("Widget".into()),
                uom: "each".into(),
                lot: Some("LOT-1".into()),
                serial: None,
                expiration: None,
                inventory_status: InventoryBalanceStatus::Available,
                quantity: 4,
            },
        }
    }

    fn plate_claim() -> PutawayClaimResponse {
        let mut claim = loose_claim();
        claim.task_id = 12;
        claim.work = PutawayClaimWork::LicensePlate {
            license_plate_id: 91,
            license_plate_barcode: "LP-91".into(),
            planned_balance_count: 3,
        };
        claim
    }

    fn success(request: PutawayRequest, outcome: PutawayTransportOutcome) -> PutawayTransportEvent {
        PutawayTransportEvent {
            request,
            outcome: Ok(outcome),
        }
    }

    #[test]
    fn current_claim_resumes_source_then_destination_scan() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current-1".into()).unwrap();
        assert_eq!(
            state.apply(success(
                current,
                PutawayTransportOutcome::Current(Some(loose_claim()))
            )),
            PutawayApplyResult::Applied
        );
        assert_eq!(
            state.expected_scan(),
            Some(PutawayScanStage::SourceLocation)
        );

        *state.scan_draft_mut() = "RECV-01".into();
        assert!(state
            .submit_scan("unused".into(), "unused".into())
            .is_none());
        assert_eq!(
            state.expected_scan(),
            Some(PutawayScanStage::DestinationLocation)
        );

        *state.scan_draft_mut() = "A-01-01".into();
        let confirmation = state
            .submit_scan("confirm-1".into(), "confirm-key-1".into())
            .unwrap();
        assert!(matches!(
            confirmation.command,
            PutawayCommand::ConfirmLoose {
                task_id: 11,
                destination_location_barcode,
                idempotency_key
            } if destination_location_barcode == "A-01-01"
                && idempotency_key == "confirm-key-1"
        ));
    }

    #[test]
    fn license_plate_scan_is_required_before_destination() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current-1".into()).unwrap();
        state.apply(success(
            current,
            PutawayTransportOutcome::Current(Some(plate_claim())),
        ));
        *state.scan_draft_mut() = "RECV-01".into();
        state.submit_scan("unused".into(), "unused".into());
        assert_eq!(state.expected_scan(), Some(PutawayScanStage::LicensePlate));

        *state.scan_draft_mut() = "LP-WRONG".into();
        state.submit_scan("unused".into(), "unused".into());
        assert_eq!(state.expected_scan(), Some(PutawayScanStage::LicensePlate));
        assert!(state.scan_error().is_some());

        *state.scan_draft_mut() = "LP-91".into();
        state.submit_scan("unused".into(), "unused".into());
        *state.scan_draft_mut() = "A-01-01".into();
        let confirmation = state
            .submit_scan("confirm-2".into(), "confirm-key-2".into())
            .unwrap();
        assert!(matches!(
            confirmation.command,
            PutawayCommand::ConfirmLicensePlate {
                license_plate_barcode,
                destination_location_barcode,
                ..
            } if license_plate_barcode == "LP-91"
                && destination_location_barcode == "A-01-01"
        ));
    }

    #[test]
    fn ambiguous_failure_retries_exact_command_with_new_request_identity() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current-1".into()).unwrap();
        state.apply(success(current, PutawayTransportOutcome::Current(None)));
        let claim = state
            .begin_claim("attempt-1".into(), "stable-key".into())
            .unwrap();
        let failure = PutawayTransportEvent {
            request: claim.clone(),
            outcome: Err(PutawayTransportFailure {
                status: None,
                error: None,
                message: "connection closed".into(),
            }),
        };

        state.apply(failure);
        let retry = state.retry("attempt-2".into()).unwrap();

        assert_eq!(retry.request_id, "attempt-2");
        assert_eq!(retry.command, claim.command);
        assert_eq!(retry.command.idempotency_key(), Some("stable-key"));
    }

    #[test]
    fn selected_task_claim_validates_and_preserves_the_scanned_id() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current-1".into()).unwrap();
        state.apply(success(current, PutawayTransportOutcome::Current(None)));
        *state.task_id_draft_mut() = "not-a-task".into();
        assert!(state
            .begin_claim_by_id("attempt-1".into(), "selected-key".into())
            .is_none());

        *state.task_id_draft_mut() = " 81 ".into();
        let request = state
            .begin_claim_by_id("attempt-2".into(), "selected-key".into())
            .unwrap();

        assert!(matches!(
            request.command,
            PutawayCommand::ClaimById {
                task_id: 81,
                idempotency_key
            } if idempotency_key == "selected-key"
        ));
    }

    #[test]
    fn conflict_requires_current_claim_reconciliation() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current-1".into()).unwrap();
        state.apply(success(current, PutawayTransportOutcome::Current(None)));
        let claim = state
            .begin_claim("claim-1".into(), "claim-key".into())
            .unwrap();
        state.apply(PutawayTransportEvent {
            request: claim,
            outcome: Err(PutawayTransportFailure {
                status: Some(409),
                error: Some(wareboxes_api_contract::v1::ErrorResponse::new(
                    ErrorReason::Conflict,
                    "active task exists",
                    "request-1",
                )),
                message: "active task exists".into(),
            }),
        });

        assert_eq!(state.activity(), PutawayActivity::ReconcileRequired);
        assert!(state.retry("retry".into()).is_none());
        assert!(state.begin_current("current-2".into()).is_some());
    }

    #[test]
    fn late_response_is_ignored() {
        let mut state = PutawayWorkflowState::default();
        let first = state.begin_current("current-1".into()).unwrap();
        state.apply(PutawayTransportEvent {
            request: first,
            outcome: Err(PutawayTransportFailure {
                status: None,
                error: None,
                message: "timeout".into(),
            }),
        });
        let retry = state.retry("current-2".into()).unwrap();
        let mut late = retry.clone();
        late.request_id = "current-1".into();

        assert_eq!(
            state.apply(success(late, PutawayTransportOutcome::Current(None))),
            PutawayApplyResult::Ignored
        );
        assert_eq!(state.activity(), PutawayActivity::Pending);
    }

    #[test]
    fn confirmation_success_clears_active_work() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current".into()).unwrap();
        state.apply(success(
            current,
            PutawayTransportOutcome::Current(Some(loose_claim())),
        ));
        *state.scan_draft_mut() = "RECV-01".into();
        state.submit_scan("unused".into(), "unused".into());
        *state.scan_draft_mut() = "A-01-01".into();
        let request = state
            .submit_scan("confirm".into(), "confirm-key".into())
            .unwrap();
        let result = state.apply(success(
            request,
            PutawayTransportOutcome::LooseConfirmed(PutawayConfirmationResponse {
                task_id: 11,
                inventory_owner_id: 22,
                facility_id: 33,
                inventory_transaction_id: 100,
                source_inventory_balance_id: 66,
                destination_inventory_balance_id: 67,
                source_location_id: 44,
                destination_location_id: 55,
                destination_location_barcode: "A-01-01".into(),
                item_batch_id: 77,
                item_id: 88,
                quantity: 4,
                inventory_status: "available".into(),
                confirmed_by: 99,
                confirmed_at: "2026-07-27T00:00:00Z".into(),
            }),
        ));

        assert!(matches!(result, PutawayApplyResult::Completed(_)));
        assert!(state.claim().is_none());
        assert_eq!(state.activity(), PutawayActivity::Ready);
    }

    #[test]
    fn background_heartbeat_keeps_scanning_ready_and_uses_server_ttl() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current".into()).unwrap();
        state.apply_at(
            success(
                current,
                PutawayTransportOutcome::Current(Some(loose_claim())),
            ),
            100.0,
        );
        let heartbeat = state
            .poll_heartbeat(100.0, "heartbeat-1".into(), "heartbeat-key-1".into())
            .unwrap();

        assert_eq!(
            state.expected_scan(),
            Some(PutawayScanStage::SourceLocation)
        );
        assert!(matches!(
            heartbeat.command,
            PutawayCommand::Heartbeat {
                task_id: 11,
                ref idempotency_key
            } if idempotency_key == "heartbeat-key-1"
        ));
        state.apply_at(
            success(
                heartbeat,
                PutawayTransportOutcome::Heartbeat(PutawayClaimHeartbeatResponse {
                    task_id: 11,
                    heartbeat_at: "2026-07-27T00:00:00Z".into(),
                    lease_expires_at: "2026-07-27T00:30:00Z".into(),
                }),
            ),
            101.0,
        );

        assert!(state
            .poll_heartbeat(400.9, "too-early".into(), "new-key".into())
            .is_none());
        let next = state
            .poll_heartbeat(401.0, "heartbeat-2".into(), "heartbeat-key-2".into())
            .unwrap();
        assert_eq!(next.command.idempotency_key(), Some("heartbeat-key-2"));
        assert_eq!(
            state.expected_scan(),
            Some(PutawayScanStage::SourceLocation)
        );
    }

    #[test]
    fn ambiguous_heartbeat_retries_same_key_without_blocking_scan() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current".into()).unwrap();
        state.apply_at(
            success(
                current,
                PutawayTransportOutcome::Current(Some(loose_claim())),
            ),
            0.0,
        );
        let first = state
            .poll_heartbeat(0.0, "heartbeat-1".into(), "stable-heartbeat".into())
            .unwrap();
        state.apply_at(
            PutawayTransportEvent {
                request: first.clone(),
                outcome: Err(PutawayTransportFailure {
                    status: None,
                    error: None,
                    message: "connection closed".into(),
                }),
            },
            1.0,
        );

        assert!(state
            .poll_heartbeat(2.9, "early-retry".into(), "unused".into())
            .is_none());
        let retry = state
            .poll_heartbeat(3.0, "heartbeat-2".into(), "unused".into())
            .unwrap();
        assert_eq!(retry.command, first.command);
        assert_eq!(retry.command.idempotency_key(), Some("stable-heartbeat"));
        assert_eq!(
            state.expected_scan(),
            Some(PutawayScanStage::SourceLocation)
        );
        let mut late = retry.clone();
        late.request_id = "heartbeat-1".into();
        assert_eq!(
            state.apply_at(
                success(
                    late,
                    PutawayTransportOutcome::Heartbeat(PutawayClaimHeartbeatResponse {
                        task_id: 11,
                        heartbeat_at: "2026-07-27T00:00:00Z".into(),
                        lease_expires_at: "2026-07-27T00:30:00Z".into(),
                    })
                ),
                4.0
            ),
            PutawayApplyResult::Ignored
        );
    }

    #[test]
    fn timed_out_heartbeat_retries_exact_command_then_reconciles() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current".into()).unwrap();
        state.apply_at(
            success(
                current,
                PutawayTransportOutcome::Current(Some(loose_claim())),
            ),
            0.0,
        );
        let first = state
            .poll_heartbeat(0.0, "heartbeat-1".into(), "stable-key".into())
            .unwrap();

        assert!(state
            .poll_heartbeat(9.9, "too-early".into(), "unused".into())
            .is_none());
        for (attempt, now) in [(2, 10.0), (3, 20.0), (4, 30.0), (5, 40.0)] {
            let retry = state
                .poll_heartbeat(now, format!("heartbeat-{attempt}"), "unused".into())
                .unwrap();
            assert_eq!(retry.command, first.command);
            assert_eq!(retry.command.idempotency_key(), Some("stable-key"));
        }
        assert!(state
            .poll_heartbeat(50.0, "heartbeat-6".into(), "unused".into())
            .is_none());
        assert_eq!(state.activity(), PutawayActivity::ReconcileRequired);
        assert!(state.expected_scan().is_none());
    }

    #[test]
    fn terminal_foreground_command_keeps_heartbeat_lane_active() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current".into()).unwrap();
        state.apply_at(
            success(
                current,
                PutawayTransportOutcome::Current(Some(loose_claim())),
            ),
            0.0,
        );
        *state.scan_draft_mut() = "RECV-01".into();
        state.submit_scan("unused".into(), "unused".into());
        *state.scan_draft_mut() = "A-01-01".into();
        let confirmation = state
            .submit_scan("confirm".into(), "confirm-key".into())
            .unwrap();
        let heartbeat = state
            .poll_heartbeat(0.0, "heartbeat-1".into(), "heartbeat-key-1".into())
            .unwrap();

        state.apply_at(
            PutawayTransportEvent {
                request: confirmation,
                outcome: Err(PutawayTransportFailure {
                    status: None,
                    error: None,
                    message: "connection closed".into(),
                }),
            },
            1.0,
        );
        state.apply_at(
            success(
                heartbeat,
                PutawayTransportOutcome::Heartbeat(PutawayClaimHeartbeatResponse {
                    task_id: 11,
                    heartbeat_at: "2026-07-27T00:00:00Z".into(),
                    lease_expires_at: "2026-07-27T00:30:00Z".into(),
                }),
            ),
            1.0,
        );

        assert_eq!(state.activity(), PutawayActivity::Retryable);
        assert!(state
            .poll_heartbeat(301.0, "heartbeat-2".into(), "heartbeat-key-2".into())
            .is_some());
    }

    #[test]
    fn known_lease_deadline_requires_reconciliation_before_expiry() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current".into()).unwrap();
        state.apply_at(
            success(
                current,
                PutawayTransportOutcome::Current(Some(loose_claim())),
            ),
            0.0,
        );
        let heartbeat = state
            .poll_heartbeat(0.0, "heartbeat-1".into(), "heartbeat-key-1".into())
            .unwrap();
        state.apply_at(
            success(
                heartbeat,
                PutawayTransportOutcome::Heartbeat(PutawayClaimHeartbeatResponse {
                    task_id: 11,
                    heartbeat_at: "2026-07-27T00:00:00Z".into(),
                    lease_expires_at: "2026-07-27T00:00:30Z".into(),
                }),
            ),
            0.0,
        );

        assert!(state
            .poll_heartbeat(28.0, "heartbeat-2".into(), "heartbeat-key-2".into())
            .is_none());
        assert_eq!(state.activity(), PutawayActivity::ReconcileRequired);
    }

    #[test]
    fn old_heartbeat_replay_near_expiry_requests_a_fresh_renewal() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current".into()).unwrap();
        state.apply_at(
            success(
                current,
                PutawayTransportOutcome::Current(Some(loose_claim())),
            ),
            0.0,
        );
        let heartbeat = state
            .poll_heartbeat(0.0, "heartbeat-1".into(), "old-key".into())
            .unwrap();
        state.apply_at(
            success(
                heartbeat,
                PutawayTransportOutcome::Heartbeat(PutawayClaimHeartbeatResponse {
                    task_id: 11,
                    heartbeat_at: "2026-07-27T00:00:00Z".into(),
                    lease_expires_at: "2026-07-27T00:30:00Z".into(),
                }),
            ),
            1_795.0,
        );

        let fresh = state
            .poll_heartbeat(1_795.0, "heartbeat-2".into(), "fresh-key".into())
            .unwrap();
        assert_eq!(fresh.command.idempotency_key(), Some("fresh-key"));
    }

    #[test]
    fn heartbeat_conflict_requires_authoritative_reconciliation() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current".into()).unwrap();
        state.apply_at(
            success(
                current,
                PutawayTransportOutcome::Current(Some(loose_claim())),
            ),
            0.0,
        );
        let heartbeat = state
            .poll_heartbeat(0.0, "heartbeat".into(), "heartbeat-key".into())
            .unwrap();
        state.apply_at(
            PutawayTransportEvent {
                request: heartbeat,
                outcome: Err(PutawayTransportFailure {
                    status: Some(409),
                    error: Some(wareboxes_api_contract::v1::ErrorResponse::new(
                        ErrorReason::Conflict,
                        "claim expired",
                        "request-heartbeat",
                    )),
                    message: "claim expired".into(),
                }),
            },
            1.0,
        );

        assert_eq!(state.activity(), PutawayActivity::ReconcileRequired);
        assert!(state.expected_scan().is_none());
        assert!(state.begin_current("reconcile".into()).is_some());
    }

    #[test]
    fn reasoned_release_is_retryable_and_clears_active_work() {
        let mut state = PutawayWorkflowState::default();
        let current = state.begin_current("current".into()).unwrap();
        state.apply_at(
            success(
                current,
                PutawayTransportOutcome::Current(Some(loose_claim())),
            ),
            0.0,
        );
        let release = state
            .begin_release(
                "release-1".into(),
                "release-key".into(),
                PutawayClaimReleaseReason::DestinationBlocked,
                Some("Rack inspection".into()),
            )
            .unwrap();
        state.apply_at(
            PutawayTransportEvent {
                request: release.clone(),
                outcome: Err(PutawayTransportFailure {
                    status: None,
                    error: None,
                    message: "connection closed".into(),
                }),
            },
            1.0,
        );
        let retry = state.retry("release-2".into()).unwrap();
        assert_eq!(retry.command, release.command);

        let result = state.apply_at(
            success(
                retry,
                PutawayTransportOutcome::Released(PutawayClaimReleaseResponse {
                    task_id: 11,
                    released_at: "2026-07-27T00:10:00Z".into(),
                    release_count: 1,
                    reason: PutawayClaimReleaseReason::DestinationBlocked,
                    note: Some("Rack inspection".into()),
                }),
            ),
            2.0,
        );

        assert!(matches!(result, PutawayApplyResult::Completed(_)));
        assert!(state.claim().is_none());
        assert!(state
            .poll_heartbeat(10.0, "heartbeat".into(), "key".into())
            .is_none());
    }
}
