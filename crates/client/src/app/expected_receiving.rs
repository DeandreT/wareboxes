use wareboxes_api_contract::v1::{
    ExpectedReceiptDisposition, ExpectedReceiptExceptionReason, ExpectedReceiptLine,
    ExpectedReceivingSessionResponse,
};

use super::*;
use crate::expected_receiving_workflow::{ExpectedReceivingActivity, ExpectedReceivingScanStage};

impl WareboxesApp {
    pub(super) fn expected_receiving_screen(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(Self::icon(Icon::ScanBarcode).color(Self::accent_color(ui)));
                    ui.heading("Expected receiving");
                    if let Some(session) = self.expected_receiving.session() {
                        ui.separator();
                        ui.weak(
                            session.reference_number.as_deref().map_or_else(
                                || format!("Load #{}", session.load_id),
                                str::to_owned,
                            ),
                        );
                    }
                });
                ui.separator();

                self.expected_receiving_request_state(ui);
                if let Some(session) = self.expected_receiving.session().cloned() {
                    self.expected_receiving_active(ui, &session);
                } else if matches!(
                    self.expected_receiving.activity(),
                    ExpectedReceivingActivity::Uninitialized | ExpectedReceivingActivity::Ready
                ) {
                    self.expected_receiving_load_prompt(ui);
                }
            });
    }

    fn expected_receiving_request_state(&mut self, ui: &mut egui::Ui) {
        match self.expected_receiving.activity() {
            ExpectedReceivingActivity::Pending => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(18.0));
                    ui.strong(if self.expected_receiving.session().is_some() {
                        "Updating receipt"
                    } else {
                        "Loading receipt"
                    });
                });
                ui.add_space(6.0);
            }
            ExpectedReceivingActivity::Retryable => {
                self.expected_receiving_error_band(ui, false);
            }
            ExpectedReceivingActivity::ReconcileRequired => {
                self.expected_receiving_error_band(ui, true);
            }
            ExpectedReceivingActivity::Uninitialized | ExpectedReceivingActivity::Ready => {}
        }
    }

    fn expected_receiving_error_band(&mut self, ui: &mut egui::Ui, reconcile: bool) {
        egui::Frame::none()
            .fill(if ui.visuals().dark_mode {
                egui::Color32::from_rgb(70, 30, 32)
            } else {
                egui::Color32::from_rgb(255, 236, 235)
            })
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .rounding(egui::Rounding::same(4.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(Self::icon(Icon::AlertTriangle).color(Self::danger_text_color(ui)));
                    ui.label(
                        self.expected_receiving
                            .request_error()
                            .unwrap_or("Expected receiving request failed"),
                    );
                    let clicked = if reconcile {
                        ui.button("Reconcile").clicked()
                    } else {
                        ui.button("Retry").clicked()
                    };
                    if clicked {
                        let request_id = Self::new_expected_receiving_request_id();
                        let request = if reconcile {
                            self.expected_receiving.reconcile(request_id)
                        } else {
                            self.expected_receiving.retry(request_id)
                        };
                        if let Some(request) = request {
                            self.api.execute_expected_receiving(request);
                        }
                    }
                    if reconcile && ui.button("New load").clicked() {
                        self.expected_receiving.reset_for_next_load();
                    }
                });
            });
        ui.add_space(8.0);
    }

    fn expected_receiving_load_prompt(&mut self, ui: &mut egui::Ui) {
        if let Some(message) = self.expected_receiving.completion() {
            egui::Frame::none()
                .fill(if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(22, 68, 55)
                } else {
                    egui::Color32::from_rgb(226, 247, 238)
                })
                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                .rounding(egui::Rounding::same(4.0))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            Self::icon(Icon::CheckCircle2).color(Self::success_text_color(ui)),
                        );
                        ui.strong(message);
                    });
                });
            ui.add_space(10.0);
        }
        self.expected_receiving_scan_control(ui);
        if let Some(error) = self.expected_receiving.request_error() {
            ui.colored_label(Self::danger_text_color(ui), error);
        }
    }

    fn expected_receiving_active(
        &mut self,
        ui: &mut egui::Ui,
        session: &ExpectedReceivingSessionResponse,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.weak(format!(
                "Owner #{} | Facility #{}",
                session.inventory_owner_id, session.facility_id
            ));
            if self.expected_receiving.activity() == ExpectedReceivingActivity::Ready
                && ui.button("New load").clicked()
            {
                self.expected_receiving.reset_for_next_load();
            }
        });
        ui.add_space(6.0);
        self.expected_receiving_dock_band(ui, session);
        ui.add_space(8.0);
        self.expected_receiving_line_list(ui, session);
        ui.add_space(10.0);

        if self.expected_receiving.activity() != ExpectedReceivingActivity::Ready {
            return;
        }

        self.expected_receiving_disposition_control(ui);
        ui.add_space(8.0);
        self.expected_receiving_scan_control(ui);

        if self.expected_receiving.active_line().is_some()
            && self.expected_receiving.scan_stage().is_none()
        {
            self.expected_receiving_confirmation_fields(ui);
        } else if self.expected_receiving.scan_stage().is_none() {
            ui.strong("Select an open line");
        }
    }

    fn expected_receiving_dock_band(
        &self,
        ui: &mut egui::Ui,
        session: &ExpectedReceivingSessionResponse,
    ) {
        egui::Frame::none()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .rounding(egui::Rounding::same(4.0))
            .show(ui, |ui| {
                ui.weak("RECEIVING DOCK");
                ui.label(
                    egui::RichText::new(
                        session
                            .receiving_location
                            .name
                            .as_deref()
                            .unwrap_or(&session.receiving_location.barcode),
                    )
                    .size(18.0)
                    .strong(),
                );
                if session.receiving_location.name.as_deref()
                    != Some(session.receiving_location.barcode.as_str())
                {
                    ui.monospace(&session.receiving_location.barcode);
                }
            });
    }

    fn expected_receiving_line_list(
        &mut self,
        ui: &mut egui::Ui,
        session: &ExpectedReceivingSessionResponse,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Open lines");
            ui.weak(format!("{}", session.lines.len()));
        });
        ui.add_space(4.0);

        for line in &session.lines {
            let selected = self.expected_receiving.selected_line_id() == Some(line.load_line_id);
            let fill = if selected {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().extreme_bg_color
            };
            let framed = egui::Frame::none()
                .fill(fill)
                .inner_margin(egui::Margin::symmetric(10.0, 7.0))
                .rounding(egui::Rounding::same(4.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(
                            line.item_description
                                .as_deref()
                                .map_or_else(|| format!("Item #{}", line.item_id), str::to_owned),
                        );
                        ui.separator();
                        ui.label(format!(
                            "{} {} remaining",
                            line.remaining_quantity, line.uom
                        ));
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.weak(format!("Line #{}", line.load_line_id));
                        ui.label(format!("Expected {}", line.expected_quantity));
                        if line.received_quantity > 0 {
                            ui.label(format!("Received {}", line.received_quantity));
                        }
                        if line.rejected_quantity > 0 {
                            ui.label(format!("Rejected {}", line.rejected_quantity));
                        }
                        if line.missing_quantity > 0 {
                            ui.label(format!("Missing {}", line.missing_quantity));
                        }
                    });
                    self.expected_receiving_dimensions(ui, line);
                });
            let response = ui.interact(
                framed.response.rect,
                ui.id().with(("expected_receiving_line", line.load_line_id)),
                egui::Sense::click(),
            );
            if response.clicked() {
                self.expected_receiving.select_line(line.load_line_id);
            }
            ui.add_space(4.0);
        }
    }

    fn expected_receiving_dimensions(&self, ui: &mut egui::Ui, line: &ExpectedReceiptLine) {
        if line.lot.is_none() && line.serial.is_none() && line.expiration.is_none() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(lot) = line.lot.as_deref() {
                ui.weak(format!("Lot {lot}"));
            }
            if let Some(serial) = line.serial.as_deref() {
                ui.weak(format!("Serial {serial}"));
            }
            if let Some(expiration) = line.expiration.as_deref() {
                ui.weak(format!("Expires {expiration}"));
            }
        });
    }

    fn expected_receiving_disposition_control(&mut self, ui: &mut egui::Ui) {
        let current = self.expected_receiving.disposition();
        let mut selected = current;
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut selected,
                ExpectedReceiptDisposition::Received,
                "Received",
            );
            ui.selectable_value(
                &mut selected,
                ExpectedReceiptDisposition::Rejected,
                "Rejected",
            );
            ui.selectable_value(
                &mut selected,
                ExpectedReceiptDisposition::Missing,
                "Missing",
            );
        });
        if selected != current {
            self.expected_receiving.select_disposition(selected);
        }
    }

    fn expected_receiving_scan_control(&mut self, ui: &mut egui::Ui) {
        let Some(stage) = self.expected_receiving.scan_stage() else {
            return;
        };
        let (label, hint) = match stage {
            ExpectedReceivingScanStage::LoadId => ("Scan load", "Load ID"),
            ExpectedReceivingScanStage::ItemBarcode => ("Scan item", "Item barcode"),
            ExpectedReceivingScanStage::ReceivingLocation => {
                ("Scan receiving dock", "Dock barcode")
            }
        };
        ui.strong(label);
        ui.add_space(4.0);

        let width = ui.available_width().min(520.0);
        let response = match stage {
            ExpectedReceivingScanStage::LoadId => ui.add_sized(
                [width, 42.0],
                egui::TextEdit::singleline(self.expected_receiving.load_id_draft_mut())
                    .id(egui::Id::new("expected_receiving_scan"))
                    .hint_text(hint)
                    .font(egui::TextStyle::Heading),
            ),
            ExpectedReceivingScanStage::ItemBarcode
            | ExpectedReceivingScanStage::ReceivingLocation => ui.add_sized(
                [width, 42.0],
                egui::TextEdit::singleline(self.expected_receiving.scan_draft_mut())
                    .id(egui::Id::new("expected_receiving_scan"))
                    .hint_text(hint)
                    .font(egui::TextStyle::Heading),
            ),
        };
        if !response.has_focus() {
            response.request_focus();
        }
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        let submit = (enter && (response.has_focus() || response.lost_focus()))
            || ui
                .add_sized([140.0, 34.0], egui::Button::new("Submit scan"))
                .clicked();
        if let Some(error) = self.expected_receiving.scan_error() {
            ui.colored_label(Self::danger_text_color(ui), error);
        }
        if submit {
            let request = self.expected_receiving.submit_scan(
                Self::new_expected_receiving_request_id(),
                Self::new_expected_receiving_idempotency_key(),
            );
            if let Some(request) = request {
                self.api.execute_expected_receiving(request);
            }
        }
        ui.add_space(8.0);
    }

    fn expected_receiving_confirmation_fields(&mut self, ui: &mut egui::Ui) {
        let disposition = self.expected_receiving.disposition();
        ui.strong("Quantity");
        ui.add_sized(
            [ui.available_width().min(180.0), 34.0],
            egui::TextEdit::singleline(self.expected_receiving.quantity_draft_mut())
                .hint_text("Quantity"),
        );
        ui.add_space(5.0);

        match disposition {
            ExpectedReceiptDisposition::Received => {
                self.expected_receiving_received_fields(ui);
            }
            ExpectedReceiptDisposition::Rejected | ExpectedReceiptDisposition::Missing => {
                self.expected_receiving_exception_fields(ui);
            }
        }

        if let Some(error) = self.expected_receiving.request_error() {
            ui.colored_label(Self::danger_text_color(ui), error);
        }
        ui.add_space(6.0);
        if ui
            .add_sized(
                [180.0, 38.0],
                egui::Button::new(egui::RichText::new("Confirm receipt").strong()),
            )
            .clicked()
        {
            let request = self.expected_receiving.begin_confirmation(
                Self::new_expected_receiving_request_id(),
                Self::new_expected_receiving_idempotency_key(),
            );
            if let Some(request) = request {
                self.api.execute_expected_receiving(request);
            }
        }
    }

    fn expected_receiving_received_fields(&mut self, ui: &mut egui::Ui) {
        Self::expected_receiving_text_field(
            ui,
            "License plate",
            "Optional license plate",
            self.expected_receiving.license_plate_draft_mut(),
        );
        Self::expected_receiving_text_field(
            ui,
            "Lot",
            "Optional lot",
            self.expected_receiving.lot_draft_mut(),
        );
        Self::expected_receiving_text_field(
            ui,
            "Serial",
            "Optional serial",
            self.expected_receiving.serial_draft_mut(),
        );
        Self::expected_receiving_text_field(
            ui,
            "Expiration",
            "RFC 3339 timestamp",
            self.expected_receiving.expiration_draft_mut(),
        );
    }

    fn expected_receiving_text_field(
        ui: &mut egui::Ui,
        label: &str,
        hint: &str,
        draft: &mut String,
    ) {
        ui.strong(label);
        ui.add_sized(
            [ui.available_width().min(360.0), 32.0],
            egui::TextEdit::singleline(draft).hint_text(hint),
        );
        ui.add_space(4.0);
    }

    fn expected_receiving_exception_fields(&mut self, ui: &mut egui::Ui) {
        let current = self.expected_receiving.reason();
        let mut selected = current;
        ui.strong("Reason");
        egui::ComboBox::from_id_source("expected_receiving_exception_reason")
            .selected_text(Self::expected_receiving_reason_label(current))
            .show_ui(ui, |ui| {
                for reason in [
                    ExpectedReceiptExceptionReason::Damaged,
                    ExpectedReceiptExceptionReason::QualityRejected,
                    ExpectedReceiptExceptionReason::ShortShipment,
                    ExpectedReceiptExceptionReason::CountDiscrepancy,
                    ExpectedReceiptExceptionReason::WrongItem,
                    ExpectedReceiptExceptionReason::Other,
                ] {
                    ui.selectable_value(
                        &mut selected,
                        reason,
                        Self::expected_receiving_reason_label(reason),
                    );
                }
            });
        if selected != current {
            self.expected_receiving.select_reason(selected);
        }
        ui.add_space(4.0);
        ui.strong("Note");
        ui.add_sized(
            [ui.available_width().min(520.0), 56.0],
            egui::TextEdit::multiline(self.expected_receiving.note_draft_mut())
                .char_limit(1_000)
                .hint_text(if selected == ExpectedReceiptExceptionReason::Other {
                    "Required"
                } else {
                    "Optional"
                }),
        );
    }

    fn expected_receiving_reason_label(reason: ExpectedReceiptExceptionReason) -> &'static str {
        match reason {
            ExpectedReceiptExceptionReason::Damaged => "Damaged",
            ExpectedReceiptExceptionReason::QualityRejected => "Quality rejected",
            ExpectedReceiptExceptionReason::ShortShipment => "Short shipment",
            ExpectedReceiptExceptionReason::CountDiscrepancy => "Count discrepancy",
            ExpectedReceiptExceptionReason::WrongItem => "Wrong item",
            ExpectedReceiptExceptionReason::Other => "Other",
        }
    }

    pub(super) fn new_expected_receiving_request_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn new_expected_receiving_idempotency_key() -> String {
        format!("rf-expected-receiving-{}", uuid::Uuid::new_v4())
    }
}
