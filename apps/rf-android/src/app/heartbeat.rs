use std::time::{Duration, Instant};

use chrono::Utc;
use eframe::egui;
use lucide_icons::Icon;
use wareboxes_api_contract::v1::{ErrorReason, ErrorResponse};

use crate::lease::{
    ActionBlockReason, ClockSample, HeartbeatAttemptId, HeartbeatFailureKind, HeartbeatLease,
    HeartbeatOutcome, HeartbeatState, LeasePolicy, MovementLeaseMonitor,
};
use crate::transport::{
    AuthenticatedTransport, NetworkResponse, build_movement_heartbeat_request, send_heartbeat,
};
use crate::wire::{decode_heartbeat_response, decode_relocation_heartbeat_response};
use crate::workflow::{Activity, MovementOperation};

use super::RfApp;

struct ExpectedHeartbeat {
    operation: MovementOperation,
    task_id: i64,
    attempt_id: HeartbeatAttemptId,
    request_id: String,
}

pub(super) struct HeartbeatRuntime {
    epoch: Instant,
    monitor: Option<MovementLeaseMonitor>,
    expected: Option<ExpectedHeartbeat>,
    idempotency_key: Option<String>,
}

impl HeartbeatRuntime {
    pub(super) fn new() -> Self {
        Self {
            epoch: Instant::now(),
            monitor: None,
            expected: None,
            idempotency_key: None,
        }
    }

    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    fn reset(&mut self) {
        self.monitor = None;
        self.expected = None;
        self.idempotency_key = None;
    }
}

impl RfApp {
    pub(super) fn maintain_claim_heartbeat(&mut self, context: &egui::Context) {
        if self.lease_check_task_id.is_some() {
            self.heartbeat.reset();
            context.request_repaint_after(Duration::from_secs(1));
            return;
        }

        let active_claim = (self.workflow.activity() == Activity::Active)
            .then(|| self.workflow.claim())
            .flatten()
            .map(|claim| (claim.task_id, claim.lease_expires_at.clone()));
        let Some((task_id, lease_expires_at)) = active_claim else {
            self.heartbeat.reset();
            return;
        };

        let now = self.heartbeat.now();
        if self
            .heartbeat
            .monitor
            .as_ref()
            .is_none_or(|monitor| monitor.task_id() != task_id)
        {
            match MovementLeaseMonitor::new(
                task_id,
                &lease_expires_at,
                ClockSample::new(Utc::now(), now),
                LeasePolicy::default(),
            ) {
                Ok(monitor) => {
                    self.heartbeat.reset();
                    self.heartbeat.monitor = Some(monitor);
                }
                Err(_) => {
                    self.heartbeat.reset();
                    self.workflow.require_reconciliation(
                        "The task lease returned by the server is invalid.".into(),
                    );
                    return;
                }
            }
        }

        let timeout_result = self
            .heartbeat
            .monitor
            .as_mut()
            .map(|monitor| monitor.expire_timed_out_heartbeat(now));
        match timeout_result {
            Some(Ok(Some(attempt_id))) => {
                if self
                    .heartbeat
                    .expected
                    .as_ref()
                    .is_some_and(|expected| expected.attempt_id == attempt_id)
                {
                    self.heartbeat.expected = None;
                }
            }
            Some(Err(_)) => {
                self.fail_heartbeat_integrity();
                return;
            }
            Some(Ok(None)) | None => {}
        }

        let Some(attempt) = self
            .heartbeat
            .monitor
            .as_mut()
            .and_then(|monitor| monitor.begin_heartbeat(now))
        else {
            let poll = if matches!(
                self.heartbeat
                    .monitor
                    .as_ref()
                    .map(MovementLeaseMonitor::heartbeat_state),
                Some(HeartbeatState::InFlight { .. })
            ) {
                Duration::from_secs(1)
            } else {
                Duration::from_secs(5)
            };
            context.request_repaint_after(poll);
            return;
        };

        let Some(session) = self.session.as_ref() else {
            return;
        };
        let idempotency_key = self
            .heartbeat
            .idempotency_key
            .get_or_insert_with(|| format!("rf-heartbeat-{}", uuid::Uuid::new_v4()))
            .clone();
        let request_id = format!("rf-{}", uuid::Uuid::new_v4());
        let transport = AuthenticatedTransport {
            endpoint: &session.endpoint,
            token: &session.token,
            scope: &session.scope,
        };
        let operation = self.workflow.operation();
        let request = match build_movement_heartbeat_request(
            &transport,
            operation,
            attempt.task_id,
            &request_id,
            &idempotency_key,
        ) {
            Ok(request) => request,
            Err(_) => {
                self.fail_heartbeat_integrity();
                return;
            }
        };
        self.heartbeat.expected = Some(ExpectedHeartbeat {
            operation,
            task_id: attempt.task_id,
            attempt_id: attempt.id,
            request_id: request_id.clone(),
        });
        send_heartbeat(
            request,
            operation,
            attempt.task_id,
            request_id,
            self.network_tx.clone(),
            context.clone(),
        );
        context.request_repaint_after(Duration::from_secs(1));
    }

    pub(super) fn handle_heartbeat_response(
        &mut self,
        context: &egui::Context,
        operation: MovementOperation,
        task_id: i64,
        request_id: &str,
        response: Result<NetworkResponse, String>,
    ) {
        let Some(expected) = self.heartbeat.expected.as_ref() else {
            return;
        };
        if expected.operation != operation
            || expected.task_id != task_id
            || expected.request_id != request_id
        {
            return;
        }
        if self.workflow.operation() != operation
            || self.workflow.activity() != Activity::Active
            || self
                .workflow
                .claim()
                .is_none_or(|claim| claim.task_id != task_id)
        {
            self.heartbeat.reset();
            return;
        }
        let Some(expected) = self.heartbeat.expected.take() else {
            return;
        };
        let now = self.heartbeat.now();
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                self.record_retryable_heartbeat(expected.attempt_id, now);
                return;
            }
        };

        if (200..300).contains(&response.status) {
            let heartbeat = match operation {
                MovementOperation::Putaway => {
                    decode_heartbeat_response(task_id, response.status, &response.body).map(
                        |response| {
                            (
                                response.task_id,
                                response.heartbeat_at,
                                response.lease_expires_at,
                            )
                        },
                    )
                }
                MovementOperation::InventoryRelocation => {
                    decode_relocation_heartbeat_response(task_id, response.status, &response.body)
                        .map(|response| {
                            (
                                response.task_id,
                                response.heartbeat_at,
                                response.lease_expires_at,
                            )
                        })
                }
            };
            let (heartbeat_task_id, heartbeat_at, lease_expires_at) = match heartbeat {
                Ok(heartbeat) => heartbeat,
                Err(_) => {
                    self.fail_heartbeat_integrity();
                    return;
                }
            };
            let result = self.heartbeat.monitor.as_mut().map(|monitor| {
                monitor.heartbeat_succeeded(
                    expected.attempt_id,
                    HeartbeatLease {
                        task_id: heartbeat_task_id,
                        heartbeat_at: &heartbeat_at,
                        lease_expires_at: &lease_expires_at,
                    },
                    now,
                )
            });
            match result {
                Some(Ok(())) => {
                    self.heartbeat.idempotency_key = None;
                    context.request_repaint();
                }
                Some(Err(_)) | None => self.fail_heartbeat_integrity(),
            }
            return;
        }

        match response.status {
            401 => {
                self.heartbeat.reset();
                self.require_reauthentication_for_task(task_id);
            }
            408 | 429 | 500..=599 => {
                self.record_retryable_heartbeat(expected.attempt_id, now);
            }
            409 if serde_json::from_slice::<ErrorResponse>(&response.body)
                .is_ok_and(|error| error.reason == ErrorReason::IdempotencyKeyReused) =>
            {
                self.fail_heartbeat_integrity();
            }
            403 | 404 | 409 => {
                let result = self.heartbeat.monitor.as_mut().map(|monitor| {
                    monitor.heartbeat_failed(
                        expected.attempt_id,
                        HeartbeatFailureKind::LeaseRejected,
                        now,
                    )
                });
                if matches!(result, Some(Ok(()))) {
                    self.request_current_claim_after_rejection(context, task_id);
                } else {
                    self.fail_heartbeat_integrity();
                }
            }
            _ => self.fail_heartbeat_integrity(),
        }
    }

    pub(super) fn heartbeat_header(&self) -> Option<(&'static str, egui::Color32)> {
        if self.workflow.activity() != Activity::Active {
            return None;
        }
        if self.lease_check_task_id.is_some() {
            return Some(("CHECK", Self::danger()));
        }
        let now = self.heartbeat.now();
        let monitor = self.heartbeat.monitor.as_ref()?;
        if monitor.action_block_reason(now).is_some() {
            Some(("CHECK", Self::danger()))
        } else if matches!(
            monitor.last_outcome(),
            Some(HeartbeatOutcome::Failed {
                kind: HeartbeatFailureKind::Retryable,
                ..
            })
        ) {
            Some(("RETRY", Self::warning()))
        } else {
            None
        }
    }

    pub(super) fn heartbeat_status(&mut self, ui: &mut egui::Ui, task_id: i64) -> bool {
        let now = self.heartbeat.now();
        let snapshot = self
            .heartbeat
            .monitor
            .as_ref()
            .filter(|monitor| monitor.task_id() == task_id)
            .map(|monitor| {
                (
                    monitor.action_block_reason(now),
                    monitor.last_outcome(),
                    monitor.heartbeat_state(),
                )
            });
        let Some((block_reason, last_outcome, state)) = snapshot else {
            Self::state_band(
                ui,
                Self::danger(),
                Icon::WifiOff,
                "Connection required",
                "Reconnecting. Pause work and do not move or scan inventory.",
            );
            if self.lease_check_task_id == Some(task_id)
                && self.expected_claim_request_id.is_none()
                && ui
                    .add_sized(
                        [ui.available_width(), 54.0],
                        egui::Button::new(egui::RichText::new("Check task").strong())
                            .fill(egui::Color32::from_rgb(112, 72, 18)),
                    )
                    .clicked()
            {
                self.request_current_claim_for_lease(ui.ctx(), task_id);
            }
            return false;
        };

        if block_reason.is_none() {
            if matches!(
                last_outcome,
                Some(HeartbeatOutcome::Failed {
                    kind: HeartbeatFailureKind::Retryable,
                    ..
                })
            ) {
                Self::message_band(
                    ui,
                    Self::warning(),
                    Icon::WifiOff,
                    "Reconnecting. You can keep working.",
                );
            }
            return true;
        }

        let checking = matches!(block_reason, Some(ActionBlockReason::UnverifiedLease))
            && matches!(
                state,
                HeartbeatState::Scheduled { .. }
                    | HeartbeatState::InFlight { .. }
                    | HeartbeatState::RetryScheduled { .. }
            );
        if checking && last_outcome.is_none() {
            Self::state_band(
                ui,
                Self::warning(),
                Icon::Loader,
                "Checking task",
                "Confirming this task is still assigned.",
            );
        } else {
            Self::state_band(
                ui,
                Self::danger(),
                Icon::WifiOff,
                "Connection required",
                "Reconnecting. Pause work and do not move or scan inventory.",
            );
        }

        let can_check = self.session.is_some()
            && self.expected_claim_request_id.is_none()
            && self.lease_check_task_id.is_none()
            && !matches!(block_reason, Some(ActionBlockReason::UnverifiedLease));
        if can_check
            && ui
                .add_sized(
                    [ui.available_width(), 54.0],
                    egui::Button::new(egui::RichText::new("Check task").strong())
                        .fill(egui::Color32::from_rgb(112, 72, 18)),
                )
                .clicked()
        {
            self.request_current_claim_for_lease(ui.ctx(), task_id);
        }
        false
    }

    fn record_retryable_heartbeat(&mut self, attempt_id: HeartbeatAttemptId, now: Duration) {
        let result = self.heartbeat.monitor.as_mut().map(|monitor| {
            monitor.heartbeat_failed(attempt_id, HeartbeatFailureKind::Retryable, now)
        });
        if !matches!(result, Some(Ok(()))) {
            self.fail_heartbeat_integrity();
        }
    }

    fn fail_heartbeat_integrity(&mut self) {
        self.heartbeat.reset();
        self.workflow.require_reconciliation(
            "The task connection could not be verified. Do not move inventory.".into(),
        );
    }

    pub(super) fn clear_claim_heartbeat(&mut self) {
        self.heartbeat.reset();
    }
}
