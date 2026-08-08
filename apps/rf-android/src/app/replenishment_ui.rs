use eframe::egui;
use lucide_icons::Icon;

use crate::replenishment::{ReplenishmentClaim, ReplenishmentLocation, ReplenishmentScanStage};
use crate::workflow::{Activity, Transition, WorkflowEffect};

use super::RfApp;

impl RfApp {
    pub(super) fn replenishment_view(&mut self, ui: &mut egui::Ui) {
        self.replenishment_command_state(ui);
        if let Some(error) = self.replenishment.error() {
            ui.add_space(8.0);
            Self::message_band(ui, Self::danger(), Icon::ScanLine, error);
        }
        if let Some(notice) = self.replenishment.notice() {
            ui.add_space(8.0);
            Self::message_band(ui, Self::accent(), Icon::CheckCircle, notice);
        }
        if self.replenishment.claim().is_some() {
            self.replenishment_active(ui);
        } else if self.replenishment.activity() == Activity::Idle {
            self.replenishment_idle(ui);
        } else if self.replenishment.activity() == Activity::Active {
            self.replenishment
                .require_reconciliation("Active replenishment is missing its task details".into());
        }
    }

    fn replenishment_idle(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label("No replenishment assigned");
        let can_execute = self.can_execute();
        if ui
            .add_enabled(
                can_execute,
                egui::Button::new(egui::RichText::new("Get next replenishment").strong())
                    .fill(Self::primary_fill(can_execute))
                    .min_size(egui::vec2(ui.available_width(), 58.0)),
            )
            .clicked()
        {
            let (command_id, key) = Self::command_identity("replenishment-claim");
            let effect = self.replenishment.begin_claim_next(command_id, key);
            self.emit_replenishment(effect);
        }

        ui.add_space(12.0);
        ui.label(egui::RichText::new("CLAIM SPECIFIC TASK").small().strong());
        let task_id = ui.add_enabled(
            can_execute,
            egui::TextEdit::singleline(&mut self.replenishment_task_id_draft)
                .font(egui::TextStyle::Monospace)
                .hint_text("Task ID"),
        );
        let parsed_task_id = self
            .replenishment_task_id_draft
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|task_id| *task_id > 0);
        let enter = task_id.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let clicked = ui
            .add_enabled(
                can_execute && parsed_task_id.is_some(),
                Self::secondary_button("Claim task", ui.available_width(), 50.0),
            )
            .clicked();
        if can_execute
            && (enter || clicked)
            && let Some(task_id) = parsed_task_id
        {
            let (command_id, key) = Self::command_identity("replenishment-claim-id");
            let effect = self
                .replenishment
                .begin_claim_by_id(task_id, command_id, key);
            self.emit_replenishment(effect);
        }
    }

    fn replenishment_active(&mut self, ui: &mut egui::Ui) {
        let Some(claim) = self.replenishment.claim().cloned() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(format!("TASK {}", claim.work_id))
                    .strong()
                    .color(Self::accent()),
            );
            ui.separator();
            ui.label(format!("Stop {}", claim.sequence));
            ui.separator();
            ui.label(format!("Priority {}", claim.priority));
        });
        Self::replenishment_location_band(ui, "RESERVE SOURCE", &claim.source_location);
        Self::replenishment_item_band(ui, &claim);
        Self::replenishment_location_band(
            ui,
            "DESTINATION PICK FACE",
            &claim.destination_pick_face,
        );
        if let Some(instructions) = claim.instructions.as_deref() {
            ui.label(
                egui::RichText::new(instructions)
                    .color(Self::warning())
                    .strong(),
            );
        }

        let lease_actions_allowed = if self.replenishment.activity() == Activity::Active {
            self.heartbeat_status(ui, claim.work_id)
        } else {
            false
        };
        if let Some(stage) = self.replenishment.expected_scan() {
            self.replenishment_scan_control(ui, claim.work_id, stage, lease_actions_allowed);
        } else {
            self.replenishment_confirm_control(ui, &claim, lease_actions_allowed);
        }
        self.replenishment_release_control(ui, lease_actions_allowed);
    }

    fn replenishment_location_band(
        ui: &mut egui::Ui,
        label: &str,
        location: &ReplenishmentLocation,
    ) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_min_width((width - 20.0).max(0.0));
                ui.label(egui::RichText::new(label).small().strong());
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(location.name.as_deref().unwrap_or(&location.barcode))
                            .size(19.0)
                            .strong(),
                    );
                    if location.name.is_some() {
                        ui.monospace(&location.barcode);
                    }
                });
            });
    }

    fn replenishment_item_band(ui: &mut egui::Ui, claim: &ReplenishmentClaim) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_min_width((width - 20.0).max(0.0));
                ui.label(egui::RichText::new("MOVE").small().strong());
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} {}", claim.quantity, claim.uom))
                            .size(25.0)
                            .strong()
                            .color(RfApp::accent()),
                    );
                    ui.label(
                        egui::RichText::new(
                            claim
                                .item_description
                                .clone()
                                .unwrap_or_else(|| format!("Item {}", claim.item_id)),
                        )
                        .size(19.0)
                        .strong(),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    if let Some(lot) = claim.lot.as_deref() {
                        ui.monospace(format!("Lot {lot}"));
                    }
                    if let Some(serial) = claim.serial.as_deref() {
                        ui.monospace(format!("Serial {serial}"));
                    }
                    if let Some(expiration) = claim.expiration.as_deref() {
                        ui.label(format!(
                            "Expires {}",
                            expiration
                                .split_once('T')
                                .map_or(expiration, |(date, _)| date)
                        ));
                    }
                });
            });
    }

    fn replenishment_scan_control(
        &mut self,
        ui: &mut egui::Ui,
        work_id: i64,
        stage: ReplenishmentScanStage,
        lease_actions_allowed: bool,
    ) {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(stage.prompt())
                .size(19.0)
                .strong()
                .color(Self::accent()),
        );
        let response = ui
            .add_enabled_ui(lease_actions_allowed, |ui| {
                ui.add_sized(
                    [ui.available_width(), 56.0],
                    egui::TextEdit::singleline(self.replenishment.scan_draft_mut())
                        .id(egui::Id::new(("replenishment_scan", work_id, stage)))
                        .font(egui::TextStyle::Monospace)
                        .hint_text("SCAN"),
                )
            })
            .inner;
        let focus_key = (work_id, stage);
        if lease_actions_allowed && self.replenishment_scan_focus != Some(focus_key) {
            response.request_focus();
            self.replenishment_scan_focus = Some(focus_key);
        } else if !lease_actions_allowed {
            self.replenishment_scan_focus = None;
        }
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let scan_ready = !self.replenishment.scan_draft_mut().trim().is_empty();
        let clicked = ui
            .add_enabled(
                scan_ready && lease_actions_allowed,
                egui::Button::new(egui::RichText::new("Confirm scan").strong())
                    .fill(Self::primary_fill(scan_ready && lease_actions_allowed))
                    .min_size(egui::vec2(ui.available_width(), 54.0)),
            )
            .clicked();
        if lease_actions_allowed && (enter || clicked) {
            self.replenishment.submit_scan();
            self.replenishment_scan_focus = None;
        }
    }

    fn replenishment_confirm_control(
        &mut self,
        ui: &mut egui::Ui,
        claim: &ReplenishmentClaim,
        lease_actions_allowed: bool,
    ) {
        ui.add_space(6.0);
        let label = format!("Confirm move of {} {}", claim.quantity, claim.uom);
        if ui
            .add_enabled(
                lease_actions_allowed,
                egui::Button::new(egui::RichText::new(label).strong())
                    .fill(Self::primary_fill(lease_actions_allowed))
                    .min_size(egui::vec2(ui.available_width(), 58.0)),
            )
            .clicked()
        {
            let (command_id, key) = Self::command_identity("replenishment-confirm");
            let effect = self.replenishment.begin_confirmation(command_id, key);
            self.emit_replenishment(effect);
        }
    }

    fn replenishment_release_control(&mut self, ui: &mut egui::Ui, allowed: bool) {
        ui.add_space(8.0);
        if self.release_confirmation {
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(57, 42, 21))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.strong("Return this replenishment to the queue?");
                    ui.horizontal(|ui| {
                        if ui
                            .add(Self::secondary_button(
                                "Cancel",
                                ui.available_width() / 2.0 - 4.0,
                                48.0,
                            ))
                            .clicked()
                        {
                            self.release_confirmation = false;
                        }
                        if ui
                            .add_enabled(
                                allowed,
                                egui::Button::new("Return to queue")
                                    .fill(egui::Color32::from_rgb(112, 72, 18))
                                    .min_size(egui::vec2(ui.available_width(), 48.0)),
                            )
                            .clicked()
                        {
                            let (command_id, key) = Self::command_identity("replenishment-release");
                            let effect = self.replenishment.begin_release(command_id, key);
                            self.emit_replenishment(effect);
                            self.release_confirmation = false;
                        }
                    });
                });
        } else if ui
            .add_enabled(
                allowed,
                Self::secondary_button("Release task", ui.available_width(), 48.0),
            )
            .clicked()
        {
            self.release_confirmation = true;
        }
    }

    fn replenishment_command_state(&mut self, ui: &mut egui::Ui) {
        match self.replenishment.activity() {
            Activity::Persisting => Self::state_band(
                ui,
                Self::warning(),
                Icon::Save,
                "Saving replenishment",
                "Do not submit again",
            ),
            Activity::ReadyToDispatch => Self::state_band(
                ui,
                Self::warning(),
                Icon::Send,
                "Replenishment saved",
                "Waiting for connection. Do not submit again.",
            ),
            Activity::InFlight => Self::state_band(
                ui,
                Self::warning(),
                Icon::Loader,
                "Sending replenishment",
                "Waiting for the server. Do not submit again.",
            ),
            Activity::Ambiguous => {
                Self::state_band(
                    ui,
                    Self::danger(),
                    Icon::AlertTriangle,
                    "Checking last replenishment",
                    self.replenishment
                        .ambiguous_message()
                        .unwrap_or("The command result is unknown"),
                );
                if ui
                    .add_sized(
                        [ui.available_width(), 54.0],
                        egui::Button::new("Check again").fill(egui::Color32::from_rgb(112, 72, 18)),
                    )
                    .clicked()
                {
                    let effect = self.replenishment.retry_ambiguous();
                    self.emit_replenishment(effect);
                }
            }
            Activity::ReconcileRequired => Self::state_band(
                ui,
                Self::danger(),
                Icon::ShieldAlert,
                "Replenishment blocked",
                self.replenishment
                    .reconcile_reason()
                    .unwrap_or("Device and server state must be reconciled"),
            ),
            Activity::Idle | Activity::Active => {}
        }
    }

    pub(super) fn emit_replenishment(&mut self, effect: Option<WorkflowEffect>) {
        if let Some(effect) = effect {
            self.replenishment_effects.push_back(effect);
        }
    }

    pub(super) fn emit_replenishment_transition(&mut self, transition: Transition) {
        if let Transition::Effect(effect) = transition {
            self.replenishment_effects.push_back(effect);
        }
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    pub(super) fn load_replenishment_preview(&mut self) {
        self.work_mode = super::WorkMode::Replenish;
        self.workflow = crate::workflow::MovementWorkflow::default();
        let claim = debug_claim();
        self.load_verified_debug_heartbeat(claim.work_id);
        self.replenishment.load_debug_claim(claim);
    }
}

#[cfg(all(debug_assertions, not(target_os = "android")))]
fn debug_claim() -> ReplenishmentClaim {
    ReplenishmentClaim {
        work_id: 5_041,
        plan_id: 701,
        policy_id: 88,
        policy_revision: 3,
        inventory_owner_id: 12,
        facility_id: 4,
        sequence: 1,
        priority: 90,
        instructions: Some("Keep reserve case labels facing the aisle".into()),
        due_at: None,
        lease_expires_at: (chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339(),
        source_inventory_balance_id: 144,
        item_batch_id: 301,
        item_id: 88,
        item_description: Some("Nitrile gloves, medium".into()),
        item_barcodes: vec!["CASE-100".into(), "0081234500017".into()],
        uom: "EA".into(),
        lot: Some("LOT-2407".into()),
        serial: None,
        expiration: Some("2027-07-31T00:00:00Z".into()),
        quantity: 24,
        source_location: ReplenishmentLocation {
            location_id: 71,
            barcode: "R-04-12".into(),
            name: Some("Reserve R-04-12".into()),
        },
        destination_pick_face: ReplenishmentLocation {
            location_id: 72,
            barcode: "A-03-02".into(),
            name: Some("Pick A-03-02".into()),
        },
    }
}
