use eframe::egui;
use lucide_icons::Icon;

use crate::workflow::{
    Activity, InventoryRelocationClaim, Location, MovementClaimDetails, MovementKind, MovementWork,
    PutawayClaim, ReleaseReason, ScanStage,
};

use super::{RfApp, WorkMode, action_requested};

impl RfApp {
    pub(super) fn movement_idle(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("WORK TYPE").small().strong());
        let segment_width = (ui.available_width() - 8.0) / 2.0;
        ui.horizontal(|ui| {
            for kind in [MovementKind::Loose, MovementKind::LicensePlate] {
                let selected = self.workflow.selected_kind() == kind;
                if ui
                    .add_sized(
                        [segment_width, 52.0],
                        egui::Button::selectable(selected, kind.label()),
                    )
                    .clicked()
                {
                    self.workflow.select_kind(kind);
                }
            }
        });
        ui.add_space(10.0);

        let can_execute = self.can_execute();
        let button = egui::Button::new(egui::RichText::new("Get next task").strong())
            .fill(Self::primary_fill(can_execute));
        if Self::full_width_button(ui, can_execute, button, 58.0).clicked() {
            let (command_id, key) = Self::command_identity("claim");
            let effect = self.workflow.begin_claim_next(command_id, key);
            self.emit(effect);
        }

        if let Some(error) = self.storage_error.as_deref() {
            ui.add_space(12.0);
            ui.colored_label(Self::danger(), error);
        }

        #[cfg(debug_assertions)]
        {
            if Self::show_preview_controls() {
                ui.add_space(12.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 48.0],
                        egui::Button::new("Load preview task")
                            .fill(ui.visuals().faint_bg_color)
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(79, 91, 87))),
                    )
                    .clicked()
                {
                    self.workflow.load_debug_claim(Self::debug_claim());
                }
            }
        }

        if let Some(notice) = self.workflow.notice() {
            ui.add_space(12.0);
            ui.colored_label(Self::warning(), notice);
        }
    }

    pub(super) fn active_movement(&mut self, ui: &mut egui::Ui) {
        let Some(claim) = self.workflow.claim().cloned() else {
            return;
        };

        Self::task_reference(ui, &format!("Task {}", claim.task_id), claim.priority);

        if let Some(instructions) = claim.instructions.as_deref() {
            Self::message_band(ui, Self::warning(), Icon::AlertTriangle, instructions);
        }

        let lease_actions_allowed = if self.workflow.activity() == Activity::Active {
            self.heartbeat_status(ui, claim.task_id)
        } else {
            false
        };
        if let Some(stage) = self.workflow.expected_scan() {
            let expected = match stage {
                ScanStage::Source => claim
                    .source
                    .as_ref()
                    .and_then(|location| location.barcode.as_deref()),
                ScanStage::LicensePlate => match &claim.work {
                    MovementWork::LicensePlate { barcode, .. } => Some(barcode.as_str()),
                    MovementWork::Loose { .. } => None,
                },
                ScanStage::Destination => claim.destination.barcode.as_deref(),
            };
            self.movement_scan_control(ui, claim.task_id, stage, expected, lease_actions_allowed);
        }

        ui.add_space(4.0);
        Self::section_label(ui, "MOVE DETAILS");
        if let Some(source) = &claim.source {
            Self::movement_location_band(ui, "FROM", source);
        }
        Self::movement_work_band(ui, &claim.work);
        Self::movement_location_band(ui, "TO", &claim.destination);

        ui.add_space(8.0);
        if self.release_confirmation {
            self.movement_release_confirmation(ui, lease_actions_allowed);
        } else {
            let release_clicked = ui
                .add_enabled(
                    lease_actions_allowed,
                    Self::secondary_button("Release work", ui.available_width(), 48.0),
                )
                .on_disabled_hover_text("Check task connection first")
                .clicked();
            if self.workflow.activity() == Activity::Active
                && action_requested(lease_actions_allowed, release_clicked)
            {
                self.release_confirmation = true;
            }
        }
    }

    fn movement_location_band(ui: &mut egui::Ui, label: &str, location: &Location) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 60, 56)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(0.0));
                ui.label(egui::RichText::new(label).small().strong());
                let display_name = location.display_name();
                ui.label(egui::RichText::new(&display_name).size(23.0).strong());
                if let Some(barcode) = location
                    .barcode
                    .as_deref()
                    .filter(|barcode| *barcode != display_name)
                {
                    ui.monospace(barcode);
                }
            });
    }

    fn movement_work_band(ui: &mut egui::Ui, work: &MovementWork) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 60, 56)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(0.0));
                match work {
                    MovementWork::Loose {
                        item_description,
                        item_id,
                        quantity,
                        uom,
                        lot,
                        serial,
                    } => {
                        ui.label(egui::RichText::new("LOOSE INVENTORY").small().strong());
                        ui.label(
                            egui::RichText::new(
                                item_description
                                    .clone()
                                    .unwrap_or_else(|| format!("Item {item_id}")),
                            )
                            .size(21.0)
                            .strong(),
                        );
                        ui.label(format!("{quantity} {uom}"));
                        if let Some(lot) = lot {
                            ui.monospace(format!("Lot {lot}"));
                        }
                        if let Some(serial) = serial {
                            ui.monospace(format!("Serial {serial}"));
                        }
                    }
                    MovementWork::LicensePlate {
                        barcode,
                        planned_balance_count,
                    } => {
                        ui.label(egui::RichText::new("LICENSE PLATE").small().strong());
                        ui.monospace(egui::RichText::new(barcode).size(23.0).strong());
                        ui.label(format!("{planned_balance_count} inventory balances"));
                    }
                }
            });
    }

    fn movement_scan_control(
        &mut self,
        ui: &mut egui::Ui,
        task_id: i64,
        stage: ScanStage,
        expected: Option<&str>,
        lease_actions_allowed: bool,
    ) {
        let (response, clicked) = Self::scanner_action(
            ui,
            stage.prompt(),
            expected,
            "Confirm scan",
            lease_actions_allowed,
            self.workflow.scan_draft_mut(),
            egui::Id::new(("putaway_scan", task_id, stage)),
        );
        let focus_key = (task_id, stage);
        if lease_actions_allowed && self.scan_focus != Some(focus_key) {
            response.request_focus();
            self.scan_focus = Some(focus_key);
        } else if !lease_actions_allowed {
            self.scan_focus = None;
        }
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        if action_requested(lease_actions_allowed, enter || clicked) {
            let (command_id, key) = Self::command_identity("confirm");
            let effect = self.workflow.submit_scan(command_id, key);
            self.emit(effect);
        }
    }

    fn movement_release_confirmation(&mut self, ui: &mut egui::Ui, lease_actions_allowed: bool) {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(57, 42, 21))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.strong("Return this task to the queue?");
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
                    let release_clicked = ui
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
                        .on_disabled_hover_text("Check task connection first")
                        .clicked();
                    if action_requested(lease_actions_allowed, release_clicked) {
                        let (command_id, key) = Self::command_identity("release");
                        let effect = self.workflow.begin_release(
                            command_id,
                            key,
                            ReleaseReason::WorkInterrupted,
                            None,
                        );
                        self.emit(effect);
                        self.release_confirmation = false;
                    }
                });
            });
    }

    pub(super) fn movement_command_state(&mut self, ui: &mut egui::Ui) {
        match self.workflow.activity() {
            Activity::Persisting => Self::state_band(
                ui,
                Self::warning(),
                Icon::Save,
                "Saving scan",
                "Do not scan again",
            ),
            Activity::ReadyToDispatch => Self::state_band(
                ui,
                Self::warning(),
                Icon::Send,
                "Scan saved",
                "Waiting for connection. Do not scan again.",
            ),
            Activity::InFlight => Self::state_band(
                ui,
                Self::warning(),
                Icon::Loader,
                "Sending scan",
                "Waiting for the server. Do not scan again.",
            ),
            Activity::Ambiguous => {
                let message = self
                    .workflow
                    .ambiguous_message()
                    .unwrap_or("The command result is unknown");
                Self::state_band(
                    ui,
                    Self::danger(),
                    Icon::AlertTriangle,
                    "Checking last scan",
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
                    let effect = self.workflow.retry_ambiguous();
                    self.emit(effect);
                }
            }
            Activity::ReconcileRequired => Self::state_band(
                ui,
                Self::danger(),
                Icon::ShieldAlert,
                "Work blocked",
                self.workflow
                    .reconcile_reason()
                    .unwrap_or("Device and server state must be reconciled"),
            ),
            Activity::Idle | Activity::Active => {}
        }
    }

    pub(super) fn movement_error(&self, ui: &mut egui::Ui) {
        if let Some(error) = self.workflow.error() {
            ui.colored_label(Self::danger(), egui::RichText::new(error).strong());
        }
    }

    #[cfg(debug_assertions)]
    pub(super) fn debug_claim() -> PutawayClaim {
        PutawayClaim::new(MovementClaimDetails {
            task_id: 1042,
            inventory_owner_id: 12,
            facility_id: 4,
            priority: 80,
            instructions: Some("Keep upright".into()),
            lease_expires_at: (chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339(),
            source: Some(Location {
                location_id: 17,
                name: Some("Receiving 01".into()),
                barcode: Some("RECV-01".into()),
            }),
            destination: Location {
                location_id: 31,
                name: Some("A-01-03".into()),
                barcode: Some("A-01-03".into()),
            },
            work: MovementWork::Loose {
                item_description: Some("Case-picked item".into()),
                item_id: 88,
                quantity: 4,
                uom: "cases".into(),
                lot: Some("LOT-2407".into()),
                serial: None,
            },
        })
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    pub(super) fn open_debug_relocation_preview(&mut self, kind: MovementKind) {
        let work = match kind {
            MovementKind::Loose => MovementWork::Loose {
                item_description: Some("Case-picked item".into()),
                item_id: 88,
                quantity: 4,
                uom: "cases".into(),
                lot: Some("LOT-2407".into()),
                serial: None,
            },
            MovementKind::LicensePlate => MovementWork::LicensePlate {
                barcode: "LP-0001042".into(),
                planned_balance_count: 3,
            },
        };
        self.work_mode = WorkMode::Relocate;
        self.workflow
            .load_debug_relocation_claim(InventoryRelocationClaim::new(MovementClaimDetails {
                task_id: 2042,
                inventory_owner_id: 12,
                facility_id: 4,
                priority: 70,
                instructions: Some("Keep aisle crossing clear".into()),
                lease_expires_at: (chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339(),
                source: Some(Location {
                    location_id: 31,
                    name: Some("A-01-03".into()),
                    barcode: Some("A-01-03".into()),
                }),
                destination: Location {
                    location_id: 47,
                    name: Some("B-02-01".into()),
                    barcode: Some("B-02-01".into()),
                },
                work,
            }));
    }
}
