use eframe::egui;
use lucide_icons::Icon;

use crate::cross_dock::{CrossDockClaim, CrossDockLocation, CrossDockScanStage};
use crate::workflow::{Activity, Transition, WorkflowEffect};

use super::RfApp;

impl RfApp {
    pub(super) fn cross_dock_view(&mut self, ui: &mut egui::Ui) {
        self.cross_dock_command_state(ui);
        if let Some(error) = self.cross_dock.error() {
            ui.add_space(8.0);
            Self::message_band(ui, Self::danger(), Icon::ScanLine, error);
        }
        if let Some(notice) = self.cross_dock.notice() {
            ui.add_space(8.0);
            Self::message_band(ui, Self::accent(), Icon::CheckCircle, notice);
        }
        if let Some(claim) = self.cross_dock.claim().cloned() {
            self.cross_dock_active(ui, &claim);
        } else if self.cross_dock.activity() == Activity::Idle {
            self.cross_dock_idle(ui);
        } else if self.cross_dock.activity() == Activity::Active {
            self.cross_dock
                .require_reconciliation("Active cross-dock work is missing task details".into());
        }
    }

    fn cross_dock_idle(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.vertical_centered(|ui| ui.label("No cross-dock work assigned"));
        let allowed = self.can_execute();
        if Self::full_width_button(
            ui,
            allowed,
            egui::Button::new(egui::RichText::new("Get next cross-dock move").strong())
                .fill(Self::primary_fill(allowed)),
            58.0,
        )
        .clicked()
        {
            let (command_id, key) = Self::command_identity("cross-dock-claim");
            let effect = self.cross_dock.begin_claim_next(command_id, key);
            self.emit_cross_dock(effect);
        }
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("CLAIM SPECIFIC TASK").small().strong());
        });
        let width = ui.available_width();
        let response = ui
            .add_enabled_ui(allowed, |ui| {
                ui.add_sized(
                    [width, 52.0],
                    Self::centered_text_edit(
                        egui::TextEdit::singleline(&mut self.cross_dock_task_id_draft)
                            .font(egui::TextStyle::Monospace),
                    ),
                )
            })
            .inner;
        Self::centered_hint(
            ui,
            &response,
            self.cross_dock_task_id_draft.is_empty(),
            "Task ID",
            egui::TextStyle::Monospace,
        );
        let task_id = self
            .cross_dock_task_id_draft
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|value| *value > 0);
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let clicked = Self::full_width_button(
            ui,
            allowed && task_id.is_some(),
            Self::secondary_button("Claim task", ui.available_width(), 50.0),
            50.0,
        )
        .clicked();
        if allowed
            && (enter || clicked)
            && let Some(task_id) = task_id
        {
            let (command_id, key) = Self::command_identity("cross-dock-claim-id");
            let effect = self.cross_dock.begin_claim_by_id(task_id, command_id, key);
            self.emit_cross_dock(effect);
        }
    }

    fn cross_dock_active(&mut self, ui: &mut egui::Ui, claim: &CrossDockClaim) {
        Self::task_reference(
            ui,
            &format!(
                "Task {}  ·  {} / {}",
                claim.work_id, claim.order_key, claim.order_line_key
            ),
            claim.priority,
        );
        if let Some(instructions) = claim.instructions.as_deref() {
            Self::message_band(ui, Self::warning(), Icon::AlertTriangle, instructions);
        }
        let allowed = if self.cross_dock.activity() == Activity::Active {
            self.heartbeat_status(ui, claim.work_id)
        } else {
            false
        };
        if let Some(stage) = self.cross_dock.expected_scan() {
            let expected = match stage {
                CrossDockScanStage::SourceReceivingLocation => {
                    Some(claim.source_receiving_location.barcode.as_str())
                }
                CrossDockScanStage::Item => claim.item_barcodes.first().map(String::as_str),
                CrossDockScanStage::Lot => claim.lot.as_deref(),
                CrossDockScanStage::Serial => claim.serial.as_deref(),
                CrossDockScanStage::DestinationPickFace => {
                    Some(claim.destination_pick_face.barcode.as_str())
                }
            };
            self.cross_dock_scan_control(ui, claim.work_id, stage, expected, allowed);
        } else {
            self.cross_dock_confirm_control(ui, claim, allowed);
        }
        ui.add_space(4.0);
        Self::section_label(ui, "FLOW DETAILS");
        Self::cross_dock_location_band(ui, "RECEIVING SOURCE", &claim.source_receiving_location);
        Self::cross_dock_item_band(ui, claim);
        Self::cross_dock_location_band(ui, "PICK FACE", &claim.destination_pick_face);
        self.cross_dock_release_control(ui, allowed);
    }

    fn cross_dock_location_band(ui: &mut egui::Ui, label: &str, location: &CrossDockLocation) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 60, 56)))
            .corner_radius(egui::CornerRadius::same(8))
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

    fn cross_dock_item_band(ui: &mut egui::Ui, claim: &CrossDockClaim) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 60, 56)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_min_width((width - 20.0).max(0.0));
                ui.label(egui::RichText::new("MOVE TO DEMAND").small().strong());
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
                });
            });
    }

    fn cross_dock_scan_control(
        &mut self,
        ui: &mut egui::Ui,
        work_id: i64,
        stage: CrossDockScanStage,
        expected: Option<&str>,
        allowed: bool,
    ) {
        let (response, clicked) = Self::scanner_action(
            ui,
            stage.prompt(),
            expected,
            "Confirm scan",
            allowed,
            self.cross_dock.scan_draft_mut(),
            egui::Id::new(("cross_dock_scan", work_id, stage)),
        );
        let focus_key = (work_id, stage);
        if allowed && self.cross_dock_scan_focus != Some(focus_key) {
            response.request_focus();
            self.cross_dock_scan_focus = Some(focus_key);
        } else if !allowed {
            self.cross_dock_scan_focus = None;
        }
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if allowed && (enter || clicked) {
            self.cross_dock.submit_scan();
            self.cross_dock_scan_focus = None;
        }
    }

    fn cross_dock_confirm_control(
        &mut self,
        ui: &mut egui::Ui,
        claim: &CrossDockClaim,
        allowed: bool,
    ) {
        let clicked = Self::full_width_button(
            ui,
            allowed,
            egui::Button::new(
                egui::RichText::new(format!(
                    "Confirm cross-dock move of {} {}",
                    claim.quantity, claim.uom
                ))
                .strong(),
            )
            .fill(Self::primary_fill(allowed)),
            58.0,
        )
        .clicked();
        if clicked {
            let (command_id, key) = Self::command_identity("cross-dock-confirm");
            let effect = self.cross_dock.begin_confirmation(command_id, key);
            self.emit_cross_dock(effect);
        }
    }

    fn cross_dock_release_control(&mut self, ui: &mut egui::Ui, allowed: bool) {
        ui.add_space(8.0);
        if self.release_confirmation {
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(57, 42, 21))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.strong("Return this cross-dock move to the queue?");
                    ui.horizontal(|ui| {
                        if ui
                            .add(Self::secondary_button(
                                "Keep task",
                                ui.available_width() / 2.0 - 4.0,
                                48.0,
                            ))
                            .clicked()
                        {
                            self.release_confirmation = false;
                        }
                        if Self::full_width_button(
                            ui,
                            allowed,
                            egui::Button::new("Return to queue")
                                .fill(egui::Color32::from_rgb(112, 72, 18)),
                            48.0,
                        )
                        .clicked()
                        {
                            let (command_id, key) = Self::command_identity("cross-dock-release");
                            let effect = self.cross_dock.begin_release(command_id, key);
                            self.emit_cross_dock(effect);
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

    fn cross_dock_command_state(&mut self, ui: &mut egui::Ui) {
        match self.cross_dock.activity() {
            Activity::Persisting => Self::state_band(
                ui,
                Self::warning(),
                Icon::Save,
                "Saving cross-dock work",
                "Do not submit again",
            ),
            Activity::ReadyToDispatch => Self::state_band(
                ui,
                Self::warning(),
                Icon::Send,
                "Cross-dock work saved",
                "Waiting for connection. Do not submit again.",
            ),
            Activity::InFlight => Self::state_band(
                ui,
                Self::warning(),
                Icon::Loader,
                "Sending cross-dock work",
                "Waiting for the server. Do not submit again.",
            ),
            Activity::Ambiguous => {
                Self::state_band(
                    ui,
                    Self::danger(),
                    Icon::AlertTriangle,
                    "Checking last cross-dock action",
                    self.cross_dock
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
                    let effect = self.cross_dock.retry_ambiguous();
                    self.emit_cross_dock(effect);
                }
            }
            Activity::ReconcileRequired => Self::state_band(
                ui,
                Self::danger(),
                Icon::ShieldAlert,
                "Cross-dock work blocked",
                self.cross_dock
                    .reconcile_reason()
                    .unwrap_or("Device and server state must be reconciled"),
            ),
            Activity::Idle | Activity::Active => {}
        }
    }

    pub(super) fn emit_cross_dock(&mut self, effect: Option<WorkflowEffect>) {
        if let Some(effect) = effect {
            self.cross_dock_effects.push_back(effect);
        }
    }

    pub(super) fn emit_cross_dock_transition(&mut self, transition: Transition) {
        if let Transition::Effect(effect) = transition {
            self.cross_dock_effects.push_back(effect);
        }
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    pub(super) fn load_cross_dock_preview(&mut self) {
        self.work_mode = super::WorkMode::CrossDock;
        self.workflow = crate::workflow::MovementWorkflow::default();
        let claim = debug_claim();
        self.load_verified_debug_heartbeat(claim.work_id);
        self.cross_dock.load_debug_claim(claim);
    }
}

#[cfg(all(debug_assertions, not(target_os = "android")))]
fn debug_claim() -> CrossDockClaim {
    CrossDockClaim {
        work_id: 6_401,
        plan_id: 912,
        inventory_owner_id: 12,
        facility_id: 4,
        order_id: 2_201,
        order_key: "WB-DEMO-XD-ORDER-02".into(),
        order_line_id: 2_202,
        order_line_key: "1".into(),
        reservation_id: 2_203,
        priority: 68,
        instructions: Some("Move received cases directly to the forward pick face".into()),
        due_at: None,
        lease_expires_at: (chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339(),
        source_receipt_inventory_transaction_id: 8_101,
        source_inventory_balance_id: 8_102,
        item_batch_id: 8_103,
        item_id: 440,
        item_description: Some("Demo cross-dock item".into()),
        item_barcodes: vec!["WB-DEMO-XD-ITEM-02".into()],
        uom: "case".into(),
        lot: Some("WB-DEMO-XD-LOT-02".into()),
        serial: None,
        expiration: Some("2027-08-31T00:00:00Z".into()),
        quantity: 8,
        source_receiving_location: CrossDockLocation {
            location_id: 501,
            barcode: "WB-DEMO-XD-RECV-02".into(),
            name: Some("Receiving lane 02".into()),
        },
        destination_pick_face: CrossDockLocation {
            location_id: 502,
            barcode: "WB-DEMO-XD-PICK-02".into(),
            name: Some("Forward pick face 02".into()),
        },
    }
}
