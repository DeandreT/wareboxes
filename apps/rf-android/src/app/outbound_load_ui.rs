use eframe::egui;
use lucide_icons::Icon;
use wareboxes_api_contract::v1::PackedCartonPositionStateResponse;

use crate::outbound_load::{OutboundCartonOperation, OutboundLoadScanStage};
use crate::workflow::{Activity, Transition};

use super::RfApp;

impl RfApp {
    pub(super) fn outbound_load_view(&mut self, ui: &mut egui::Ui) {
        self.outbound_load_command_state(ui);
        if let Some(error) = self.outbound_load.error() {
            ui.add_space(8.0);
            Self::message_band(ui, Self::danger(), Icon::ScanLine, error);
        }
        if let Some(notice) = self.outbound_load.notice() {
            ui.add_space(8.0);
            Self::message_band(ui, Self::accent(), Icon::CheckCircle, notice);
        }
        if self.outbound_load.load().is_some() {
            self.outbound_load_active(ui);
        } else if self.outbound_load.activity() == Activity::Idle {
            self.outbound_load_idle(ui);
        }
    }

    fn outbound_load_idle(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("SCAN OUTBOUND LOAD").small().strong());
        let enabled = self.can_execute() && self.expected_outbound_load_request_id.is_none();
        let input = ui
            .add_enabled_ui(enabled, |ui| {
                ui.add_sized(
                    [ui.available_width(), 58.0],
                    egui::TextEdit::singleline(&mut self.outbound_load_barcode_draft)
                        .id(egui::Id::new("outbound_load_barcode"))
                        .font(egui::TextStyle::Monospace)
                        .hint_text("LOAD BARCODE"),
                )
            })
            .inner;
        if enabled && self.field_focus_pending {
            input.request_focus();
            self.field_focus_pending = false;
        }
        let submit = input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let ready = !self.outbound_load_barcode_draft.trim().is_empty();
        if ui
            .add_enabled(
                enabled && ready,
                egui::Button::new(egui::RichText::new("Open load").strong())
                    .fill(Self::primary_fill(enabled && ready))
                    .min_size(egui::vec2(ui.available_width(), 56.0)),
            )
            .clicked()
            || (enabled && ready && submit)
        {
            self.begin_outbound_load_lookup(ui.ctx());
        }
        if self.expected_outbound_load_request_id.is_some() {
            ui.add_space(8.0);
            Self::state_band(
                ui,
                Self::warning(),
                Icon::Loader,
                "Opening load",
                "Waiting for authoritative carton state",
            );
        }
    }

    fn outbound_load_active(&mut self, ui: &mut egui::Ui) {
        let Some(load) = self.outbound_load.load().cloned() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(&load.load_reference)
                    .size(20.0)
                    .strong()
                    .color(Self::accent()),
            );
            ui.separator();
            ui.monospace(&load.load_barcode);
            ui.separator();
            ui.label(format!("{:?}", load.status));
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} / {} loaded",
                load.progress.loaded_carton_count, load.progress.planned_carton_count
            ));
            ui.separator();
            ui.label(format!("{} staged", load.progress.staged_carton_count));
            ui.separator();
            ui.label(format!("Rev {}", load.revision.get()));
        });
        ui.add_space(6.0);
        let selected = self.outbound_load.operation();
        let can_change = self.outbound_load.activity() == Activity::Active;
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 24.0) / 4.0;
            for operation in OutboundCartonOperation::ALL {
                let allowed = self.outbound_load.operation_allowed(operation);
                if ui
                    .add_enabled(
                        can_change && allowed,
                        egui::Button::selectable(selected == operation, operation.label())
                            .min_size(egui::vec2(width, 42.0)),
                    )
                    .clicked()
                {
                    self.outbound_load.select_operation(operation);
                    self.outbound_load_scan_focus = None;
                }
            }
        });
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("EXECUTION").small().strong());
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("Lane {}", load.staging_location_name));
                    if let Some(trailer) = load.trailer_number.as_deref() {
                        ui.separator();
                        ui.monospace(format!("Trailer {trailer}"));
                    }
                });
                let next = load
                    .cartons
                    .iter()
                    .filter(|carton| match selected {
                        OutboundCartonOperation::Stage => matches!(
                            carton.state,
                            PackedCartonPositionStateResponse::Packed { .. }
                        ),
                        OutboundCartonOperation::Load | OutboundCartonOperation::Unstage => {
                            matches!(
                                carton.state,
                                PackedCartonPositionStateResponse::Staged { .. }
                            )
                        }
                        OutboundCartonOperation::Unload => matches!(
                            carton.state,
                            PackedCartonPositionStateResponse::Loaded { .. }
                        ),
                    })
                    .take(4)
                    .map(|carton| format!("{}:{}", carton.load_sequence, carton.carton_barcode))
                    .collect::<Vec<_>>();
                if !next.is_empty() {
                    ui.monospace(next.join("  "));
                }
            });
        if let Some(stage) = self.outbound_load.expected_scan() {
            self.outbound_load_scan_control(ui, load.outbound_load_id, stage);
        } else if self.outbound_load.activity() == Activity::Active
            && ui
                .add(
                    egui::Button::new(
                        egui::RichText::new(format!("Confirm {}", selected.label().to_lowercase()))
                            .strong(),
                    )
                    .fill(Self::primary_fill(true))
                    .min_size(egui::vec2(ui.available_width(), 58.0)),
                )
                .clicked()
        {
            let (command_id, key) = Self::command_identity("outbound-carton-move");
            let transition = self.outbound_load.begin_movement(command_id, key);
            self.emit_outbound_load_transition(transition);
        }
        if self.outbound_load.activity() == Activity::Active {
            ui.add_space(8.0);
            if ui
                .add(Self::secondary_button(
                    "Close load",
                    ui.available_width(),
                    46.0,
                ))
                .clicked()
            {
                self.outbound_load.clear_load();
                self.outbound_load_barcode_draft.clear();
                self.field_focus_pending = true;
            }
        }
    }

    fn outbound_load_scan_control(
        &mut self,
        ui: &mut egui::Ui,
        load_id: i64,
        stage: OutboundLoadScanStage,
    ) {
        let operation = self.outbound_load.operation();
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(stage.prompt(operation))
                .size(19.0)
                .strong()
                .color(Self::accent()),
        );
        let response = ui.add_sized(
            [ui.available_width(), 56.0],
            egui::TextEdit::singleline(self.outbound_load.scan_draft_mut())
                .id(egui::Id::new(("outbound_load_scan", load_id, stage)))
                .font(egui::TextStyle::Monospace)
                .hint_text("SCAN"),
        );
        let focus = (load_id, stage);
        if self.outbound_load_scan_focus != Some(focus) {
            response.request_focus();
            self.outbound_load_scan_focus = Some(focus);
        }
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let ready = !self.outbound_load.scan_draft_mut().trim().is_empty();
        if ui
            .add_enabled(
                ready,
                egui::Button::new("Confirm scan")
                    .fill(Self::primary_fill(ready))
                    .min_size(egui::vec2(ui.available_width(), 52.0)),
            )
            .clicked()
            || (ready && enter)
        {
            self.outbound_load.submit_scan();
            self.outbound_load_scan_focus = None;
        }
    }

    fn outbound_load_command_state(&mut self, ui: &mut egui::Ui) {
        match self.outbound_load.activity() {
            Activity::Persisting => Self::state_band(
                ui,
                Self::warning(),
                Icon::Save,
                "Saving carton move",
                "Do not scan again",
            ),
            Activity::ReadyToDispatch | Activity::InFlight => Self::state_band(
                ui,
                Self::warning(),
                Icon::Wifi,
                "Sending carton move",
                "Inventory is locked until the result is saved",
            ),
            Activity::Ambiguous => {
                let message = self
                    .outbound_load
                    .ambiguous_message()
                    .unwrap_or("The server result is unknown")
                    .to_owned();
                Self::state_band(
                    ui,
                    Self::danger(),
                    Icon::WifiOff,
                    "Check saved carton move",
                    &message,
                );
                if ui
                    .add(
                        egui::Button::new("Retry exact saved move")
                            .fill(Self::primary_fill(true))
                            .min_size(egui::vec2(ui.available_width(), 54.0)),
                    )
                    .clicked()
                {
                    let transition = self.outbound_load.retry_ambiguous();
                    self.emit_outbound_load_transition(transition);
                }
            }
            Activity::ReconcileRequired => Self::state_band(
                ui,
                Self::danger(),
                Icon::ShieldAlert,
                "Carton move blocked",
                self.outbound_load
                    .reconcile_reason()
                    .unwrap_or("Supervisor review is required"),
            ),
            Activity::Idle | Activity::Active => {}
        }
    }

    pub(super) fn emit_outbound_load_transition(&mut self, transition: Transition) {
        if let Transition::Effect(effect) = transition {
            self.outbound_load_effects.push_back(effect);
        }
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    pub(super) fn load_outbound_load_preview(&mut self) {
        self.work_mode = super::WorkMode::OutboundLoad;
        self.workflow = crate::workflow::MovementWorkflow::default();
        self.outbound_load = crate::outbound_load::OutboundLoadWorkflow::default();
        self.outbound_load
            .resolve_load(crate::outbound_load::example_outbound_load());
        self.outbound_load_scan_focus = None;
    }
}
