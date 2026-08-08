use eframe::egui;
use lucide_icons::Icon;

use crate::expected_receiving::{
    ActionBlockReason, ActionGuard, CommandAccess, CommandAccessBlock, ConfirmationMode,
    ContainerCapture, ExceptionNote, ExpectedReceiptLine, ExpectedReceivingReducer, FocusTarget,
    LoadLineId, PositiveQuantity, ReceiptExceptionReason, ReceivingActivity,
    ReceivingOperatorError, ReceivingTransition, ReconciliationReason, ScannerTarget,
};
#[cfg(all(debug_assertions, not(target_os = "android")))]
use crate::expected_receiving::{
    ExpectedReceiptLineInput, FacilityId, InventoryOwnerId, ItemBarcode, ItemId, LoadId,
    LoadResolutionFailure, LocationId, NonNegativeQuantity, ReceivingDock, ReceivingEffect,
    ReceivingLoadStatus, ReceivingSession, ReceivingSessionInput, StockDimension,
};
use crate::workflow::{Activity, MovementKind, MovementOperation};

use super::RfApp;
use super::SessionGate;
use super::session::ReceivingCommandPhase;

mod saved;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum WorkMode {
    Receive,
    Putaway,
    Pick,
    Relocate,
    Count,
}

impl WorkMode {
    const ALL: [Self; 5] = [
        Self::Receive,
        Self::Putaway,
        Self::Pick,
        Self::Relocate,
        Self::Count,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Receive => "Receive",
            Self::Putaway => "Putaway",
            Self::Pick => "Pick",
            Self::Relocate => "Relocate",
            Self::Count => "Count",
        }
    }
}

impl From<MovementOperation> for WorkMode {
    fn from(operation: MovementOperation) -> Self {
        match operation {
            MovementOperation::Putaway => Self::Putaway,
            MovementOperation::InventoryRelocation => Self::Relocate,
        }
    }
}

pub(super) struct ReceivingUiState {
    scan_draft: String,
    quantity_draft: String,
    note_draft: String,
    mode: ConfirmationMode,
    container: ContainerCapture,
    reason: Option<ReceiptExceptionReason>,
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
    note: Option<String>,
}

impl Default for ReceivingUiState {
    fn default() -> Self {
        Self {
            scan_draft: String::new(),
            quantity_draft: "1".into(),
            note_draft: String::new(),
            mode: ConfirmationMode::Received,
            container: ContainerCapture::Loose,
            reason: None,
            focus: None,
            selected_line_id: None,
            viewport_width: None,
            synced_draft: None,
        }
    }
}

impl ReceivingUiState {
    #[cfg(all(debug_assertions, not(target_os = "android")))]
    fn reset_confirmation(&mut self) {
        self.scan_draft.clear();
        self.quantity_draft = "1".into();
        self.note_draft.clear();
        self.mode = ConfirmationMode::Received;
        self.container = ContainerCapture::Loose;
        self.reason = None;
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
            && self.reason == draft.reason;
        let note_matches = if draft.reason == Some(ReceiptExceptionReason::Other) {
            ExceptionNote::new(self.note_draft.clone()).is_ok()
                && draft.note.as_deref() == Some(self.note_draft.as_str())
        } else {
            true
        };
        quantity_matches && controls_match && note_matches
    }
}

impl RfApp {
    pub(super) fn work_header(&mut self, ui: &mut egui::Ui) {
        ui.set_min_height(130.0);
        ui.set_max_height(130.0);
        let (label, color) = self.work_status();
        egui::containers::Sides::new().height(34.0).show(
            ui,
            |ui| {
                ui.horizontal(|ui| {
                    let icon = match self.work_mode {
                        WorkMode::Receive => Icon::PackagePlus,
                        WorkMode::Putaway => Icon::PackageOpen,
                        WorkMode::Pick => Icon::ScanBarcode,
                        WorkMode::Relocate => Icon::Move,
                        WorkMode::Count => Icon::ClipboardCheck,
                    };
                    ui.label(Self::icon(icon).color(Self::accent()));
                    ui.heading(self.work_mode.label());
                });
            },
            |ui| {
                ui.label(egui::RichText::new(label).strong().color(color));
            },
        );
        if let Some(session) = self.session.as_ref() {
            ui.label(
                egui::RichText::new(&session.tenant_name)
                    .small()
                    .color(egui::Color32::from_rgb(166, 177, 173)),
            );
        }

        let switching_allowed = work_mode_switch_allowed(
            self.workflow.activity(),
            self.receiving.activity(),
            self.cycle_count.activity(),
            self.picking.activity(),
        );
        let segment_width = (ui.available_width() - 32.0) / 5.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().button_padding.x = 3.0;
            for mode in WorkMode::ALL {
                let selected = self.work_mode == mode;
                let response = ui
                    .add_enabled(
                        selected || switching_allowed,
                        egui::Button::selectable(
                            selected,
                            egui::RichText::new(mode.label()).small(),
                        )
                        .min_size(egui::vec2(segment_width, 48.0)),
                    )
                    .on_disabled_hover_text("Finish or recover current work before switching");
                if response.clicked() && switching_allowed {
                    self.work_mode = mode;
                    match mode {
                        WorkMode::Putaway => {
                            self.workflow.select_operation(MovementOperation::Putaway)
                        }
                        WorkMode::Relocate => self
                            .workflow
                            .select_operation(MovementOperation::InventoryRelocation),
                        WorkMode::Receive | WorkMode::Pick | WorkMode::Count => {}
                    }
                    self.receiving_ui.focus = None;
                    self.scan_focus = None;
                    self.pick_scan_focus = None;
                }
            }
        });
        ui.separator();
    }

    fn work_status(&self) -> (&'static str, egui::Color32) {
        match self.work_mode {
            WorkMode::Putaway | WorkMode::Relocate => {
                self.heartbeat_header()
                    .unwrap_or_else(|| match self.workflow.activity() {
                        Activity::Idle => ("READY", Self::accent()),
                        Activity::Active => ("ACTIVE", Self::accent()),
                        Activity::Persisting => ("SAVING", Self::warning()),
                        Activity::ReadyToDispatch => ("QUEUED", Self::warning()),
                        Activity::InFlight => ("SENDING", Self::warning()),
                        Activity::Ambiguous => ("CHECK", Self::danger()),
                        Activity::ReconcileRequired => ("BLOCKED", Self::danger()),
                    })
            }
            WorkMode::Count => self
                .heartbeat_header()
                .unwrap_or_else(|| activity_status(self.cycle_count.activity())),
            WorkMode::Pick => self
                .heartbeat_header()
                .unwrap_or_else(|| activity_status(self.picking.activity())),
            WorkMode::Receive => match self.receiving.activity() {
                ReceivingActivity::AwaitingLoad | ReceivingActivity::LoadComplete => {
                    ("READY", Self::accent())
                }
                ReceivingActivity::Active => ("ACTIVE", Self::accent()),
                ReceivingActivity::ConfirmationPending => self.receiving_command.as_ref().map_or(
                    ("WORKING", Self::warning()),
                    |command| match command.phase() {
                        ReceivingCommandPhase::Ready | ReceivingCommandPhase::InFlight => {
                            ("WORKING", Self::warning())
                        }
                        ReceivingCommandPhase::Ambiguous => ("CHECK", Self::danger()),
                        ReceivingCommandPhase::ReconcileRequired => ("BLOCKED", Self::danger()),
                    },
                ),
                ReceivingActivity::ResolvingLoad | ReceivingActivity::Refreshing => {
                    ("WORKING", Self::warning())
                }
                ReceivingActivity::LoadResolutionFailed | ReceivingActivity::RefreshFailed => {
                    ("RETRY", Self::warning())
                }
                ReceivingActivity::ReconcileRequired => ("BLOCKED", Self::danger()),
            },
        }
    }

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
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(if next_load {
                "SCAN NEXT LOAD"
            } else {
                "SCAN INBOUND LOAD"
            })
            .small()
            .strong(),
        );
        let response = ui.add_sized(
            [ui.available_width(), 58.0],
            egui::TextEdit::singleline(&mut self.receiving_ui.scan_draft)
                .id(egui::Id::new("receiving_load_scan"))
                .font(egui::TextStyle::Monospace)
                .hint_text("LOAD BARCODE"),
        );
        self.request_receiving_focus(&response, FocusTarget::Scanner(ScannerTarget::LoadBarcode));

        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let has_scan = !self.receiving_ui.scan_draft.trim().is_empty();
        let clicked = ui
            .add_enabled(
                has_scan,
                egui::Button::new(egui::RichText::new("Open load").strong())
                    .fill(Self::primary_fill(has_scan))
                    .min_size(egui::vec2(ui.available_width(), 56.0)),
            )
            .on_disabled_hover_text("Scan a load barcode")
            .clicked();
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
        self.receiving_session_summary(ui);
        self.receiving_disposition(ui);

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
            if self.receiving_ui.mode == ConfirmationMode::Received {
                self.receiving_container_control(ui);
            }
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
                self.receiving_quantity(ui);
                if matches!(
                    self.receiving_ui.mode,
                    ConfirmationMode::Rejected | ConfirmationMode::Missing
                ) {
                    self.receiving_exception(ui);
                }
                self.receiving_confirm(ui);
            }
            FocusTarget::Scanner(ScannerTarget::LoadBarcode | ScannerTarget::ItemBarcode)
            | FocusTarget::Blocked(_) => {}
        }
        self.receiving_operator_error(ui);
    }

    fn receiving_session_summary(&self, ui: &mut egui::Ui) {
        let Some(session) = self.receiving.session() else {
            return;
        };
        let reference = session.reference_number().unwrap_or("Inbound load");
        ui.label(egui::RichText::new(reference).size(22.0).strong());
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} open {}",
                session.lines().len(),
                if session.lines().len() == 1 {
                    "line"
                } else {
                    "lines"
                }
            ));
            ui.separator();
            ui.label(egui::RichText::new("RECEIVE AT").small().strong());
            ui.monospace(session.dock().barcode().as_str());
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
        ui.separator();
    }

    fn receiving_disposition(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("DISPOSITION").small().strong());
        let width = (ui.available_width() - 16.0) / 3.0;
        ui.horizontal(|ui| {
            for mode in [
                ConfirmationMode::Received,
                ConfirmationMode::Rejected,
                ConfirmationMode::Missing,
            ] {
                if ui
                    .add_sized(
                        [width, 50.0],
                        egui::Button::selectable(
                            self.receiving_ui.mode == mode,
                            confirmation_mode_label(mode),
                        ),
                    )
                    .clicked()
                {
                    self.receiving_ui.mode = mode;
                    self.receiving_ui.reason = None;
                    self.receiving_ui.note_draft.clear();
                    self.receiving_ui.focus = None;
                    let transition = self.receiving.select_mode(mode);
                    self.emit_receiving_transition(transition);
                }
            }
        });
    }

    fn receiving_scan_control(&mut self, ui: &mut egui::Ui, target: ScannerTarget) {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(scanner_prompt(target))
                .size(19.0)
                .strong()
                .color(Self::accent()),
        );
        let response = ui.add_sized(
            [ui.available_width(), 58.0],
            egui::TextEdit::singleline(&mut self.receiving_ui.scan_draft)
                .id(egui::Id::new(("receiving_scan", scanner_hint(target))))
                .font(egui::TextStyle::Monospace)
                .hint_text(scanner_hint(target)),
        );
        self.request_receiving_focus(&response, FocusTarget::Scanner(target));
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let has_scan = !self.receiving_ui.scan_draft.trim().is_empty();
        let clicked = ui
            .add_enabled(
                has_scan,
                egui::Button::new(egui::RichText::new("Confirm scan").strong())
                    .fill(Self::primary_fill(has_scan))
                    .min_size(egui::vec2(ui.available_width(), 54.0)),
            )
            .clicked();
        if has_scan && (enter || clicked) {
            self.submit_receiving_scan();
        }
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

    fn receiving_container_control(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("CONTAINER").small().strong());
        let width = (ui.available_width() - 8.0) / 2.0;
        ui.horizontal(|ui| {
            for (capture, label) in [
                (ContainerCapture::Loose, "Loose"),
                (ContainerCapture::LicensePlate, "License plate"),
            ] {
                if ui
                    .add_sized(
                        [width, 48.0],
                        egui::Button::selectable(self.receiving_ui.container == capture, label),
                    )
                    .clicked()
                {
                    self.receiving_ui.container = capture;
                    self.receiving_ui.focus = None;
                    let transition = self.receiving.set_container_capture(capture);
                    self.emit_receiving_transition(transition);
                }
            }
        });
    }

    fn receiving_quantity(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("QUANTITY").small().strong());
        let response = ui.add_sized(
            [ui.available_width(), 56.0],
            egui::TextEdit::singleline(&mut self.receiving_ui.quantity_draft)
                .id(egui::Id::new("receiving_quantity"))
                .font(egui::TextStyle::Monospace)
                .char_limit(10)
                .hint_text("1"),
        );
        if self.receiving.focus_target() == FocusTarget::Quantity {
            self.request_receiving_focus(&response, FocusTarget::Quantity);
        }
        if response.changed()
            && let Ok(quantity) = self.receiving_ui.quantity_draft.parse::<i64>()
        {
            let transition = self.receiving.set_quantity(quantity);
            self.emit_receiving_transition(transition);
        }
    }

    fn receiving_exception(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("REASON").small().strong());
        egui::ComboBox::from_id_salt("receiving_exception_reason")
            .width(ui.available_width())
            .selected_text(
                self.receiving_ui
                    .reason
                    .map_or("Select a reason", exception_reason_label),
            )
            .show_ui(ui, |ui| {
                for reason in [
                    ReceiptExceptionReason::Damaged,
                    ReceiptExceptionReason::QualityRejected,
                    ReceiptExceptionReason::ShortShipment,
                    ReceiptExceptionReason::CountDiscrepancy,
                    ReceiptExceptionReason::WrongItem,
                    ReceiptExceptionReason::Other,
                ] {
                    if ui
                        .selectable_value(
                            &mut self.receiving_ui.reason,
                            Some(reason),
                            exception_reason_label(reason),
                        )
                        .changed()
                    {
                        let transition = self.receiving.set_exception_reason(reason);
                        self.emit_receiving_transition(transition);
                        if reason != ReceiptExceptionReason::Other {
                            self.receiving_ui.note_draft.clear();
                        }
                        self.receiving_ui.focus = None;
                    }
                }
            });

        if self.receiving_ui.reason == Some(ReceiptExceptionReason::Other) {
            ui.label(egui::RichText::new("NOTE").small().strong());
            let response = ui.add_sized(
                [ui.available_width(), 56.0],
                egui::TextEdit::singleline(&mut self.receiving_ui.note_draft)
                    .id(egui::Id::new("receiving_exception_note"))
                    .char_limit(1_000)
                    .hint_text("Required detail"),
            );
            if self.receiving.focus_target() == FocusTarget::ExceptionNote {
                self.request_receiving_focus(&response, FocusTarget::ExceptionNote);
            }
            if response.changed() {
                let value = (!self.receiving_ui.note_draft.is_empty())
                    .then_some(self.receiving_ui.note_draft.as_str());
                let transition = self.receiving.set_exception_note(value);
                self.emit_receiving_transition(transition);
            }
        }
    }

    fn receiving_confirm(&mut self, ui: &mut egui::Ui) {
        let access = self.receiving_command_access();
        let guard = self.receiving.confirmation_guard(access);
        let displayed_values_match = receiving_draft_snapshot(&self.receiving)
            .is_some_and(|draft| self.receiving_ui.displayed_confirmation_matches(&draft));
        let enabled = guard == ActionGuard::Allowed && displayed_values_match;
        let label = match self.receiving_ui.mode {
            ConfirmationMode::Received => "Confirm receipt",
            ConfirmationMode::Rejected => "Confirm rejection",
            ConfirmationMode::Missing => "Record missing",
        };
        if ui
            .add_enabled(
                enabled,
                egui::Button::new(egui::RichText::new(label).strong())
                    .fill(Self::primary_fill(enabled))
                    .min_size(egui::vec2(ui.available_width(), 58.0)),
            )
            .on_disabled_hover_text(action_guard_message(guard))
            .clicked()
        {
            let transition = self.receiving.begin_confirmation(access);
            self.emit_receiving_transition(transition);
        }
        if let ActionGuard::Blocked(reason) = guard {
            ui.label(
                egui::RichText::new(action_block_message(reason))
                    .small()
                    .color(egui::Color32::from_rgb(166, 177, 173)),
            );
        }
    }

    fn receiving_confirmation_pending(&mut self, ui: &mut egui::Ui) {
        self.receiving_session_summary(ui);
        self.receiving_saved_draft(ui);
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
                "Receipt saved",
                "Queued for the warehouse service. Do not scan this inventory again.",
            ),
            ReceivingCommandPhase::InFlight => Self::state_band(
                ui,
                Self::warning(),
                Icon::Send,
                "Sending receipt",
                "Waiting for the warehouse service. Do not scan this inventory again.",
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
            "relocation-loose" => self.open_debug_relocation_preview(MovementKind::Loose),
            "relocation-license-plate" => {
                self.open_debug_relocation_preview(MovementKind::LicensePlate)
            }
            "receiving-active" => self.load_receiving_preview(ReceivingPreview::Active),
            "receiving-error" => self.load_receiving_preview(ReceivingPreview::Error),
            "receiving-recovery" => self.load_receiving_preview(ReceivingPreview::Recovery),
            "receiving-reconcile" => self.load_receiving_preview(ReceivingPreview::Reconcile),
            "count-active" => self.load_count_preview(),
            "pick-active" => self.load_pick_preview(),
            "pick-shortage" => self.load_pick_shortage_preview(),
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
            ReceivingPreview::Active | ReceivingPreview::Recovery | ReceivingPreview::Reconcile => {
                let Some(session) = debug_receiving_session() else {
                    return;
                };
                self.receiving.load_resolved(resolution_id, session);
                self.receiving.scan_item("CASE-100");
                if preview == ReceivingPreview::Recovery {
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
            note: draft.exception_note.map(str::to_owned),
        })
}

fn work_mode_switch_allowed(
    putaway: Activity,
    receiving: ReceivingActivity,
    count: Activity,
    picking: Activity,
) -> bool {
    putaway == Activity::Idle
        && count == Activity::Idle
        && picking == Activity::Idle
        && matches!(
            receiving,
            ReceivingActivity::AwaitingLoad | ReceivingActivity::LoadComplete
        )
}

fn activity_status(activity: Activity) -> (&'static str, egui::Color32) {
    match activity {
        Activity::Idle => ("READY", RfApp::accent()),
        Activity::Active => ("ACTIVE", RfApp::accent()),
        Activity::Persisting => ("SAVING", RfApp::warning()),
        Activity::ReadyToDispatch => ("QUEUED", RfApp::warning()),
        Activity::InFlight => ("SENDING", RfApp::warning()),
        Activity::Ambiguous => ("CHECK", RfApp::danger()),
        Activity::ReconcileRequired => ("BLOCKED", RfApp::danger()),
    }
}

const fn confirmation_mode_label(mode: ConfirmationMode) -> &'static str {
    match mode {
        ConfirmationMode::Received => "Received",
        ConfirmationMode::Rejected => "Rejected",
        ConfirmationMode::Missing => "Missing",
    }
}

const fn scanner_prompt(target: ScannerTarget) -> &'static str {
    match target {
        ScannerTarget::LoadBarcode => "Scan inbound load",
        ScannerTarget::ItemBarcode => "Scan expected item",
        ScannerTarget::DockBarcode => "Scan receiving dock",
        ScannerTarget::LicensePlateBarcode => "Scan license plate",
    }
}

const fn scanner_hint(target: ScannerTarget) -> &'static str {
    match target {
        ScannerTarget::LoadBarcode => "LOAD BARCODE",
        ScannerTarget::ItemBarcode => "ITEM BARCODE",
        ScannerTarget::DockBarcode => "DOCK BARCODE",
        ScannerTarget::LicensePlateBarcode => "LICENSE PLATE",
    }
}

const fn exception_reason_label(reason: ReceiptExceptionReason) -> &'static str {
    match reason {
        ReceiptExceptionReason::Damaged => "Damaged",
        ReceiptExceptionReason::QualityRejected => "Quality rejected",
        ReceiptExceptionReason::ShortShipment => "Short shipment",
        ReceiptExceptionReason::CountDiscrepancy => "Count discrepancy",
        ReceiptExceptionReason::WrongItem => "Wrong item",
        ReceiptExceptionReason::Other => "Other",
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
fn debug_receiving_session() -> Option<ReceivingSession> {
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
        status: ReceivingLoadStatus::Receiving,
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
