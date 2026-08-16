use eframe::egui;
use lucide_icons::Icon;

use crate::picking::{
    PickClaim, PickControlledEvidence, PickExecutionEvidence, PickExecutionMethod,
    PickReleaseReason, PickScanStage, PickShortageDisposition, PickShortageReason,
};
use crate::workflow::{Activity, Transition, WorkflowEffect};

use super::RfApp;

impl RfApp {
    pub(super) fn pick_view(&mut self, ui: &mut egui::Ui) {
        if self.picking.claim().is_some() {
            self.pick_active(ui);
        } else {
            match self.picking.activity() {
                Activity::Idle => self.pick_idle(ui),
                Activity::Active => self
                    .picking
                    .require_reconciliation("Active pick is missing its claim details".into()),
                Activity::Persisting
                | Activity::ReadyToDispatch
                | Activity::InFlight
                | Activity::Ambiguous
                | Activity::ReconcileRequired => {}
            }
        }
        self.pick_command_state(ui);
        if let Some(error) = self.picking.error() {
            ui.colored_label(Self::danger(), egui::RichText::new(error).strong());
        }
    }

    fn pick_idle(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("DIRECTED PICKING")
                .small()
                .strong()
                .color(Self::accent()),
        );
        ui.label("Claim the highest-priority released pick in your facility scope.");
        ui.add_space(12.0);

        let can_claim = self.can_execute();
        let clicked = Self::full_width_button(
            ui,
            can_claim,
            egui::Button::new(egui::RichText::new("Get next pick").strong())
                .fill(Self::primary_fill(can_claim)),
            58.0,
        )
        .clicked();
        if clicked {
            let (command_id, key) = Self::command_identity("pick-claim");
            let effect = self.picking.begin_claim_next(command_id, key);
            self.emit_pick(effect);
        }

        ui.add_space(12.0);
        ui.label(egui::RichText::new("CLUSTER CART").small().strong());
        ui.horizontal(|ui| {
            ui.label("Route");
            ui.add(
                egui::TextEdit::singleline(self.picking.cluster_id_draft_mut())
                    .hint_text("Scan cluster ID"),
            );
        });
        let cluster_clicked = Self::full_width_button(
            ui,
            can_claim,
            egui::Button::new(egui::RichText::new("Start / continue cluster").strong())
                .fill(Self::primary_fill(can_claim)),
            52.0,
        )
        .clicked();
        if cluster_clicked {
            let (command_id, key) = Self::command_identity("pick-cluster-claim");
            let effect = self.picking.begin_cluster_claim(command_id, key);
            self.emit_pick(effect);
        }

        if let Some(notice) = self.picking.notice() {
            ui.add_space(12.0);
            ui.colored_label(Self::warning(), notice);
        }

        #[cfg(all(debug_assertions, not(target_os = "android")))]
        {
            if Self::show_preview_controls() {
                ui.add_space(12.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 48.0],
                        Self::secondary_button("Load preview pick", ui.available_width(), 48.0),
                    )
                    .clicked()
                {
                    self.picking.load_debug_claim(debug_pick_claim());
                }
            }
        }
    }

    fn pick_active(&mut self, ui: &mut egui::Ui) {
        let Some(claim) = self.picking.claim().cloned() else {
            return;
        };
        Self::task_reference(
            ui,
            &format!("{}  ·  Pick {}", claim.order_key, claim.task_id),
            claim.priority,
        );
        if claim.execution.method == PickExecutionMethod::ClusterCart {
            ui.group(|ui| {
                ui.label(egui::RichText::new("CLUSTER CART").small().strong());
                ui.horizontal_wrapped(|ui| {
                    ui.monospace(
                        claim
                            .execution
                            .cart_barcode
                            .as_deref()
                            .unwrap_or("Unknown cart"),
                    );
                    ui.strong(format!(
                        "Slot {}",
                        claim.execution.slot_code.as_deref().unwrap_or("?")
                    ));
                    if let (Some(sequence), Some(task_count)) =
                        (claim.execution.sequence, claim.execution.task_count)
                    {
                        ui.label(format!("Stop {sequence} of {task_count}"));
                    }
                });
            });
        } else if claim.execution.method == PickExecutionMethod::Case {
            ui.group(|ui| {
                ui.label(egui::RichText::new("CASE PICK").small().strong());
                ui.label(format!(
                    "Pick {} sealed case{} as whole handling units. Do not break case packaging.",
                    claim.content.planned_quantity,
                    if claim.content.planned_quantity == 1 {
                        ""
                    } else {
                        "s"
                    }
                ));
            });
        }

        let lease_actions_allowed = if self.picking.activity() == Activity::Active {
            self.heartbeat_status(ui, claim.task_id)
        } else {
            false
        };
        if self.picking.shortage().is_some() {
            self.pick_shortage_panel(ui, &claim, lease_actions_allowed);
        } else {
            if let Some(stage) = self.picking.expected_scan() {
                self.pick_scan_control(
                    ui,
                    claim.task_id,
                    stage,
                    pick_scan_hint(&claim, stage),
                    lease_actions_allowed,
                );
            } else if lease_actions_allowed {
                let confirm_clicked = ui
                    .add_sized(
                        [ui.available_width(), 54.0],
                        egui::Button::new(egui::RichText::new("Confirm directed pick").strong())
                            .fill(Self::primary_fill(true)),
                    )
                    .clicked();
                if confirm_clicked {
                    let (command_id, key) = Self::command_identity("pick-confirm");
                    let effect = self.picking.begin_confirmation(command_id, key);
                    self.emit_pick(effect);
                }
            }

            ui.add_space(4.0);
            Self::section_label(ui, "PICK DETAILS");
            pick_location_band(ui, "FROM", &claim.content.source_location_barcode);
            pick_content_band(ui, &claim);
            pick_location_band(ui, "TO", &claim.destination_location_barcode);

            ui.add_space(8.0);
            if self.release_confirmation {
                self.pick_release_confirmation(ui, lease_actions_allowed);
            } else {
                let width = (ui.available_width() - 8.0) / 2.0;
                ui.horizontal(|ui| {
                    let short_clicked = ui
                        .add_enabled(
                            lease_actions_allowed,
                            egui::Button::new(egui::RichText::new("Short pick").strong())
                                .fill(if lease_actions_allowed {
                                    egui::Color32::from_rgb(112, 72, 18)
                                } else {
                                    egui::Color32::from_rgb(28, 34, 32)
                                })
                                .min_size(egui::vec2(width, 48.0)),
                        )
                        .on_disabled_hover_text("Check pick connection first")
                        .clicked();
                    if short_clicked {
                        self.picking.begin_shortage();
                        self.pick_scan_focus = None;
                    }

                    let release_clicked = ui
                        .add_enabled(
                            lease_actions_allowed,
                            Self::secondary_button("Release", width, 48.0),
                        )
                        .on_disabled_hover_text("Check pick connection first")
                        .clicked();
                    if release_clicked {
                        self.release_confirmation = true;
                    }
                });
            }
        }
    }

    fn pick_shortage_panel(
        &mut self,
        ui: &mut egui::Ui,
        claim: &PickClaim,
        lease_actions_allowed: bool,
    ) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("SHORT PICK")
                    .small()
                    .strong()
                    .color(Self::warning()),
            );
            ui.weak(format!(
                "Planned {} {}",
                claim.content.planned_quantity, claim.content.uom
            ));
        });

        let Some(snapshot) = self.picking.shortage().cloned() else {
            return;
        };
        let mut reason = snapshot.reason();
        ui.add_enabled_ui(lease_actions_allowed, |ui| {
            egui::ComboBox::from_id_salt(("pick_shortage_reason", claim.task_id))
                .width(ui.available_width())
                .selected_text(reason.label())
                .show_ui(ui, |ui| {
                    for candidate in PickShortageReason::ALL {
                        ui.selectable_value(&mut reason, candidate, candidate.label());
                    }
                });
        });
        if reason != snapshot.reason() {
            self.picking.set_shortage_reason(reason);
            self.pick_scan_focus = None;
        }

        let disposition = self
            .picking
            .shortage()
            .map(|draft| draft.disposition())
            .unwrap_or(PickShortageDisposition::NoPick);
        ui.add_enabled_ui(lease_actions_allowed, |ui| {
            ui.horizontal(|ui| {
                for candidate in [
                    PickShortageDisposition::NoPick,
                    PickShortageDisposition::Partial,
                ] {
                    let enabled =
                        candidate == PickShortageDisposition::NoPick || reason.supports_partial();
                    if ui
                        .add_enabled(
                            enabled,
                            egui::Button::selectable(disposition == candidate, candidate.label()),
                        )
                        .clicked()
                    {
                        self.picking.set_shortage_disposition(candidate);
                        self.pick_scan_focus = None;
                    }
                }
            });
        });

        if reason == PickShortageReason::LotOrSerialMismatch
            && claim.content.lot.is_some()
            && claim.content.serial.is_some()
        {
            let evidence = self
                .picking
                .shortage()
                .and_then(|draft| draft.controlled_evidence());
            ui.add_enabled_ui(lease_actions_allowed, |ui| {
                ui.horizontal(|ui| {
                    ui.weak("Evidence");
                    for candidate in [PickControlledEvidence::Lot, PickControlledEvidence::Serial] {
                        if ui
                            .add(egui::Button::selectable(
                                evidence == Some(candidate),
                                candidate.label(),
                            ))
                            .clicked()
                        {
                            self.picking.set_controlled_evidence(candidate);
                            self.pick_scan_focus = None;
                        }
                    }
                });
            });
        }

        if disposition == PickShortageDisposition::Partial {
            ui.horizontal(|ui| {
                ui.label("Picked qty");
                let available = ui.available_width();
                let draft = self.picking.shortage_mut();
                if let Some(draft) = draft {
                    let response = ui.add_enabled(
                        lease_actions_allowed,
                        Self::centered_text_edit(
                            egui::TextEdit::singleline(draft.picked_quantity_mut())
                                .desired_width(available)
                                .font(egui::TextStyle::Monospace),
                        ),
                    );
                    Self::centered_hint(
                        ui,
                        &response,
                        draft.picked_quantity_mut().is_empty(),
                        "0",
                        egui::TextStyle::Monospace,
                    );
                }
            });
        }

        if let Some(stage) = self.picking.expected_scan() {
            self.pick_scan_control(
                ui,
                claim.task_id,
                stage,
                pick_scan_hint(claim, stage),
                lease_actions_allowed,
            );
        } else if let Some(draft) = self.picking.shortage() {
            ui.horizontal_wrapped(|ui| {
                if let Some(item) = draft.observed_item_barcode() {
                    ui.monospace(format!("Item {item}"));
                }
                if let Some(lot) = draft.observed_lot() {
                    ui.monospace(format!("Lot {lot}"));
                }
                if let Some(serial) = draft.observed_serial() {
                    ui.monospace(format!("Serial {serial}"));
                }
            });
        }

        ui.label(if reason == PickShortageReason::Other {
            "Note (required)"
        } else {
            "Note (optional)"
        });
        if let Some(draft) = self.picking.shortage_mut() {
            ui.add_enabled(
                lease_actions_allowed,
                egui::TextEdit::multiline(draft.note_mut())
                    .desired_width(ui.available_width())
                    .desired_rows(2)
                    .char_limit(500),
            );
        }

        let validation = self.picking.shortage_validation_message();
        if let Some(message) = validation {
            ui.weak(message);
        }
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 8.0) / 2.0;
            if ui
                .add(Self::secondary_button("Cancel", width, 48.0))
                .clicked()
            {
                self.picking.cancel_shortage();
                self.pick_scan_focus = None;
            }
            let can_report = lease_actions_allowed && validation.is_none();
            if ui
                .add_enabled(
                    can_report,
                    egui::Button::new(egui::RichText::new("Report short").strong())
                        .fill(Self::primary_fill(can_report))
                        .min_size(egui::vec2(width, 48.0)),
                )
                .on_disabled_hover_text(validation.unwrap_or("Check pick connection first"))
                .clicked()
            {
                let (command_id, key) = Self::command_identity("pick-shortage");
                let effect = self.picking.begin_shortage_report(command_id, key);
                self.emit_pick(effect);
                self.pick_scan_focus = None;
            }
        });
    }

    fn pick_scan_control(
        &mut self,
        ui: &mut egui::Ui,
        task_id: i64,
        stage: PickScanStage,
        expected: Option<&str>,
        lease_actions_allowed: bool,
    ) {
        let (response, clicked) = Self::scanner_action(
            ui,
            stage.prompt(),
            expected,
            "Confirm scan",
            lease_actions_allowed,
            self.picking.scan_draft_mut(),
            egui::Id::new(("pick_scan", task_id, stage)),
        );
        let focus_key = (task_id, stage);
        if lease_actions_allowed && self.pick_scan_focus != Some(focus_key) {
            response.request_focus();
            self.pick_scan_focus = Some(focus_key);
        } else if !lease_actions_allowed {
            self.pick_scan_focus = None;
        }

        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        if lease_actions_allowed && (enter || clicked) {
            let (command_id, key) = Self::command_identity("pick-confirm");
            let effect = self.picking.submit_scan(command_id, key);
            self.emit_pick(effect);
            self.pick_scan_focus = None;
        }
    }

    fn pick_release_confirmation(&mut self, ui: &mut egui::Ui, lease_actions_allowed: bool) {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(57, 42, 21))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.strong("Return this pick to the queue?");
                ui.label("Scanned progress will be cleared on this device.");
                ui.horizontal(|ui| {
                    if ui
                        .add(Self::secondary_button(
                            "Keep pick",
                            ui.available_width() / 2.0 - 4.0,
                            48.0,
                        ))
                        .clicked()
                    {
                        self.release_confirmation = false;
                    }
                    let clicked = ui
                        .add_enabled(
                            lease_actions_allowed,
                            egui::Button::new("Return to queue")
                                .fill(if lease_actions_allowed {
                                    egui::Color32::from_rgb(112, 72, 18)
                                } else {
                                    egui::Color32::from_rgb(28, 34, 32)
                                })
                                .min_size(egui::vec2(ui.available_width(), 48.0)),
                        )
                        .clicked();
                    if clicked && lease_actions_allowed {
                        let (command_id, key) = Self::command_identity("pick-release");
                        let effect = self.picking.begin_release(
                            command_id,
                            key,
                            PickReleaseReason::WorkInterrupted,
                            None,
                        );
                        self.emit_pick(effect);
                        self.release_confirmation = false;
                    }
                });
            });
    }

    fn pick_command_state(&mut self, ui: &mut egui::Ui) {
        match self.picking.activity() {
            Activity::Persisting => Self::state_band(
                ui,
                Self::warning(),
                Icon::Save,
                "Saving pick",
                "Do not scan again",
            ),
            Activity::ReadyToDispatch => Self::state_band(
                ui,
                Self::warning(),
                Icon::Send,
                "Pick saved",
                "Waiting for connection. Do not scan again.",
            ),
            Activity::InFlight => Self::state_band(
                ui,
                Self::warning(),
                Icon::Loader,
                "Sending pick",
                "Waiting for the server. Do not scan again.",
            ),
            Activity::Ambiguous => {
                let message = self
                    .picking
                    .ambiguous_message()
                    .unwrap_or("The pick result is unknown");
                Self::state_band(
                    ui,
                    Self::danger(),
                    Icon::AlertTriangle,
                    "Checking last pick",
                    message,
                );
                if ui
                    .add_sized(
                        [ui.available_width(), 54.0],
                        egui::Button::new(egui::RichText::new("Check again").strong())
                            .fill(egui::Color32::from_rgb(112, 72, 18)),
                    )
                    .clicked()
                {
                    let effect = self.picking.retry_ambiguous();
                    self.emit_pick(effect);
                }
            }
            Activity::ReconcileRequired => Self::state_band(
                ui,
                Self::danger(),
                Icon::ShieldAlert,
                "Picking blocked",
                self.picking
                    .reconcile_reason()
                    .unwrap_or("Device and server pick state must be reconciled"),
            ),
            Activity::Idle | Activity::Active => {}
        }
    }

    pub(super) fn emit_pick(&mut self, effect: Option<WorkflowEffect>) {
        if let Some(effect) = effect {
            self.picking_effects.push_back(effect);
        }
    }

    pub(super) fn emit_pick_transition(&mut self, transition: Transition) {
        if let Transition::Effect(effect) = transition {
            self.picking_effects.push_back(effect);
        }
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    pub(super) fn load_pick_preview(&mut self) {
        self.work_mode = super::WorkMode::Pick;
        self.workflow = crate::workflow::MovementWorkflow::default();
        self.cycle_count = crate::cycle_count::CycleCountWorkflow::default();
        let claim = debug_pick_claim();
        self.load_verified_debug_heartbeat(claim.task_id);
        self.picking.load_debug_claim(claim);
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    pub(super) fn load_pick_shortage_preview(&mut self) {
        self.load_pick_preview();
        self.picking.begin_shortage();
    }
}

fn pick_location_band(ui: &mut egui::Ui, label: &str, barcode: &str) {
    let width = ui.available_width();
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 60, 56)))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_min_width((width - 24.0).max(0.0));
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).small().strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.monospace(egui::RichText::new(barcode).size(22.0).strong());
                });
            });
        });
}

fn pick_content_band(ui: &mut egui::Ui, claim: &PickClaim) {
    let content = &claim.content;
    let width = ui.available_width();
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 60, 56)))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_min_width((width - 24.0).max(0.0));
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} {}", content.planned_quantity, content.uom))
                        .size(23.0)
                        .strong()
                        .color(RfApp::accent()),
                );
                ui.weak("|");
                ui.label(egui::RichText::new(
                    content
                        .item_description
                        .clone()
                        .unwrap_or_else(|| format!("Item {}", content.item_id)),
                ));
            });
            ui.horizontal_wrapped(|ui| {
                if let Some(lot) = content.lot.as_deref() {
                    ui.monospace(format!("Lot {lot}"));
                }
                if let Some(serial) = content.serial.as_deref() {
                    ui.monospace(format!("Serial {serial}"));
                }
                if let Some(plate) = content.source_license_plate_barcode.as_deref() {
                    ui.monospace(format!("From {plate}"));
                }
            });
        });
}

fn pick_scan_hint(claim: &PickClaim, stage: PickScanStage) -> Option<&str> {
    match stage {
        PickScanStage::SourceLocation => Some(&claim.content.source_location_barcode),
        PickScanStage::Item | PickScanStage::ObservedItem => {
            claim.content.item_barcodes.first().map(String::as_str)
        }
        PickScanStage::SourceLicensePlate => claim.content.source_license_plate_barcode.as_deref(),
        PickScanStage::ObservedLot => claim.content.lot.as_deref(),
        PickScanStage::ObservedSerial => claim.content.serial.as_deref(),
        PickScanStage::DestinationLicensePlate | PickScanStage::ShortageDestinationLicensePlate => {
            None
        }
    }
}

#[cfg(all(debug_assertions, not(target_os = "android")))]
fn debug_pick_claim() -> PickClaim {
    use crate::picking::{PickClaimContent, PickContentState};

    PickClaim {
        task_id: 410,
        order_id: 510,
        inventory_owner_id: 2,
        facility_id: 3,
        order_key: "SO-10510".into(),
        order_revision: 4,
        priority: 90,
        ship_by: Some("2026-08-09T20:00:00Z".into()),
        lease_expires_at: "2099-08-08T20:00:00Z".into(),
        destination_location_id: 9,
        destination_location_barcode: "STAGE-01".into(),
        destination_location_name: Some("Outbound stage 1".into()),
        execution: PickExecutionEvidence::discrete(),
        pick_policy: crate::picking::PickDecisionPolicy::product_default(),
        suggested_destination_license_plate_barcode: None,
        content: PickClaimContent {
            content_id: 610,
            order_line_id: 710,
            inventory_allocation_id: 810,
            source_inventory_balance_id: 910,
            item_batch_id: 1_010,
            source_location_id: 8,
            source_location_barcode: "A-01-02".into(),
            source_location_name: Some("Forward A-01-02".into()),
            source_license_plate_id: Some(12),
            source_license_plate_barcode: Some("LP-SOURCE-12".into()),
            item_id: 1_110,
            item_description: Some("High-efficiency replacement filters".into()),
            item_barcodes: vec!["CASE-1110".into()],
            uom: "case".into(),
            lot: Some("LOT-2028-08".into()),
            serial: None,
            expiration: Some("2028-08-01T00:00:00Z".into()),
            planned_quantity: 4,
            state: PickContentState::Pending,
        },
    }
}
