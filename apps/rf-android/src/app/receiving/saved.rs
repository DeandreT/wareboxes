use eframe::egui;

use crate::expected_receiving::{ConfirmationMode, ExpectedReceiptLine};

use super::{RfApp, confirmation_mode_label, exception_reason_label, receiving_draft_snapshot};

impl RfApp {
    pub(super) fn receiving_saved_draft(&self, ui: &mut egui::Ui) {
        let Some(draft) = receiving_draft_snapshot(&self.receiving) else {
            return;
        };
        let line = self.receiving.selected_line();
        let uom = line.map(|line| line.uom().as_str()).unwrap_or("units");
        let item = draft.item_barcode.as_deref().map_or_else(
            || {
                line.and_then(ExpectedReceiptLine::item_description)
                    .map_or_else(
                        || {
                            draft.selected_line_id.map_or_else(
                                || "Expected item".to_owned(),
                                |line_id| format!("Line {}", line_id.get()),
                            )
                        },
                        str::to_owned,
                    )
            },
            |barcode| format!("Item {barcode}"),
        );
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_min_width((width - 20.0).max(0.0));
                ui.label(egui::RichText::new("SAVED RECEIPT").small().strong());
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(confirmation_mode_label(draft.mode))
                            .strong()
                            .color(Self::accent()),
                    );
                    ui.separator();
                    ui.label(format!(
                        "{} {uom}",
                        draft
                            .quantity
                            .map_or_else(|| "?".to_owned(), |quantity| quantity.to_string())
                    ));
                    ui.separator();
                    ui.monospace(item);
                });
                match draft.mode {
                    ConfirmationMode::Received => {
                        ui.horizontal_wrapped(|ui| {
                            if let Some(dock) = draft.dock_barcode.as_deref() {
                                ui.monospace(format!("Dock {dock}"));
                            }
                            if let Some(license_plate) = draft.license_plate_barcode.as_deref() {
                                ui.separator();
                                ui.monospace(format!("LP {license_plate}"));
                            } else {
                                ui.separator();
                                ui.label("Loose");
                            }
                        });
                    }
                    ConfirmationMode::Rejected | ConfirmationMode::Missing => {
                        ui.horizontal_wrapped(|ui| {
                            if let Some(reason) = draft.reason {
                                ui.label(exception_reason_label(reason));
                            }
                            if let Some(note) = draft.note.as_deref() {
                                ui.separator();
                                ui.label(note);
                            }
                        });
                    }
                }
            });
        ui.add_space(8.0);
    }
}
