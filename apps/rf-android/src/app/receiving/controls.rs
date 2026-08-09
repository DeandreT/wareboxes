use eframe::egui;
use lucide_icons::Icon;

use crate::expected_receiving::{
    ActionGuard, ConfirmationMode, ContainerCapture, FocusTarget, ReceiptExceptionReason,
    ReceivingLoadStatus, UnexpectedReceiptReason,
};

use super::{ReceivingUiState, RfApp, action_block_message, receiving_draft_snapshot};

impl RfApp {
    pub(super) fn receiving_unexpected_evidence(&self, ui: &mut egui::Ui) {
        let Some(draft) = receiving_draft_snapshot(&self.receiving) else {
            return;
        };
        let Some(item_barcode) = draft.item_barcode.as_deref() else {
            return;
        };
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_min_width((width - 20.0).max(0.0));
                ui.label(egui::RichText::new("UNEXPECTED ITEM").small().strong());
                ui.monospace(item_barcode);
                ui.horizontal_wrapped(|ui| {
                    if let Some(dock) = draft.dock_barcode.as_deref() {
                        ui.monospace(format!("Dock {dock}"));
                    }
                    ui.separator();
                    if let Some(plate) = draft.license_plate_barcode.as_deref() {
                        ui.monospace(format!("LP {plate}"));
                    } else {
                        ui.label("Loose");
                    }
                });
            });
    }

    pub(super) fn receiving_disposition(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("DISPOSITION").small().strong());
        let expected_work_complete = self
            .receiving
            .session()
            .is_some_and(|session| session.status() == ReceivingLoadStatus::Received);
        let width = (ui.available_width() - 8.0) / 2.0;
        if !expected_work_complete {
            egui::Grid::new("receiving_disposition_grid")
                .num_columns(2)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    for mode in [
                        ConfirmationMode::Received,
                        ConfirmationMode::Quarantined,
                        ConfirmationMode::Rejected,
                        ConfirmationMode::Missing,
                    ] {
                        if ui
                            .add_sized(
                                [width, 50.0],
                                egui::Button::selectable(
                                    self.receiving_ui.mode == mode,
                                    super::confirmation_mode_label(mode),
                                ),
                            )
                            .clicked()
                        {
                            self.select_receiving_mode(mode);
                        }
                        if matches!(
                            mode,
                            ConfirmationMode::Quarantined | ConfirmationMode::Missing
                        ) {
                            ui.end_row();
                        }
                    }
                });
        } else {
            Self::message_band(
                ui,
                Self::accent(),
                Icon::PackageCheck,
                "All expected work is complete",
            );
        }
        if ui
            .add_sized(
                [ui.available_width(), 50.0],
                egui::Button::selectable(
                    self.receiving_ui.mode == ConfirmationMode::Unexpected,
                    "Unexpected / excess stock",
                ),
            )
            .clicked()
        {
            self.select_receiving_mode(ConfirmationMode::Unexpected);
        }
        if expected_work_complete
            && ui
                .add_sized(
                    [ui.available_width(), 46.0],
                    egui::Button::new("Finish load and scan next"),
                )
                .clicked()
        {
            let transition = self.receiving.finish_received_load();
            self.emit_receiving_transition(transition);
            self.receiving_ui = ReceivingUiState::default();
        }
    }

    fn select_receiving_mode(&mut self, mode: ConfirmationMode) {
        self.receiving_ui.mode = mode;
        self.receiving_ui.reason = None;
        self.receiving_ui.unexpected_reason = None;
        self.receiving_ui.note_draft.clear();
        self.receiving_ui.focus = None;
        let transition = self.receiving.select_mode(mode);
        self.emit_receiving_transition(transition);
    }

    pub(super) fn receiving_container_control(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn receiving_quantity(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn receiving_exception(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("REASON").small().strong());
        egui::ComboBox::from_id_salt("receiving_exception_reason")
            .width(ui.available_width())
            .selected_text(
                self.receiving_ui
                    .reason
                    .map_or("Select a reason", exception_reason_label),
            )
            .show_ui(ui, |ui| {
                let reasons: &[_] = if self.receiving_ui.mode == ConfirmationMode::Quarantined {
                    &[
                        ReceiptExceptionReason::Damaged,
                        ReceiptExceptionReason::QualityRejected,
                        ReceiptExceptionReason::CountDiscrepancy,
                        ReceiptExceptionReason::WrongItem,
                        ReceiptExceptionReason::Other,
                    ]
                } else {
                    &[
                        ReceiptExceptionReason::Damaged,
                        ReceiptExceptionReason::QualityRejected,
                        ReceiptExceptionReason::ShortShipment,
                        ReceiptExceptionReason::CountDiscrepancy,
                        ReceiptExceptionReason::WrongItem,
                        ReceiptExceptionReason::Other,
                    ]
                };
                for &reason in reasons {
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
            self.receiving_note(ui, "receiving_exception_note");
        }
    }

    pub(super) fn receiving_unexpected_reason(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("REASON").small().strong());
        egui::ComboBox::from_id_salt("receiving_unexpected_reason")
            .width(ui.available_width())
            .selected_text(
                self.receiving_ui
                    .unexpected_reason
                    .map_or("Select a reason", unexpected_reason_label),
            )
            .show_ui(ui, |ui| {
                for reason in [
                    UnexpectedReceiptReason::Excess,
                    UnexpectedReceiptReason::UnexpectedItem,
                    UnexpectedReceiptReason::BlindReceipt,
                    UnexpectedReceiptReason::MisShipped,
                    UnexpectedReceiptReason::Other,
                ] {
                    if ui
                        .selectable_value(
                            &mut self.receiving_ui.unexpected_reason,
                            Some(reason),
                            unexpected_reason_label(reason),
                        )
                        .changed()
                    {
                        let transition = self.receiving.set_unexpected_reason(reason);
                        self.emit_receiving_transition(transition);
                        if reason != UnexpectedReceiptReason::Other {
                            self.receiving_ui.note_draft.clear();
                        }
                        self.receiving_ui.focus = None;
                    }
                }
            });
        if self.receiving_ui.unexpected_reason == Some(UnexpectedReceiptReason::Other) {
            self.receiving_note(ui, "receiving_unexpected_note");
        }
    }

    fn receiving_note(&mut self, ui: &mut egui::Ui, id: &'static str) {
        ui.label(egui::RichText::new("NOTE").small().strong());
        let response = ui.add_sized(
            [ui.available_width(), 56.0],
            egui::TextEdit::singleline(&mut self.receiving_ui.note_draft)
                .id(egui::Id::new(id))
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

    pub(super) fn receiving_confirm(&mut self, ui: &mut egui::Ui) {
        let access = self.receiving_command_access();
        let guard = self.receiving.confirmation_guard(access);
        let displayed_values_match = receiving_draft_snapshot(&self.receiving)
            .is_some_and(|draft| self.receiving_ui.displayed_confirmation_matches(&draft));
        let enabled = guard == ActionGuard::Allowed && displayed_values_match;
        let label = match self.receiving_ui.mode {
            ConfirmationMode::Received => "Confirm receipt",
            ConfirmationMode::Quarantined => "Receive into quarantine",
            ConfirmationMode::Unexpected => "Receive unexpected into quarantine",
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
            .on_disabled_hover_text(super::action_guard_message(guard))
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
}

pub(super) const fn exception_reason_label(reason: ReceiptExceptionReason) -> &'static str {
    match reason {
        ReceiptExceptionReason::Damaged => "Damaged",
        ReceiptExceptionReason::QualityRejected => "Quality rejected",
        ReceiptExceptionReason::ShortShipment => "Short shipment",
        ReceiptExceptionReason::CountDiscrepancy => "Count discrepancy",
        ReceiptExceptionReason::WrongItem => "Wrong item",
        ReceiptExceptionReason::Other => "Other",
    }
}

pub(super) const fn unexpected_reason_label(reason: UnexpectedReceiptReason) -> &'static str {
    match reason {
        UnexpectedReceiptReason::Excess => "Excess quantity",
        UnexpectedReceiptReason::UnexpectedItem => "Unexpected item",
        UnexpectedReceiptReason::BlindReceipt => "Blind receipt",
        UnexpectedReceiptReason::MisShipped => "Mis-shipped",
        UnexpectedReceiptReason::Other => "Other",
    }
}
