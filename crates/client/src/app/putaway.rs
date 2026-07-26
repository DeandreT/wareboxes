use wareboxes_api_contract::v1::{
    PutawayClaimReleaseReason, PutawayClaimResponse, PutawayClaimWork, PutawayWorkflow,
};

use super::*;
use crate::putaway_workflow::{PutawayActivity, PutawayScanStage};

impl WareboxesApp {
    pub(super) fn putaway_screen(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(Self::icon(Icon::ScanBarcode).color(Self::accent_color(ui)));
            ui.heading("Directed putaway");
            if let Some(claim) = self.putaway.claim() {
                ui.separator();
                ui.weak(format!("Task #{}", claim.task_id));
            }
        });
        ui.separator();

        self.putaway_request_state(ui);

        if let Some(claim) = self.putaway.claim().cloned() {
            self.putaway_active_work(ui, &claim);
        } else if self.putaway.activity() == PutawayActivity::Ready {
            self.putaway_queue(ui);
        }
    }

    fn putaway_request_state(&mut self, ui: &mut egui::Ui) {
        match self.putaway.activity() {
            PutawayActivity::Uninitialized | PutawayActivity::Pending => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(18.0));
                    ui.strong(if self.putaway.claim().is_some() {
                        "Updating putaway work"
                    } else {
                        "Loading putaway work"
                    });
                });
                ui.add_space(6.0);
            }
            PutawayActivity::Retryable => {
                self.putaway_error_band(ui, false);
            }
            PutawayActivity::ReconcileRequired => {
                self.putaway_error_band(ui, true);
            }
            PutawayActivity::Ready => {}
        }
    }

    fn putaway_error_band(&mut self, ui: &mut egui::Ui, reconcile: bool) {
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
                        self.putaway
                            .request_error()
                            .unwrap_or("Putaway request failed"),
                    );
                    let clicked = if reconcile {
                        ui.button("Reconcile").clicked()
                    } else {
                        ui.button("Retry").clicked()
                    };
                    if clicked {
                        let request = if reconcile {
                            self.putaway.begin_current(Self::new_putaway_request_id())
                        } else {
                            self.putaway.retry(Self::new_putaway_request_id())
                        };
                        if let Some(request) = request {
                            self.api.execute_putaway(request);
                        }
                    }
                });
            });
        ui.add_space(8.0);
    }

    fn putaway_queue(&mut self, ui: &mut egui::Ui) {
        if let Some(message) = self.putaway.completion() {
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
            ui.add_space(12.0);
        }

        ui.strong("Work queue");
        ui.add_space(6.0);
        let mut workflow = self.putaway.selected_workflow();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut workflow, PutawayWorkflow::Loose, "Loose inventory");
            ui.selectable_value(
                &mut workflow,
                PutawayWorkflow::LicensePlate,
                "License plate",
            );
        });
        self.putaway.select_workflow(workflow);
        ui.add_space(12.0);

        if self.putaway.no_work() {
            ui.weak("No eligible putaway work is available in this queue.");
            ui.add_space(8.0);
        }

        if ui
            .add_sized(
                [180.0, 42.0],
                egui::Button::new(egui::RichText::new("Claim next").strong()),
            )
            .clicked()
        {
            let request = self.putaway.begin_claim(
                Self::new_putaway_request_id(),
                Self::new_putaway_idempotency_key("claim"),
            );
            if let Some(request) = request {
                self.api.execute_putaway(request);
            }
        }

        ui.add_space(18.0);
        ui.separator();
        ui.add_space(8.0);
        ui.strong("Assigned task");
        ui.add_space(5.0);
        let response = ui.add_sized(
            [220.0, 38.0],
            egui::TextEdit::singleline(self.putaway.task_id_draft_mut())
                .hint_text("Scan or enter task ID"),
        );
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if enter
            || ui
                .add_sized([180.0, 36.0], egui::Button::new("Claim assigned task"))
                .clicked()
        {
            let request = self.putaway.begin_claim_by_id(
                Self::new_putaway_request_id(),
                Self::new_putaway_idempotency_key("claim-selected"),
            );
            if let Some(request) = request {
                self.api.execute_putaway(request);
            }
        }
        if let Some(error) = self.putaway.request_error() {
            ui.colored_label(Self::danger_text_color(ui), error);
        }
    }

    fn putaway_active_work(&mut self, ui: &mut egui::Ui, claim: &PutawayClaimResponse) {
        ui.horizontal(|ui| {
            ui.weak(format!(
                "Owner #{} | Facility #{}",
                claim.inventory_owner_id, claim.facility_id
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.putaway.activity() == PutawayActivity::Ready
                    && ui.button("Release work").clicked()
                {
                    self.putaway_release_reason = PutawayClaimReleaseReason::WorkInterrupted;
                    self.putaway_release_note.clear();
                    self.putaway_release_open = true;
                    self.putaway_release_error = None;
                }
            });
        });
        ui.add_space(6.0);
        self.putaway_location_band(
            ui,
            "SOURCE",
            &claim.source_location.name,
            claim.source_location.barcode.as_deref(),
        );
        ui.add_space(6.0);
        self.putaway_work_band(ui, claim);
        ui.add_space(6.0);
        self.putaway_location_band(
            ui,
            "DESTINATION",
            &claim.destination_location.name,
            Some(&claim.destination_location.barcode),
        );

        if let Some(instructions) = claim.instructions.as_deref() {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(instructions).strong());
        }
        ui.add_space(14.0);

        if let Some(error) = self.putaway.heartbeat_error() {
            ui.colored_label(
                Self::order_summary_color(ui, "processing"),
                format!("Claim renewal retrying: {error}"),
            );
            ui.add_space(8.0);
        }

        if self.putaway_release_open {
            self.putaway_release_form(ui);
            return;
        }

        let Some(stage) = self.putaway.expected_scan() else {
            return;
        };
        let (label, hint) = match stage {
            PutawayScanStage::SourceLocation => ("Scan source location", "Source barcode"),
            PutawayScanStage::LicensePlate => ("Scan license plate", "License plate barcode"),
            PutawayScanStage::DestinationLocation => {
                ("Scan destination location", "Destination barcode")
            }
        };
        ui.strong(label);
        ui.add_space(5.0);

        let response = ui.add_sized(
            [ui.available_width().min(520.0), 44.0],
            egui::TextEdit::singleline(self.putaway.scan_draft_mut())
                .id(egui::Id::new(("putaway_scan", claim.task_id, stage)))
                .hint_text(hint)
                .font(egui::TextStyle::Heading),
        );
        if !response.has_focus() {
            response.request_focus();
        }
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        let submit = (enter && (response.has_focus() || response.lost_focus()))
            || ui
                .add_sized([140.0, 36.0], egui::Button::new("Submit scan"))
                .clicked();

        if let Some(error) = self.putaway.scan_error() {
            ui.colored_label(Self::danger_text_color(ui), error);
        }

        if submit {
            let request = self.putaway.submit_scan(
                Self::new_putaway_request_id(),
                Self::new_putaway_idempotency_key("confirm"),
            );
            if let Some(request) = request {
                self.api.execute_putaway(request);
            }
        }
    }

    fn putaway_release_form(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::symmetric(12.0, 10.0))
            .rounding(egui::Rounding::same(4.0))
            .show(ui, |ui| {
                ui.strong("Release work to queue");
                ui.add_space(6.0);
                egui::ComboBox::from_id_source("putaway_release_reason")
                    .selected_text(Self::putaway_release_reason_label(
                        self.putaway_release_reason,
                    ))
                    .show_ui(ui, |ui| {
                        for reason in [
                            PutawayClaimReleaseReason::WorkInterrupted,
                            PutawayClaimReleaseReason::EquipmentUnavailable,
                            PutawayClaimReleaseReason::DestinationBlocked,
                            PutawayClaimReleaseReason::SafetyIssue,
                            PutawayClaimReleaseReason::Other,
                        ] {
                            ui.selectable_value(
                                &mut self.putaway_release_reason,
                                reason,
                                Self::putaway_release_reason_label(reason),
                            );
                        }
                    });
                ui.add_space(5.0);
                ui.add_sized(
                    [ui.available_width().min(520.0), 54.0],
                    egui::TextEdit::multiline(&mut self.putaway_release_note)
                        .char_limit(500)
                        .hint_text(
                            if self.putaway_release_reason == PutawayClaimReleaseReason::Other {
                                "Reason note (required)"
                            } else {
                                "Note (optional)"
                            },
                        ),
                );
                if let Some(error) = self.putaway_release_error.as_deref() {
                    ui.colored_label(Self::danger_text_color(ui), error);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        self.putaway_release_open = false;
                        self.putaway_release_reason = PutawayClaimReleaseReason::WorkInterrupted;
                        self.putaway_release_note.clear();
                        self.putaway_release_error = None;
                    }
                    if ui.button("Release").clicked() {
                        self.submit_putaway_release();
                    }
                });
            });
    }

    fn submit_putaway_release(&mut self) {
        let trimmed = self.putaway_release_note.trim();
        if self.putaway_release_reason == PutawayClaimReleaseReason::Other && trimmed.is_empty() {
            self.putaway_release_error = Some("A note is required for Other".into());
            return;
        }
        let note = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        let request = self.putaway.begin_release(
            Self::new_putaway_request_id(),
            Self::new_putaway_idempotency_key("release"),
            self.putaway_release_reason,
            note,
        );
        if let Some(request) = request {
            self.putaway_release_open = false;
            self.putaway_release_error = None;
            self.api.execute_putaway(request);
        }
    }

    fn putaway_location_band(
        &self,
        ui: &mut egui::Ui,
        label: &str,
        name: &Option<String>,
        barcode: Option<&str>,
    ) {
        egui::Frame::none()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::symmetric(12.0, 9.0))
            .rounding(egui::Rounding::same(4.0))
            .show(ui, |ui| {
                ui.weak(label);
                ui.label(
                    egui::RichText::new(name.as_deref().or(barcode).unwrap_or("Unnamed location"))
                        .size(20.0)
                        .strong(),
                );
                if let Some(barcode) = barcode {
                    if name.as_deref() != Some(barcode) {
                        ui.monospace(barcode);
                    }
                }
            });
    }

    fn putaway_work_band(&self, ui: &mut egui::Ui, claim: &PutawayClaimResponse) {
        egui::Frame::none()
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(egui::Margin::symmetric(12.0, 9.0))
            .rounding(egui::Rounding::same(4.0))
            .show(ui, |ui| match &claim.work {
                PutawayClaimWork::Loose {
                    item_id,
                    item_description,
                    uom,
                    lot,
                    serial,
                    inventory_status,
                    quantity,
                    ..
                } => {
                    ui.weak("LOOSE INVENTORY");
                    ui.label(
                        egui::RichText::new(
                            item_description
                                .as_deref()
                                .map(str::to_owned)
                                .unwrap_or_else(|| format!("Item #{item_id}")),
                        )
                        .size(18.0)
                        .strong(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("Quantity: {quantity} {uom}"));
                        ui.separator();
                        ui.label(format!("Status: {inventory_status:?}"));
                        if let Some(lot) = lot {
                            ui.separator();
                            ui.label(format!("Lot: {lot}"));
                        }
                        if let Some(serial) = serial {
                            ui.separator();
                            ui.label(format!("Serial: {serial}"));
                        }
                    });
                }
                PutawayClaimWork::LicensePlate {
                    license_plate_barcode,
                    planned_balance_count,
                    ..
                } => {
                    ui.weak("LICENSE PLATE");
                    ui.monospace(
                        egui::RichText::new(license_plate_barcode)
                            .size(20.0)
                            .strong(),
                    );
                    ui.label(format!("{planned_balance_count} inventory balances"));
                }
            });
    }

    pub(super) fn new_putaway_request_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn new_putaway_idempotency_key(operation: &str) -> String {
        format!("rf-putaway-{operation}-{}", uuid::Uuid::new_v4())
    }

    pub(super) fn drive_putaway_heartbeat(&mut self, now: f64) {
        let request = self.putaway.poll_heartbeat(
            now,
            Self::new_putaway_request_id(),
            Self::new_putaway_idempotency_key("heartbeat"),
        );
        if let Some(request) = request {
            self.api.execute_putaway(request);
        }
    }

    fn putaway_release_reason_label(reason: PutawayClaimReleaseReason) -> &'static str {
        match reason {
            PutawayClaimReleaseReason::WorkInterrupted => "Work interrupted",
            PutawayClaimReleaseReason::EquipmentUnavailable => "Equipment unavailable",
            PutawayClaimReleaseReason::DestinationBlocked => "Destination blocked",
            PutawayClaimReleaseReason::SafetyIssue => "Safety issue",
            PutawayClaimReleaseReason::Other => "Other",
        }
    }
}
