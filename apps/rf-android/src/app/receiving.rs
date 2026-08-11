use eframe::egui;
use lucide_icons::Icon;

use crate::expected_receiving::{
    ActionBlockReason, ActionGuard, CommandAccess, CommandAccessBlock, ConfirmationMode,
    ContainerCapture, ExceptionNote, ExpectedReceiptLine, ExpectedReceivingReducer, FocusTarget,
    LoadLineId, PositiveQuantity, ReceiptExceptionReason, ReceivingActivity, ReceivingLoadStatus,
    ReceivingOperatorError, ReceivingTransition, ReconciliationReason, ScannerTarget,
    UnexpectedReceiptReason,
};
#[cfg(all(debug_assertions, not(target_os = "android")))]
use crate::expected_receiving::{
    ExpectedReceiptLineInput, FacilityId, InventoryOwnerId, ItemBarcode, ItemId, LoadId,
    LoadResolutionFailure, LocationId, NonNegativeQuantity, ReceivingDock, ReceivingEffect,
    ReceivingSession, ReceivingSessionInput, SealBarcode, StockDimension,
};
use crate::workflow::MovementKind;

use super::RfApp;
use super::SessionGate;
use super::navigation::WorkMode;
#[cfg(test)]
use super::navigation::work_mode_switch_allowed;
use super::session::ReceivingCommandPhase;

mod controls;
mod saved;
#[cfg(test)]
mod tests;

use controls::{exception_reason_label, unexpected_reason_label};

pub(super) struct ReceivingUiState {
    scan_draft: String,
    quantity_draft: String,
    note_draft: String,
    mode: ConfirmationMode,
    disposition_menu_open: bool,
    container: ContainerCapture,
    reason: Option<ReceiptExceptionReason>,
    unexpected_reason: Option<UnexpectedReceiptReason>,
    focus: Option<FocusTarget>,
    selected_line_id: Option<LoadLineId>,
    viewport_width: Option<f32>,
    synced_draft: Option<ReceivingDraftSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReceivingDraftSnapshot {
    mode: ConfirmationMode,
    selected_line_id: Option<LoadLineId>,
    item_barcode: Option<String>,
    dock_barcode: Option<String>,
    quantity: Option<i64>,
    container: ContainerCapture,
    license_plate_barcode: Option<String>,
    reason: Option<ReceiptExceptionReason>,
    unexpected_reason: Option<UnexpectedReceiptReason>,
    note: Option<String>,
}

impl Default for ReceivingUiState {
    fn default() -> Self {
        Self {
            scan_draft: String::new(),
            quantity_draft: "1".into(),
            note_draft: String::new(),
            mode: ConfirmationMode::Received,
            disposition_menu_open: false,
            container: ContainerCapture::Loose,
            reason: None,
            unexpected_reason: None,
            focus: None,
            selected_line_id: None,
            viewport_width: None,
            synced_draft: None,
        }
    }
}

impl ReceivingUiState {
    pub(super) fn clear_focus(&mut self) {
        self.focus = None;
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    fn reset_confirmation(&mut self) {
        self.scan_draft.clear();
        self.quantity_draft = "1".into();
        self.note_draft.clear();
        self.mode = ConfirmationMode::Received;
        self.disposition_menu_open = false;
        self.container = ContainerCapture::Loose;
        self.reason = None;
        self.unexpected_reason = None;
        self.focus = None;
        self.selected_line_id = None;
        self.viewport_width = None;
        self.synced_draft = None;
    }

    fn sync_from_snapshot(&mut self, snapshot: Option<ReceivingDraftSnapshot>) {
        if snapshot == self.synced_draft {
            return;
        }
        if let Some(draft) = snapshot.as_ref() {
            let selected_changed = draft.selected_line_id != self.selected_line_id;
            self.mode = draft.mode;
            self.selected_line_id = draft.selected_line_id;
            self.quantity_draft = draft
                .quantity
                .map_or_else(String::new, |quantity| quantity.to_string());
            self.container = draft.container;
            self.reason = draft.reason;
            self.unexpected_reason = draft.unexpected_reason;
            self.note_draft = draft.note.clone().unwrap_or_default();
            if selected_changed {
                self.focus = None;
            }
        }
        self.synced_draft = snapshot;
    }

    fn displayed_confirmation_matches(&self, draft: &ReceivingDraftSnapshot) -> bool {
        let quantity_matches = self.quantity_draft.parse::<i64>().ok() == draft.quantity
            && draft.quantity.is_some_and(|quantity| quantity > 0);
        let controls_match = self.mode == draft.mode
            && self.container == draft.container
            && self.reason == draft.reason
            && self.unexpected_reason == draft.unexpected_reason;
        let note_required = draft.reason == Some(ReceiptExceptionReason::Other)
            || draft.unexpected_reason == Some(UnexpectedReceiptReason::Other);
        let note_matches = if note_required {
            ExceptionNote::new(self.note_draft.clone()).is_ok()
                && draft.note.as_deref() == Some(self.note_draft.as_str())
        } else {
            true
        };
        quantity_matches && controls_match && note_matches
    }
}

impl RfApp {
    pub(super) fn receiving_view(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        if self
            .receiving_ui
            .viewport_width
            .is_some_and(|previous| (previous - width).abs() > 1.0)
        {
            self.receiving_ui.focus = None;
        }
        self.receiving_ui.viewport_width = Some(width);
        self.sync_receiving_ui();
        match self.receiving.activity() {
            ReceivingActivity::AwaitingLoad => {
                self.receiving_load_scan(ui, false);
                self.receiving_operator_error(ui);
            }
            ReceivingActivity::ResolvingLoad => Self::state_band(
                ui,
                Self::warning(),
                Icon::Loader,
                "Finding load",
                "Waiting for the warehouse service",
            ),
            ReceivingActivity::LoadResolutionFailed => self.receiving_load_error(ui),
            ReceivingActivity::Active => self.receiving_active(ui),
            ReceivingActivity::ConfirmationPending => self.receiving_confirmation_pending(ui),
            ReceivingActivity::Refreshing => {
                self.receiving_session_summary(ui);
                Self::state_band(
                    ui,
                    Self::warning(),
                    Icon::RefreshCw,
                    "Updating load",
                    "Receipt saved. Checking remaining work.",
                );
            }
            ReceivingActivity::RefreshFailed => self.receiving_refresh_failed(ui),
            ReceivingActivity::LoadComplete => self.receiving_complete(ui),
            ReceivingActivity::ReconcileRequired => self.receiving_reconciliation(ui),
        }
    }

    fn receiving_load_scan(&mut self, ui: &mut egui::Ui, next_load: bool) {
        let prompt = if next_load {
            "Scan next inbound load"
        } else {
            "Scan inbound load"
        };
        let (response, clicked) = Self::scanner_action(
            ui,
            prompt,
            None,
            "Open load",
            true,
            &mut self.receiving_ui.scan_draft,
            egui::Id::new("receiving_load_scan"),
        );
        self.request_receiving_focus(&response, FocusTarget::Scanner(ScannerTarget::LoadBarcode));

        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let has_scan = !self.receiving_ui.scan_draft.trim().is_empty();
        if has_scan && (enter || clicked) {
            self.submit_receiving_load();
        }
    }

    fn receiving_load_error(&mut self, ui: &mut egui::Ui) {
        Self::state_band(
            ui,
            Self::danger(),
            Icon::AlertTriangle,
            "Load not opened",
            receiving_error_message(self.receiving.operator_error()),
        );
        ui.add_space(8.0);
        if ui
            .add_sized(
                [ui.available_width(), 56.0],
                egui::Button::new(egui::RichText::new("Try again").strong())
                    .fill(egui::Color32::from_rgb(112, 72, 18)),
            )
            .clicked()
        {
            let transition = self.receiving.retry_load_resolution();
            self.emit_receiving_transition(transition);
        }
        ui.add_space(8.0);
        self.receiving_load_scan(ui, false);
    }

    fn receiving_active(&mut self, ui: &mut egui::Ui) {
        if self
            .receiving
            .session()
            .is_some_and(|session| session.status() == ReceivingLoadStatus::Arrived)
        {
            self.receiving_unloading(ui);
            return;
        }
        self.receiving_session_summary(ui);
        self.receiving_disposition(ui);

        if self.receiving_ui.mode == ConfirmationMode::Unexpected {
            self.receiving_unexpected_evidence(ui);
        }

        let target = self.receiving.focus_target();
        if target == FocusTarget::Scanner(ScannerTarget::ItemBarcode) {
            if self.receiving_ui.mode == ConfirmationMode::Missing {
                self.receiving_open_lines(ui, true);
            } else {
                self.receiving_scan_control(ui, ScannerTarget::ItemBarcode);
                if matches!(
                    self.receiving.operator_error(),
                    Some(ReceivingOperatorError::ItemMatchesMultipleLines { .. })
                ) {
                    self.receiving_open_lines(ui, true);
                }
            }
        }

        if let Some(line) = self.receiving.selected_line() {
            Self::selected_line_band(ui, line);
            if matches!(
                self.receiving_ui.mode,
                ConfirmationMode::Received | ConfirmationMode::Quarantined
            ) {
                self.receiving_container_control(ui);
            }
        } else if self.receiving_ui.mode == ConfirmationMode::Unexpected
            && receiving_draft_snapshot(&self.receiving)
                .is_some_and(|draft| draft.item_barcode.is_some())
        {
            self.receiving_container_control(ui);
        }

        match target {
            FocusTarget::Scanner(ScannerTarget::DockBarcode) => {
                self.receiving_scan_control(ui, ScannerTarget::DockBarcode);
            }
            FocusTarget::Scanner(ScannerTarget::LicensePlateBarcode) => {
                self.receiving_scan_control(ui, ScannerTarget::LicensePlateBarcode);
            }
            FocusTarget::Quantity
            | FocusTarget::ExceptionReason
            | FocusTarget::ExceptionNote
            | FocusTarget::ConfirmAction => {
                self.receiving_completion_panel(ui);
            }
            FocusTarget::Scanner(ScannerTarget::LoadBarcode | ScannerTarget::ItemBarcode)
            | FocusTarget::Scanner(ScannerTarget::SealBarcode)
            | FocusTarget::Blocked(_) => {}
        }
        self.receiving_operator_error(ui);
    }

    fn receiving_unloading(&mut self, ui: &mut egui::Ui) {
        self.receiving_session_summary(ui);
        Self::state_band(
            ui,
            Self::warning(),
            Icon::PackageOpen,
            "Verify unloading",
            "Scan the assigned dock and planned seal before receiving inventory.",
        );
        ui.add_space(8.0);
        match self.receiving.focus_target() {
            FocusTarget::Scanner(ScannerTarget::DockBarcode) => {
                self.receiving_scan_control(ui, ScannerTarget::DockBarcode);
            }
            FocusTarget::Scanner(ScannerTarget::SealBarcode) => {
                self.receiving_scan_control(ui, ScannerTarget::SealBarcode);
            }
            FocusTarget::ConfirmAction => {
                let guard = self
                    .receiving
                    .unloading_start_guard(self.receiving_command_access());
                ui.label(action_guard_message(guard));
                let enabled = guard == ActionGuard::Allowed;
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(egui::RichText::new("Start unloading").strong())
                            .min_size(egui::vec2(ui.available_width(), 56.0))
                            .fill(Self::accent()),
                    )
                    .clicked()
                {
                    let access = self.receiving_command_access();
                    let transition = self.receiving.begin_unloading_start(access);
                    self.emit_receiving_transition(transition);
                }
            }
            FocusTarget::Scanner(
                ScannerTarget::LoadBarcode
                | ScannerTarget::ItemBarcode
                | ScannerTarget::LicensePlateBarcode,
            )
            | FocusTarget::Blocked(_)
            | FocusTarget::Quantity
            | FocusTarget::ExceptionReason
            | FocusTarget::ExceptionNote => {}
        }
        self.receiving_operator_error(ui);
    }

    fn receiving_session_summary(&self, ui: &mut egui::Ui) {
        let Some(session) = self.receiving.session() else {
            return;
        };
        let reference = session.reference_number().unwrap_or("Inbound load");
        egui::containers::Sides::new().height(34.0).show(
            ui,
            |ui| {
                ui.label(egui::RichText::new(reference).size(20.0).strong());
            },
            |ui| {
                ui.label(format!("{} open", session.lines().len()));
            },
        );
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("RECEIVE AT").small().strong());
            ui.monospace(
                egui::RichText::new(session.dock().barcode().as_str())
                    .strong()
                    .color(Self::accent()),
            );
        });
        if let Some(last) = self.receiving.last_confirmation() {
            ui.label(
                egui::RichText::new(format!(
                    "Last: {} {}",
                    last.quantity.get(),
                    confirmation_mode_label(last.disposition)
                ))
                .small()
                .color(Self::accent()),
            );
        }
        ui.add_space(2.0);
    }

    fn receiving_scan_control(&mut self, ui: &mut egui::Ui, target: ScannerTarget) {
        let prompt = if target == ScannerTarget::ItemBarcode
            && self.receiving_ui.mode == ConfirmationMode::Unexpected
        {
            "Scan unexpected item"
        } else {
            scanner_prompt(target)
        };
        let expected = match target {
            ScannerTarget::DockBarcode => self
                .receiving
                .session()
                .map(|session| session.dock().barcode().as_str().to_owned()),
            ScannerTarget::SealBarcode => self
                .receiving
                .session()
                .and_then(|session| session.expected_seal())
                .map(|seal| seal.as_str().to_owned()),
            ScannerTarget::LoadBarcode
            | ScannerTarget::ItemBarcode
            | ScannerTarget::LicensePlateBarcode => None,
        };
        let (response, clicked) = Self::scanner_action(
            ui,
            prompt,
            expected.as_deref(),
            "Confirm scan",
            true,
            &mut self.receiving_ui.scan_draft,
            egui::Id::new(("receiving_scan", scanner_hint(target))),
        );
        self.request_receiving_focus(&response, FocusTarget::Scanner(target));
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let has_scan = !self.receiving_ui.scan_draft.trim().is_empty();
        if has_scan && (enter || clicked) {
            self.submit_receiving_scan();
        }
    }

    fn receiving_completion_panel(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(Self::accent().gamma_multiply(0.08))
            .stroke(egui::Stroke::new(1.0, Self::accent()))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(0.0));
                Self::section_label(ui, "NEXT ACTION");
                ui.label(
                    egui::RichText::new("Review and confirm")
                        .size(23.0)
                        .strong()
                        .color(egui::Color32::WHITE),
                );
                self.receiving_quantity(ui);
                if matches!(
                    self.receiving_ui.mode,
                    ConfirmationMode::Quarantined
                        | ConfirmationMode::Rejected
                        | ConfirmationMode::Missing
                ) {
                    self.receiving_exception(ui);
                } else if self.receiving_ui.mode == ConfirmationMode::Unexpected {
                    self.receiving_unexpected_reason(ui);
                }
                self.receiving_confirm(ui);
            });
    }

    fn receiving_open_lines(&mut self, ui: &mut egui::Ui, selectable: bool) {
        let Some(lines) = self
            .receiving
            .session()
            .map(|session| session.lines().to_vec())
        else {
            return;
        };
        ui.add_space(4.0);
        ui.label(egui::RichText::new("OPEN LINES").small().strong());
        for line in lines {
            let width = ui.available_width();
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 60, 56)))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_min_width((width - 20.0).max(0.0));
                    egui::containers::Sides::new().show(
                        ui,
                        |ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(line.item_description().map_or_else(
                                        || format!("Item {}", line.item_id().get()),
                                        str::to_owned,
                                    ))
                                    .strong(),
                                );
                                ui.label(format!(
                                    "{} {} remaining",
                                    line.remaining().get(),
                                    line.uom().as_str()
                                ));
                            });
                        },
                        |ui| {
                            if selectable
                                && ui
                                    .add_sized([78.0, 48.0], egui::Button::new("Select"))
                                    .clicked()
                            {
                                let transition = self.receiving.select_line(line.load_line_id());
                                self.emit_receiving_transition(transition);
                                let transition = self.receiving.select_mode(self.receiving_ui.mode);
                                self.emit_receiving_transition(transition);
                                let transition = self.receiving.set_quantity(1);
                                self.emit_receiving_transition(transition);
                                self.receiving_ui.focus = None;
                            }
                        },
                    );
                });
        }
    }

    fn selected_line_band(ui: &mut egui::Ui, line: &ExpectedReceiptLine) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 60, 56)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_min_width((width - 20.0).max(0.0));
                ui.label(egui::RichText::new("SELECTED ITEM").small().strong());
                ui.label(
                    egui::RichText::new(
                        line.item_description().map_or_else(
                            || format!("Item {}", line.item_id().get()),
                            str::to_owned,
                        ),
                    )
                    .size(20.0)
                    .strong(),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!(
                        "{} / {} {} left",
                        line.remaining().get(),
                        line.expected().get(),
                        line.uom().as_str()
                    ));
                    if let Some(lot) = line.lot() {
                        ui.separator();
                        ui.monospace(format!("Lot {}", lot.as_str()));
                    }
                    if let Some(serial) = line.serial() {
                        ui.separator();
                        ui.monospace(format!("Serial {}", serial.as_str()));
                    }
                    if let Some(expiration) = line.expiration() {
                        ui.separator();
                        ui.monospace(format!("Exp {}", expiration.as_str()));
                    }
                });
            });
    }

    fn receiving_confirmation_pending(&mut self, ui: &mut egui::Ui) {
        self.receiving_session_summary(ui);
        let unloading = self
            .receiving
            .session()
            .is_some_and(|session| session.status() == ReceivingLoadStatus::Arrived);
        if !unloading {
            self.receiving_saved_draft(ui);
        }
        let phase = self
            .receiving_command
            .as_ref()
            .map_or(ReceivingCommandPhase::Ready, |command| command.phase());
        let message = self
            .receiving_command
            .as_ref()
            .and_then(|command| command.message());
        match phase {
            ReceivingCommandPhase::Ready => Self::state_band(
                ui,
                Self::warning(),
                Icon::Save,
                if unloading {
                    "Unloading scan saved"
                } else {
                    "Receipt saved"
                },
                if unloading {
                    "Queued for the warehouse service. Do not start receiving yet."
                } else {
                    "Queued for the warehouse service. Do not scan this inventory again."
                },
            ),
            ReceivingCommandPhase::InFlight => Self::state_band(
                ui,
                Self::warning(),
                Icon::Send,
                if unloading {
                    "Starting unloading"
                } else {
                    "Sending receipt"
                },
                if unloading {
                    "Waiting for the warehouse service. Do not start receiving yet."
                } else {
                    "Waiting for the warehouse service. Do not scan this inventory again."
                },
            ),
            ReceivingCommandPhase::Ambiguous => {
                Self::state_band(
                    ui,
                    Self::danger(),
                    Icon::AlertTriangle,
                    "Checking saved receipt",
                    message.unwrap_or(
                        "The command result is unknown. Check it before handling the inventory.",
                    ),
                );
                ui.add_space(8.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 56.0],
                        egui::Button::new(egui::RichText::new("Check again").strong())
                            .fill(egui::Color32::from_rgb(112, 72, 18)),
                    )
                    .clicked()
                {
                    self.retry_receiving_command();
                }
            }
            ReceivingCommandPhase::ReconcileRequired => Self::state_band(
                ui,
                Self::danger(),
                Icon::ShieldAlert,
                "Receiving blocked",
                message.unwrap_or("The saved receipt must be reconciled before continuing."),
            ),
        }
    }

    fn receiving_refresh_failed(&mut self, ui: &mut egui::Ui) {
        self.receiving_session_summary(ui);
        Self::state_band(
            ui,
            Self::warning(),
            Icon::WifiOff,
            "Receipt saved",
            "The remaining load work could not be refreshed.",
        );
        ui.add_space(8.0);
        if ui
            .add_sized(
                [ui.available_width(), 56.0],
                egui::Button::new(egui::RichText::new("Refresh load").strong())
                    .fill(egui::Color32::from_rgb(112, 72, 18)),
            )
            .clicked()
        {
            let transition = self.receiving.retry_refresh();
            self.emit_receiving_transition(transition);
        }
    }

    fn receiving_complete(&mut self, ui: &mut egui::Ui) {
        Self::state_band(
            ui,
            Self::accent(),
            Icon::PackageCheck,
            "Load complete",
            "All expected quantities have been resolved.",
        );
        ui.add_space(8.0);
        self.receiving_load_scan(ui, true);
        self.receiving_operator_error(ui);
    }

    fn receiving_reconciliation(&self, ui: &mut egui::Ui) {
        Self::state_band(
            ui,
            Self::danger(),
            Icon::ShieldAlert,
            "Receiving blocked",
            reconciliation_message(self.receiving.reconciliation_reason()),
        );
    }

    fn receiving_operator_error(&self, ui: &mut egui::Ui) {
        if let Some(error) = self.receiving.operator_error() {
            ui.add_space(6.0);
            Self::message_band(
                ui,
                Self::danger(),
                Icon::AlertTriangle,
                receiving_error_message(Some(error)),
            );
        }
    }

    fn submit_receiving_scan(&mut self) {
        let scan = self.receiving_ui.scan_draft.trim().to_owned();
        let target = self.receiving.focus_target();
        let transition = self.receiving.submit_scan(&scan);
        let selected_item = target == FocusTarget::Scanner(ScannerTarget::ItemBarcode)
            && matches!(transition, ReceivingTransition::Applied)
            && self.receiving.selected_line().is_some();
        if !matches!(
            transition,
            ReceivingTransition::Blocked(_) | ReceivingTransition::Ignored
        ) {
            self.receiving_ui.scan_draft.clear();
            self.receiving_ui.focus = None;
        }
        self.emit_receiving_transition(transition);
        if selected_item {
            let transition = self.receiving.select_mode(self.receiving_ui.mode);
            self.emit_receiving_transition(transition);
            let transition = self.receiving.set_quantity(1);
            self.emit_receiving_transition(transition);
        }
    }

    fn submit_receiving_load(&mut self) {
        let scan = self.receiving_ui.scan_draft.trim().to_owned();
        let transition = self.receiving.scan_load(&scan);
        if !matches!(
            transition,
            ReceivingTransition::Blocked(_) | ReceivingTransition::Ignored
        ) {
            self.receiving_ui.scan_draft.clear();
            self.receiving_ui.focus = None;
        }
        self.emit_receiving_transition(transition);
    }

    fn request_receiving_focus(&mut self, response: &egui::Response, target: FocusTarget) {
        if self.receiving_ui.focus != Some(target) {
            response.request_focus();
            response.scroll_to_me(Some(egui::Align::Center));
            response.ctx.request_repaint();
            self.receiving_ui.focus = Some(target);
        }
    }

    fn sync_receiving_ui(&mut self) {
        self.receiving_ui
            .sync_from_snapshot(receiving_draft_snapshot(&self.receiving));
        if matches!(
            self.receiving.activity(),
            ReceivingActivity::AwaitingLoad | ReceivingActivity::LoadComplete
        ) && self.receiving.session().is_none()
        {
            self.receiving_ui.selected_line_id = None;
        }
    }

    fn receiving_command_access(&self) -> CommandAccess {
        if self.session.is_none() {
            CommandAccess::Blocked(CommandAccessBlock::SignedOut)
        } else if self.receiving_command.is_some() {
            CommandAccess::Blocked(CommandAccessBlock::SavedCommandPending)
        } else if self.command_store.is_none()
            || self.execution_scope.is_none()
            || self.session_gate != SessionGate::Ready
            || self.receiving_request.is_some()
        {
            CommandAccess::Blocked(CommandAccessBlock::ServerStateUnverified)
        } else {
            CommandAccess::Allowed
        }
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    pub(super) fn open_debug_preview_from_environment(&mut self) {
        let Ok(preview) = std::env::var("WAREBOXES_RF_PREVIEW") else {
            return;
        };
        self.open_debug_preview();
        match preview.as_str() {
            "work-menu" => {
                self.workflow = crate::workflow::MovementWorkflow::default();
                self.cycle_count = crate::cycle_count::CycleCountWorkflow::default();
                self.picking = crate::picking::PickingWorkflow::default();
                self.replenishment = crate::replenishment::ReplenishmentWorkflow::default();
                self.outbound_load = crate::outbound_load::OutboundLoadWorkflow::default();
                self.receiving = ExpectedReceivingReducer::default();
                self.receiving_ui = ReceivingUiState::default();
                self.work_mode = WorkMode::Putaway;
                self.release_confirmation = false;
                self.work_menu_open = true;
            }
            "relocation-loose" => self.open_debug_relocation_preview(MovementKind::Loose),
            "relocation-license-plate" => {
                self.open_debug_relocation_preview(MovementKind::LicensePlate)
            }
            "receiving-active" => self.load_receiving_preview(ReceivingPreview::Active),
            "receiving-unloading" => self.load_receiving_preview(ReceivingPreview::Unloading),
            "receiving-quarantine" => self.load_receiving_preview(ReceivingPreview::Quarantine),
            "receiving-unexpected" => self.load_receiving_preview(ReceivingPreview::Unexpected),
            "receiving-error" => self.load_receiving_preview(ReceivingPreview::Error),
            "receiving-recovery" => self.load_receiving_preview(ReceivingPreview::Recovery),
            "receiving-reconcile" => self.load_receiving_preview(ReceivingPreview::Reconcile),
            "count-active" => self.load_count_preview(),
            "pick-active" => self.load_pick_preview(),
            "pick-shortage" => self.load_pick_shortage_preview(),
            "replenishment-active" => self.load_replenishment_preview(),
            "outbound-load-active" => self.load_outbound_load_preview(),
            _ => {}
        }
    }

    #[cfg(all(debug_assertions, not(target_os = "android")))]
    fn load_receiving_preview(&mut self, preview: ReceivingPreview) {
        self.work_mode = WorkMode::Receive;
        self.workflow = crate::workflow::MovementWorkflow::default();
        self.receiving = ExpectedReceivingReducer::default();
        self.receiving_effects.clear();
        self.receiving_request = None;
        self.receiving_command = None;
        self.receiving_ui.reset_confirmation();

        let effect = self.receiving.scan_load("WB-LOAD-2027");
        let ReceivingTransition::Effect(ReceivingEffect::ResolveLoad { resolution_id, .. }) =
            effect
        else {
            return;
        };
        match preview {
            ReceivingPreview::Error => {
                self.receiving
                    .load_resolution_failed(resolution_id, LoadResolutionFailure::Retryable);
            }
            ReceivingPreview::Active
            | ReceivingPreview::Unloading
            | ReceivingPreview::Quarantine
            | ReceivingPreview::Unexpected
            | ReceivingPreview::Recovery
            | ReceivingPreview::Reconcile => {
                let Some(session) = debug_receiving_session(preview == ReceivingPreview::Unloading)
                else {
                    return;
                };
                self.receiving.load_resolved(resolution_id, session);
                if preview == ReceivingPreview::Unloading {
                    self.receiving_ui.scan_draft = "DOCK-04".into();
                } else if preview == ReceivingPreview::Unexpected {
                    self.receiving.select_mode(ConfirmationMode::Unexpected);
                    self.receiving.scan_item("UNEXPECTED-CASE-200");
                    self.receiving.scan_dock("DOCK-04");
                    self.receiving
                        .set_container_capture(ContainerCapture::LicensePlate);
                    self.receiving.scan_license_plate("QA-UNEXPECTED-200");
                    self.receiving.set_quantity(3);
                    self.receiving
                        .set_unexpected_reason(UnexpectedReceiptReason::UnexpectedItem);
                } else {
                    self.receiving.scan_item("CASE-100");
                }
                if preview == ReceivingPreview::Quarantine {
                    self.receiving.select_mode(ConfirmationMode::Quarantined);
                    self.receiving.scan_dock("DOCK-04");
                    self.receiving
                        .set_container_capture(ContainerCapture::LicensePlate);
                    self.receiving.scan_license_plate("QA-LP-100");
                    self.receiving.set_quantity(2);
                    self.receiving
                        .set_exception_reason(ReceiptExceptionReason::Damaged);
                } else if preview == ReceivingPreview::Recovery {
                    self.receiving.scan_dock("DOCK-04");
                    self.receiving.set_quantity(4);
                    self.receiving_ui.quantity_draft = "4".into();
                    if let ReceivingTransition::Effect(ReceivingEffect::PersistConfirmation {
                        confirmation_id,
                        ..
                    }) = self.receiving.begin_confirmation(CommandAccess::Allowed)
                    {
                        self.receiving_command = Some(
                            super::session::ReceivingCommandRuntime::debug_ambiguous(
                                confirmation_id,
                                "The connection ended before the warehouse service confirmed the receipt.",
                            ),
                        );
                    }
                } else if preview == ReceivingPreview::Reconcile {
                    self.receiving
                        .require_reconciliation(ReconciliationReason::InvalidServerState);
                }
            }
        }
    }
}

#[cfg(all(debug_assertions, not(target_os = "android")))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReceivingPreview {
    Active,
    Unloading,
    Quarantine,
    Unexpected,
    Error,
    Recovery,
    Reconcile,
}

fn receiving_draft_snapshot(reducer: &ExpectedReceivingReducer) -> Option<ReceivingDraftSnapshot> {
    reducer
        .confirmation_draft_view()
        .map(|draft| ReceivingDraftSnapshot {
            mode: draft.mode,
            selected_line_id: draft.selected_line_id,
            item_barcode: draft
                .item_barcode
                .map(|barcode| barcode.as_str().to_owned()),
            dock_barcode: draft
                .dock_barcode
                .map(|barcode| barcode.as_str().to_owned()),
            quantity: draft.quantity.map(PositiveQuantity::get),
            container: draft.container_capture,
            license_plate_barcode: draft
                .license_plate_barcode
                .map(|barcode| barcode.as_str().to_owned()),
            reason: draft.exception_reason,
            unexpected_reason: draft.unexpected_reason,
            note: draft.exception_note.map(str::to_owned),
        })
}

const fn confirmation_mode_label(mode: ConfirmationMode) -> &'static str {
    match mode {
        ConfirmationMode::Received => "Received",
        ConfirmationMode::Quarantined => "Quarantine",
        ConfirmationMode::Unexpected => "Unexpected",
        ConfirmationMode::Rejected => "Rejected",
        ConfirmationMode::Missing => "Missing",
    }
}

const fn scanner_prompt(target: ScannerTarget) -> &'static str {
    match target {
        ScannerTarget::LoadBarcode => "Scan inbound load",
        ScannerTarget::ItemBarcode => "Scan expected item",
        ScannerTarget::DockBarcode => "Scan receiving dock",
        ScannerTarget::SealBarcode => "Scan trailer seal",
        ScannerTarget::LicensePlateBarcode => "Scan license plate",
    }
}

const fn scanner_hint(target: ScannerTarget) -> &'static str {
    match target {
        ScannerTarget::LoadBarcode => "LOAD BARCODE",
        ScannerTarget::ItemBarcode => "ITEM BARCODE",
        ScannerTarget::DockBarcode => "DOCK BARCODE",
        ScannerTarget::SealBarcode => "SEAL BARCODE",
        ScannerTarget::LicensePlateBarcode => "LICENSE PLATE",
    }
}

const fn receiving_error_message(error: Option<&ReceivingOperatorError>) -> &'static str {
    match error {
        Some(ReceivingOperatorError::InvalidScan) => "The barcode is not valid. Scan again.",
        Some(ReceivingOperatorError::ItemNotExpected) => {
            "This item is not expected on the open load."
        }
        Some(ReceivingOperatorError::ItemMatchesMultipleLines { .. }) => {
            "This item matches more than one line. Select the correct line."
        }
        Some(ReceivingOperatorError::LineNotOpen) => "That receiving line is no longer open.",
        Some(ReceivingOperatorError::ItemDoesNotMatchLine) => {
            "The scanned item does not match that line."
        }
        Some(ReceivingOperatorError::WrongReceivingDock) => {
            "Wrong dock. Scan the assigned receiving dock."
        }
        Some(ReceivingOperatorError::WrongSeal) => "Wrong seal. Scan the planned trailer seal.",
        Some(ReceivingOperatorError::InvalidQuantity) => "Enter a quantity greater than zero.",
        Some(ReceivingOperatorError::QuantityExceedsRemaining) => {
            "Quantity exceeds the expected amount remaining."
        }
        Some(ReceivingOperatorError::DimensionDoesNotMatchExpected) => {
            "The stock detail does not match the expected line."
        }
        Some(ReceivingOperatorError::LoadNotFound) => "Load not found or not available to you.",
        Some(ReceivingOperatorError::LoadNotReady) => "This load is not ready for receiving.",
        Some(ReceivingOperatorError::ConnectionUnavailable) | None => {
            "Can't reach the warehouse service. Check Wi-Fi and try again."
        }
        Some(ReceivingOperatorError::ConfirmationRejected) => {
            "The receipt was not accepted. Review the line and try again."
        }
        Some(ReceivingOperatorError::UnloadingStartRejected) => {
            "Unloading was not accepted. Refresh the load before continuing."
        }
    }
}

const fn action_guard_message(guard: ActionGuard) -> &'static str {
    match guard {
        ActionGuard::Allowed => "Ready to confirm",
        ActionGuard::Blocked(reason) => action_block_message(reason),
    }
}

const fn action_block_message(reason: ActionBlockReason) -> &'static str {
    match reason {
        ActionBlockReason::Device(CommandAccessBlock::SignedOut) => "Sign in to continue.",
        ActionBlockReason::Device(CommandAccessBlock::Offline) => {
            "Reconnect before confirming inventory."
        }
        ActionBlockReason::Device(CommandAccessBlock::SavedCommandPending) => {
            "Finish the saved receipt before continuing."
        }
        ActionBlockReason::Device(CommandAccessBlock::ServerStateUnverified) => {
            "Wait for server state to be checked."
        }
        ActionBlockReason::NoActiveSession => "Scan an inbound load first.",
        ActionBlockReason::NoSelectedLine => "Scan an item or select an open line.",
        ActionBlockReason::ItemScanRequired => "Scan the expected item.",
        ActionBlockReason::DockScanRequired => "Scan the assigned receiving dock.",
        ActionBlockReason::SealScanRequired => "Scan the planned trailer seal.",
        ActionBlockReason::QuantityRequired => "Enter the quantity.",
        ActionBlockReason::LicensePlateScanRequired => "Scan the license plate.",
        ActionBlockReason::ExceptionReasonRequired => "Select an exception reason.",
        ActionBlockReason::ExceptionNoteRequired => "Enter a note for the exception.",
        ActionBlockReason::QuantityExceedsRemaining => {
            "Quantity exceeds the expected amount remaining."
        }
        ActionBlockReason::WorkflowBusy => "Wait for the current action to finish.",
        ActionBlockReason::LoadComplete => "This load is complete.",
        ActionBlockReason::ReconciliationRequired => "Receiving is blocked for reconciliation.",
    }
}

const fn reconciliation_message(reason: Option<ReconciliationReason>) -> &'static str {
    match reason {
        Some(ReconciliationReason::CommandIntegrityFailure) => {
            "The saved receipt could not be verified. Contact a supervisor."
        }
        Some(
            ReconciliationReason::ConfirmationIdentityMismatch
            | ReconciliationReason::ConfirmationDispositionMismatch
            | ReconciliationReason::ConfirmationQuantityMismatch,
        ) => "The receipt response does not match the saved command. Contact a supervisor.",
        Some(
            ReconciliationReason::CumulativeQuantityRegressed
            | ReconciliationReason::CumulativeQuantityInvalid
            | ReconciliationReason::RefreshQuantityRegressed,
        ) => "Server quantities conflict with this device. Contact a supervisor.",
        Some(
            ReconciliationReason::RefreshAggregateMismatch
            | ReconciliationReason::InvalidServerState,
        )
        | None => "Load state could not be verified. Contact a supervisor.",
    }
}

#[cfg(all(debug_assertions, not(target_os = "android")))]
fn debug_receiving_session(unloading: bool) -> Option<ReceivingSession> {
    let first = debug_line(
        501,
        1_100,
        "CASE-100",
        "Surgical gloves, case of 100",
        12,
        2,
    )?;
    let second = debug_line(502, 1_101, "CASE-220", "Sterile wipes, 24 pack", 8, 0)?;
    ReceivingSession::try_new(ReceivingSessionInput {
        load_id: LoadId::try_from(2_027).ok()?,
        inventory_owner_id: InventoryOwnerId::try_from(12).ok()?,
        facility_id: FacilityId::try_from(4).ok()?,
        reference_number: Some("ASN-2027-00418".into()),
        status: if unloading {
            ReceivingLoadStatus::Arrived
        } else {
            ReceivingLoadStatus::Receiving
        },
        expected_seal: unloading
            .then(|| SealBarcode::new("SEAL-2027"))
            .transpose()
            .ok()?,
        dock: ReceivingDock::new(
            LocationId::try_from(44).ok()?,
            crate::expected_receiving::DockBarcode::new("DOCK-04").ok()?,
            Some("Receiving Dock 04".into()),
        ),
        lines: vec![first, second],
    })
    .ok()
}

#[cfg(all(debug_assertions, not(target_os = "android")))]
fn debug_line(
    line_id: i64,
    item_id: i64,
    barcode: &str,
    description: &str,
    expected: i64,
    received: i64,
) -> Option<ExpectedReceiptLine> {
    ExpectedReceiptLine::try_new(ExpectedReceiptLineInput {
        load_line_id: LoadLineId::try_from(line_id).ok()?,
        item_id: ItemId::try_from(item_id).ok()?,
        item_description: Some(description.into()),
        uom: StockDimension::new("cases").ok()?,
        item_barcodes: vec![ItemBarcode::new(barcode).ok()?],
        expected: PositiveQuantity::try_from(expected).ok()?,
        received: NonNegativeQuantity::new(received).ok()?,
        rejected: NonNegativeQuantity::new(0).ok()?,
        missing: NonNegativeQuantity::new(0).ok()?,
        remaining: NonNegativeQuantity::new(expected - received).ok()?,
        lot: Some(StockDimension::new("LOT-2407-A").ok()?),
        serial: None,
        expiration: None,
    })
    .ok()
}
