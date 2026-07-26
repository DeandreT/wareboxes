use super::*;

const RECEIPT_EXCEPTION_REASONS: [(&str, &str); 6] = [
    ("damaged", "Damaged"),
    ("quality_rejected", "Quality rejected"),
    ("short_shipment", "Short shipment"),
    ("count_discrepancy", "Count discrepancy"),
    ("wrong_item", "Wrong item"),
    ("other", "Other"),
];

#[derive(Debug, PartialEq, Eq)]
struct ReceiptStockDimensions {
    receiving_location_id: Option<i64>,
    license_plate_barcode: Option<String>,
    lot: Option<String>,
    serial: Option<String>,
    expiration: Option<chrono::DateTime<Utc>>,
}

impl WareboxesApp {
    pub(super) fn load_line_receipt_form(
        &mut self,
        ui: &mut egui::Ui,
        load: &Load,
        line: &LoadLine,
    ) {
        let resolved = line
            .received_qty
            .saturating_add(line.rejected_qty)
            .saturating_add(line.missing_qty);
        let remaining = line.expected_qty.saturating_sub(resolved);
        if !matches!(load.status, LoadStatus::Arrived | LoadStatus::Receiving)
            || load.deleted.is_some()
            || line.deleted.is_some()
            || remaining == 0
        {
            return;
        }

        let location_options = self
            .data
            .locations
            .iter()
            .filter(|location| {
                location.facility_id == load.facility_id
                    && location.active
                    && location.receivable
                    && location.deleted.is_none()
            })
            .map(|location| {
                let scan_code = location
                    .barcode
                    .as_deref()
                    .or(location.name.as_deref())
                    .unwrap_or("Unnamed location");
                (location.id, scan_code.to_owned())
            })
            .collect::<Vec<_>>();

        let key = |field: &str| format!("load-line:{}:receipt:{field}", line.id);
        let location_key = key("location");
        let received_key = key("received");
        let rejected_key = key("rejected");
        let missing_key = key("missing");
        let license_plate_key = key("license-plate");
        let lot_key = key("lot");
        let serial_key = key("serial");
        let expiration_key = key("expiration");
        let exception_reason_key = key("exception-reason");
        let exception_note_key = key("exception-note");

        let default_location = load
            .dock_door_location_id
            .and_then(|location_id| {
                location_options
                    .iter()
                    .find(|(id, _)| *id == location_id)
                    .map(|(_, label)| label.clone())
            })
            .unwrap_or_default();
        let mut location = self
            .forms
            .drafts
            .get(&location_key)
            .cloned()
            .unwrap_or(default_location);
        let mut received = self
            .forms
            .drafts
            .get(&received_key)
            .cloned()
            .unwrap_or_else(|| "0".to_owned());
        let mut rejected = self
            .forms
            .drafts
            .get(&rejected_key)
            .cloned()
            .unwrap_or_else(|| "0".to_owned());
        let mut missing = self
            .forms
            .drafts
            .get(&missing_key)
            .cloned()
            .unwrap_or_else(|| "0".to_owned());
        let mut license_plate_barcode = self
            .forms
            .drafts
            .get(&license_plate_key)
            .cloned()
            .unwrap_or_default();
        let mut lot = self
            .forms
            .drafts
            .get(&lot_key)
            .cloned()
            .unwrap_or_else(|| line.lot.clone().unwrap_or_default());
        let mut serial = self
            .forms
            .drafts
            .get(&serial_key)
            .cloned()
            .unwrap_or_else(|| line.serial.clone().unwrap_or_default());
        let mut expiration = self
            .forms
            .drafts
            .get(&expiration_key)
            .cloned()
            .unwrap_or_else(|| {
                line.expiration
                    .map(|value| value.format("%Y-%m-%d").to_string())
                    .unwrap_or_default()
            });
        let mut exception_reason = self
            .forms
            .drafts
            .get(&exception_reason_key)
            .cloned()
            .unwrap_or_default();
        let mut exception_note = self
            .forms
            .drafts
            .get(&exception_note_key)
            .cloned()
            .unwrap_or_default();

        ui.indent(("load_line_receipt", line.id), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Receive");
                ui.weak(format!("{remaining} remaining"));
                if location_options.is_empty() {
                    ui.colored_label(
                        Self::danger_text_color(ui),
                        "No receivable locations available",
                    );
                }
            });

            ui.horizontal_wrapped(|ui| {
                ui.label("Location");
                let selected_location_id = Self::entity_picker(
                    ui,
                    ("load_line_receipt_location", line.id),
                    &mut location,
                    &location_options,
                    "Scan or search location",
                )
                .filter(|selected_id| {
                    location_options
                        .iter()
                        .any(|(location_id, _)| location_id == selected_id)
                });
                ui.label("License plate");
                ui.add(
                    egui::TextEdit::singleline(&mut license_plate_barcode)
                        .desired_width(180.0)
                        .hint_text("Optional barcode"),
                );

                ui.label("Received");
                ui.add(
                    egui::TextEdit::singleline(&mut received)
                        .desired_width(64.0)
                        .hint_text("0"),
                );
                ui.label("Rejected");
                ui.add(
                    egui::TextEdit::singleline(&mut rejected)
                        .desired_width(64.0)
                        .hint_text("0"),
                );
                ui.label("Missing");
                ui.add(
                    egui::TextEdit::singleline(&mut missing)
                        .desired_width(64.0)
                        .hint_text("0"),
                );

                let parsed_rejected = Self::parse_receipt_quantity(&rejected);
                let parsed_missing = Self::parse_receipt_quantity(&missing);
                let needs_exception = parsed_rejected.is_some_and(|quantity| quantity > 0)
                    || parsed_missing.is_some_and(|quantity| quantity > 0);
                if needs_exception {
                    ui.label("Exception");
                    egui::ComboBox::from_id_source(("load_line_exception_reason", line.id))
                        .selected_text(
                            RECEIPT_EXCEPTION_REASONS
                                .iter()
                                .find(|(value, _)| *value == exception_reason)
                                .map(|(_, label)| *label)
                                .unwrap_or("Select reason"),
                        )
                        .show_ui(ui, |ui| {
                            for (value, label) in RECEIPT_EXCEPTION_REASONS {
                                ui.selectable_value(&mut exception_reason, value.to_owned(), label);
                            }
                        });
                }

                ui.label("Lot");
                ui.add(
                    egui::TextEdit::singleline(&mut lot)
                        .desired_width(120.0)
                        .hint_text("Optional"),
                );
                ui.label("Serial");
                ui.add(
                    egui::TextEdit::singleline(&mut serial)
                        .desired_width(120.0)
                        .hint_text("Optional"),
                );
                ui.label("Expiration");
                self.date_picker_ui(
                    ui,
                    &format!("load_line_{}_receipt_expiration", line.id),
                    &mut expiration,
                    "No Date",
                    None,
                    true,
                );

                if needs_exception {
                    ui.label("Exception note");
                    ui.add(
                        egui::TextEdit::singleline(&mut exception_note)
                            .desired_width(240.0)
                            .hint_text(if exception_reason == "other" {
                                "Required"
                            } else {
                                "Optional"
                            }),
                    );
                }

                if ui.button("Confirm receipt").clicked() {
                    let received_qty = Self::parse_receipt_quantity(&received);
                    let rejected_qty = Self::parse_receipt_quantity(&rejected);
                    let missing_qty = Self::parse_receipt_quantity(&missing);
                    let total = received_qty.zip(rejected_qty).zip(missing_qty).and_then(
                        |((received_qty, rejected_qty), missing_qty)| {
                            received_qty
                                .checked_add(rejected_qty)
                                .and_then(|total| total.checked_add(missing_qty))
                                .map(|total| (received_qty, rejected_qty, missing_qty, total))
                        },
                    );
                    let expiration_value = if received_qty.is_some_and(|quantity| quantity > 0) {
                        Self::receipt_expiration(&expiration)
                    } else {
                        Some(None)
                    };

                    match (total, expiration_value) {
                        (None, _) => self.toast(
                            "Receipt quantities must be non-negative whole numbers",
                            true,
                            self.now,
                        ),
                        (_, None) => self.toast("Expiration must be a valid date", true, self.now),
                        (Some((_, _, _, 0)), _) => {
                            self.toast("Enter a quantity to resolve", true, self.now)
                        }
                        (Some((_, _, _, total)), _) if total > remaining => self.toast(
                            format!("Receipt exceeds the {remaining} remaining units"),
                            true,
                            self.now,
                        ),
                        (Some((received_qty, _, _, _)), _)
                            if received_qty > 0 && selected_location_id.is_none() =>
                        {
                            self.toast(
                                "Choose a receivable location for received inventory",
                                true,
                                self.now,
                            );
                        }
                        (Some((_, rejected_qty, missing_qty, _)), _)
                            if (rejected_qty > 0 || missing_qty > 0)
                                && exception_reason.is_empty() =>
                        {
                            self.toast(
                                "Choose an exception reason for rejected or missing inventory",
                                true,
                                self.now,
                            );
                        }
                        (Some((_, rejected_qty, missing_qty, _)), _)
                            if (rejected_qty > 0 || missing_qty > 0)
                                && Self::receipt_other_note_missing(
                                    &exception_reason,
                                    &exception_note,
                                ) =>
                        {
                            self.toast(
                                "Enter an exception note when the reason is Other",
                                true,
                                self.now,
                            );
                        }
                        (
                            Some((received_qty, rejected_qty, missing_qty, _)),
                            Some(expiration_value),
                        ) => {
                            let path = format!("/api/inbound/load-lines/{}/receipts", line.id);
                            let has_exception = rejected_qty > 0 || missing_qty > 0;
                            let exception_reason_value = if has_exception {
                                Self::optional_receipt_text(&exception_reason)
                            } else {
                                None
                            };
                            let exception_note_value = if has_exception {
                                Self::optional_receipt_text(&exception_note)
                            } else {
                                None
                            };
                            let stock = Self::receipt_stock_dimensions(
                                received_qty,
                                selected_location_id,
                                &license_plate_barcode,
                                &lot,
                                &serial,
                                expiration_value,
                            );
                            self.api.action(
                                &path,
                                json!({
                                    "receiving_location_id": stock.receiving_location_id,
                                    "received_qty": received_qty,
                                    "rejected_qty": rejected_qty,
                                    "missing_qty": missing_qty,
                                    "license_plate_barcode": stock.license_plate_barcode,
                                    "lot": stock.lot,
                                    "serial": stock.serial,
                                    "expiration": stock.expiration,
                                    "exception_reason": exception_reason_value,
                                    "exception_note": exception_note_value,
                                }),
                                Screen::Loads,
                                "Receipt confirmed",
                            );
                            received = "0".to_owned();
                            rejected = "0".to_owned();
                            missing = "0".to_owned();
                            exception_reason.clear();
                            exception_note.clear();
                        }
                    }
                }
            });
        });

        self.forms.drafts.insert(location_key, location);
        self.forms.drafts.insert(received_key, received);
        self.forms.drafts.insert(rejected_key, rejected);
        self.forms.drafts.insert(missing_key, missing);
        self.forms
            .drafts
            .insert(license_plate_key, license_plate_barcode);
        self.forms.drafts.insert(lot_key, lot);
        self.forms.drafts.insert(serial_key, serial);
        self.forms.drafts.insert(expiration_key, expiration);
        self.forms
            .drafts
            .insert(exception_reason_key, exception_reason);
        self.forms.drafts.insert(exception_note_key, exception_note);
    }

    fn parse_receipt_quantity(value: &str) -> Option<i64> {
        let value = value.trim();
        if value.is_empty() {
            return Some(0);
        }
        value.parse::<i64>().ok().filter(|quantity| *quantity >= 0)
    }

    fn optional_receipt_text(value: &str) -> Option<String> {
        (!value.trim().is_empty()).then(|| value.trim().to_owned())
    }

    fn receipt_other_note_missing(reason: &str, note: &str) -> bool {
        reason == "other" && note.trim().is_empty()
    }

    fn receipt_stock_dimensions(
        received_qty: i64,
        receiving_location_id: Option<i64>,
        license_plate_barcode: &str,
        lot: &str,
        serial: &str,
        expiration: Option<chrono::DateTime<Utc>>,
    ) -> ReceiptStockDimensions {
        if received_qty == 0 {
            return ReceiptStockDimensions {
                receiving_location_id: None,
                license_plate_barcode: None,
                lot: None,
                serial: None,
                expiration: None,
            };
        }

        ReceiptStockDimensions {
            receiving_location_id,
            license_plate_barcode: Self::optional_receipt_text(license_plate_barcode),
            lot: Self::optional_receipt_text(lot),
            serial: Self::optional_receipt_text(serial),
            expiration,
        }
    }

    fn receipt_expiration(value: &str) -> Option<Option<chrono::DateTime<Utc>>> {
        let value = value.trim();
        if value.is_empty() {
            return Some(None);
        }
        Self::parse_filter_date(value)
            .map(|date| Some(Utc.from_utc_datetime(&date.and_time(NaiveTime::MIN))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_quantity_accepts_zero_and_positive_whole_numbers() {
        assert_eq!(WareboxesApp::parse_receipt_quantity(""), Some(0));
        assert_eq!(WareboxesApp::parse_receipt_quantity(" 0 "), Some(0));
        assert_eq!(WareboxesApp::parse_receipt_quantity("42"), Some(42));
        assert_eq!(WareboxesApp::parse_receipt_quantity("-1"), None);
        assert_eq!(WareboxesApp::parse_receipt_quantity("1.5"), None);
    }

    #[test]
    fn receipt_optional_text_trims_and_omits_blank_values() {
        assert_eq!(WareboxesApp::optional_receipt_text("  "), None);
        assert_eq!(
            WareboxesApp::optional_receipt_text("  short shipment "),
            Some("short shipment".to_owned())
        );
    }

    #[test]
    fn discrepancy_only_receipts_omit_stock_dimensions() {
        let expiration = WareboxesApp::receipt_expiration("2026-08-03").flatten();
        let stock = WareboxesApp::receipt_stock_dimensions(
            0,
            Some(41),
            " LP-100 ",
            " LOT-1 ",
            " SERIAL-1 ",
            expiration,
        );

        assert_eq!(
            stock,
            ReceiptStockDimensions {
                receiving_location_id: None,
                license_plate_barcode: None,
                lot: None,
                serial: None,
                expiration: None,
            }
        );
    }

    #[test]
    fn other_exception_reason_requires_a_note() {
        assert!(WareboxesApp::receipt_other_note_missing("other", " "));
        assert!(!WareboxesApp::receipt_other_note_missing(
            "other",
            "Inventory arrived wet"
        ));
        assert!(!WareboxesApp::receipt_other_note_missing("damaged", ""));
    }

    #[test]
    fn receipt_expiration_serializes_at_utc_midnight() {
        let expiration = WareboxesApp::receipt_expiration("2026-08-03")
            .flatten()
            .map(|value| value.to_rfc3339());

        assert_eq!(expiration.as_deref(), Some("2026-08-03T00:00:00+00:00"));
        assert_eq!(WareboxesApp::receipt_expiration(""), Some(None));
        assert_eq!(WareboxesApp::receipt_expiration("not-a-date"), None);
    }
}
