use eframe::egui;
use lucide_icons::Icon;

use crate::cycle_count::{CountScanStage, CycleCountClaim};
use crate::workflow::{Activity, Transition, WorkflowEffect};

use super::RfApp;

impl RfApp {
    pub(super) fn count_view(&mut self, ui: &mut egui::Ui) {
        if self.cycle_count.claim().is_some() {
            self.count_active(ui);
        } else if self.cycle_count.activity() == Activity::Idle {
            self.count_idle(ui);
        } else if self.cycle_count.activity() == Activity::Active {
            self.cycle_count
                .require_reconciliation("Active count is missing its task details".into());
        }
        self.count_command_state(ui);
        if let Some(error) = self.cycle_count.error() {
            ui.add_space(8.0);
            ui.colored_label(Self::danger(), egui::RichText::new(error).strong());
        }
        if let Some(notice) = self.cycle_count.notice() {
            ui.add_space(8.0);
            ui.colored_label(Self::warning(), notice);
        }
    }

    fn count_idle(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label("Blind item-location counts");
        let can_execute = self.can_execute();
        if Self::full_width_button(
            ui,
            can_execute,
            egui::Button::new(egui::RichText::new("Get next count").strong())
                .fill(Self::primary_fill(can_execute)),
            58.0,
        )
        .clicked()
        {
            let (command_id, key) = Self::command_identity("count-claim");
            let effect = self.cycle_count.begin_claim_next(command_id, key);
            self.emit_count(effect);
        }
    }

    fn count_active(&mut self, ui: &mut egui::Ui) {
        let Some(claim) = self.cycle_count.claim().cloned() else {
            return;
        };
        Self::task_reference(ui, &format!("Count {}", claim.task_id), claim.priority);
        if let Some(instructions) = claim.instructions.as_deref() {
            Self::message_band(ui, Self::warning(), Icon::AlertTriangle, instructions);
        }

        let lease_actions_allowed = if self.cycle_count.activity() == Activity::Active {
            self.heartbeat_status(ui, claim.task_id)
        } else {
            false
        };
        if let Some(stage) = self.cycle_count.expected_scan() {
            let expected = match stage {
                CountScanStage::Location => Some(claim.location_barcode.as_str()),
                CountScanStage::Item => claim.item_barcodes.first().map(String::as_str),
                CountScanStage::LicensePlate => claim.license_plate_barcode.as_deref(),
            };
            self.count_scan_control(ui, claim.task_id, stage, expected, lease_actions_allowed);
        } else {
            self.count_quantity_control(ui, lease_actions_allowed);
        }

        ui.add_space(4.0);
        Self::section_label(ui, "COUNT DETAILS");
        Self::count_location_band(ui, &claim);
        Self::count_item_band(ui, &claim);

        ui.add_space(8.0);
        if self.release_confirmation {
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(57, 42, 21))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.strong("Return this count to the queue?");
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
                        if Self::full_width_button(
                            ui,
                            lease_actions_allowed,
                            egui::Button::new("Return to queue")
                                .fill(egui::Color32::from_rgb(112, 72, 18)),
                            48.0,
                        )
                        .clicked()
                        {
                            let (command_id, key) = Self::command_identity("count-release");
                            let effect = self.cycle_count.begin_release(command_id, key);
                            self.emit_count(effect);
                            self.release_confirmation = false;
                        }
                    });
                });
        } else if ui
            .add_enabled(
                lease_actions_allowed,
                Self::secondary_button("Release count", ui.available_width(), 48.0),
            )
            .clicked()
        {
            self.release_confirmation = true;
        }
    }

    fn count_location_band(ui: &mut egui::Ui, claim: &CycleCountClaim) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 60, 56)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(0.0));
                ui.label(egui::RichText::new("COUNT LOCATION").small().strong());
                ui.label(
                    egui::RichText::new(
                        claim
                            .location_name
                            .as_deref()
                            .unwrap_or(&claim.location_barcode),
                    )
                    .size(23.0)
                    .strong(),
                );
                ui.monospace(&claim.location_barcode);
            });
    }

    fn count_item_band(ui: &mut egui::Ui, claim: &CycleCountClaim) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 60, 56)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(0.0));
                ui.label(egui::RichText::new("ITEM").small().strong());
                ui.label(
                    egui::RichText::new(
                        claim
                            .item_description
                            .clone()
                            .unwrap_or_else(|| format!("Item {}", claim.item_id)),
                    )
                    .size(21.0)
                    .strong(),
                );
                ui.label(format!("{} · {}", claim.uom, claim.inventory_status));
                if let Some(lot) = claim.lot.as_deref() {
                    ui.monospace(format!("Lot {lot}"));
                }
                if let Some(serial) = claim.serial.as_deref() {
                    ui.monospace(format!("Serial {serial}"));
                }
                if let Some(plate) = claim.license_plate_barcode.as_deref() {
                    ui.monospace(format!("LPN {plate}"));
                }
            });
    }

    fn count_scan_control(
        &mut self,
        ui: &mut egui::Ui,
        task_id: i64,
        stage: CountScanStage,
        expected: Option<&str>,
        lease_actions_allowed: bool,
    ) {
        let (response, clicked) = Self::scanner_action(
            ui,
            stage.prompt(),
            expected,
            "Confirm scan",
            lease_actions_allowed,
            self.cycle_count.scan_draft_mut(),
            egui::Id::new(("count_scan", task_id, stage)),
        );
        let focus_key = (task_id, stage);
        if lease_actions_allowed && self.count_scan_focus != Some(focus_key) {
            response.request_focus();
            self.count_scan_focus = Some(focus_key);
        } else if !lease_actions_allowed {
            self.count_scan_focus = None;
        }
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if lease_actions_allowed && (enter || clicked) {
            self.cycle_count.submit_scan();
            self.count_scan_focus = None;
        }
    }

    fn count_quantity_control(&mut self, ui: &mut egui::Ui, lease_actions_allowed: bool) {
        let width = ui.available_width();
        let (quantity, clicked) = egui::Frame::new()
            .fill(Self::accent().gamma_multiply(0.08))
            .stroke(egui::Stroke::new(1.0, Self::accent()))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(0.0));
                ui.vertical_centered(|ui| {
                    Self::section_label(ui, "NEXT ACTION");
                    ui.label(
                        egui::RichText::new("Enter observed quantity")
                            .size(23.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                });
                let quantity = ui.add_enabled(
                    lease_actions_allowed,
                    Self::centered_text_edit(
                        egui::TextEdit::singleline(self.cycle_count.quantity_draft_mut())
                            .id(egui::Id::new("count_quantity"))
                            .font(egui::TextStyle::Monospace),
                    ),
                );
                let quantity_empty = self.cycle_count.quantity_draft_mut().is_empty();
                Self::centered_hint(
                    ui,
                    &quantity,
                    quantity_empty,
                    "0",
                    egui::TextStyle::Monospace,
                );
                ui.add_enabled(
                    lease_actions_allowed,
                    egui::TextEdit::singleline(self.cycle_count.note_draft_mut())
                        .id(egui::Id::new("count_note"))
                        .hint_text("Optional note"),
                );
                let ready = self
                    .cycle_count
                    .quantity_draft_mut()
                    .trim()
                    .parse::<i64>()
                    .is_ok_and(|quantity| quantity >= 0);
                let can_record = ready && lease_actions_allowed;
                let clicked = Self::full_width_button(
                    ui,
                    can_record,
                    egui::Button::new(egui::RichText::new("Record count").strong())
                        .fill(Self::primary_fill(can_record)),
                    58.0,
                )
                .clicked();
                (quantity, clicked)
            })
            .inner;
        let enter = quantity.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if lease_actions_allowed && (enter || clicked) {
            let (command_id, key) = Self::command_identity("count-confirm");
            let effect = self.cycle_count.begin_confirmation(command_id, key);
            self.emit_count(effect);
        }
    }

    fn count_command_state(&mut self, ui: &mut egui::Ui) {
        match self.cycle_count.activity() {
            Activity::Persisting => Self::state_band(
                ui,
                Self::warning(),
                Icon::Save,
                "Saving count",
                "Do not submit again",
            ),
            Activity::ReadyToDispatch => Self::state_band(
                ui,
                Self::warning(),
                Icon::Send,
                "Count saved",
                "Waiting for connection. Do not submit again.",
            ),
            Activity::InFlight => Self::state_band(
                ui,
                Self::warning(),
                Icon::Loader,
                "Sending count",
                "Waiting for the server. Do not submit again.",
            ),
            Activity::Ambiguous => {
                Self::state_band(
                    ui,
                    Self::danger(),
                    Icon::AlertTriangle,
                    "Checking last count",
                    self.cycle_count
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
                    let effect = self.cycle_count.retry_ambiguous();
                    self.emit_count(effect);
                }
            }
            Activity::ReconcileRequired => Self::state_band(
                ui,
                Self::danger(),
                Icon::ShieldAlert,
                "Count blocked",
                self.cycle_count
                    .reconcile_reason()
                    .unwrap_or("Device and server state must be reconciled"),
            ),
            Activity::Idle | Activity::Active => {}
        }
    }

    pub(super) fn emit_count(&mut self, effect: Option<WorkflowEffect>) {
        if let Some(effect) = effect {
            self.cycle_count_effects.push_back(effect);
        }
    }

    pub(super) fn emit_count_transition(&mut self, transition: Transition) {
        if let Transition::Effect(effect) = transition {
            self.cycle_count_effects.push_back(effect);
        }
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    pub(super) fn load_count_preview(&mut self) {
        self.work_mode = super::WorkMode::Count;
        self.cycle_count
            .restore_current_claim(Some(CycleCountClaim {
                task_id: 3_042,
                inventory_owner_id: 12,
                facility_id: 4,
                priority: 90,
                instructions: Some("Count each unit in the active pick face".into()),
                lease_expires_at: (chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339(),
                location_id: 71,
                location_name: Some("A-03-02".into()),
                location_barcode: "A-03-02".into(),
                item_id: 88,
                item_description: Some("Nitrile gloves, medium".into()),
                item_barcodes: vec!["CASE-100".into()],
                inventory_balance_id: 144,
                license_plate_barcode: None,
                uom: "EA".into(),
                lot: Some("LOT-2407".into()),
                serial: None,
                inventory_status: "available".into(),
            }));
    }
}
