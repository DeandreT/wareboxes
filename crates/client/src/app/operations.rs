use super::*;

impl WareboxesApp {
    // ---- Inventory -------------------------------------------------------
    pub(super) fn inventory_screen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("New item batch", |ui| {
            let item_options = self.item_options();
            let load_options = self.load_options();
            ui.horizontal_wrapped(|ui| {
                let item_key = "inventory:new:item_id".to_owned();
                let load_key = "inventory:new:load_id".to_owned();
                let lot_key = "inventory:new:lot".to_owned();
                let mut item_id = self
                    .forms
                    .drafts
                    .get(&item_key)
                    .cloned()
                    .unwrap_or_default();
                let mut load_id = self
                    .forms
                    .drafts
                    .get(&load_key)
                    .cloned()
                    .unwrap_or_default();
                let mut lot = self.forms.drafts.get(&lot_key).cloned().unwrap_or_default();
                ui.label("Item");
                let selected_item_id = Self::entity_picker(
                    ui,
                    "inventory_new_item",
                    &mut item_id,
                    &item_options,
                    "Search item",
                );
                ui.label("Load");
                let selected_load_id = Self::entity_picker(
                    ui,
                    "inventory_new_load",
                    &mut load_id,
                    &load_options,
                    "Optional load",
                );
                ui.label("Lot");
                ui.text_edit_singleline(&mut lot);
                if ui.button("Create").clicked() {
                    if let Some(iid) = selected_item_id {
                        self.api.action(
                            "/api/inventory/batches/add",
                            json!({"item_id": iid, "load_id": selected_load_id, "lot": lot}),
                            Screen::Inventory,
                            "Batch created",
                        );
                    } else {
                        self.toast("Choose an item", true, self.now);
                    }
                }
                self.forms.drafts.insert(item_key, item_id);
                self.forms.drafts.insert(load_key, load_id);
                self.forms.drafts.insert(lot_key, lot);
            });
        });
        ui.separator();
        ui.heading("Batches");
        let batches = self.data.item_batches.clone();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::remainder().at_least(100.0))
            .column(Column::auto())
            .header(24.0, |mut h| {
                for label in ["ID", "Item", "Load", "Lot", "Actions"] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|mut body| {
                for batch in &batches {
                    body.row(26.0, |mut row| {
                        row.col(|ui| {
                            ui.label(batch.id.to_string());
                        });
                        row.col(|ui| {
                            ui.label(batch.item_id.to_string());
                        });
                        row.col(|ui| {
                            ui.label(
                                batch
                                    .load_id
                                    .map(|id| id.to_string())
                                    .unwrap_or_else(|| "-".to_owned()),
                            );
                        });
                        row.col(|ui| {
                            ui.label(batch.lot.clone().unwrap_or_default());
                        });
                        row.col(|ui| {
                            if batch.deleted.is_none() {
                                self.delete_button(
                                    ui,
                                    true,
                                    format!("inventory batch #{}", batch.id),
                                    "/api/inventory/batches/delete",
                                    json!({"item_batch_id": batch.id}),
                                    Screen::Inventory,
                                    "Batch deleted",
                                );
                            }
                        });
                    });
                }
            });

        ui.separator();
        ui.heading("Movements");
        for movement in &self.data.movements {
            ui.label(format!(
                "#{} {} batch {}: {} -> {}, qty {}",
                movement.id,
                movement.movement_type,
                movement.item_batch_id,
                movement
                    .from_location_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "external".to_owned()),
                movement
                    .to_location_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "external".to_owned()),
                movement.qty
            ));
        }
    }

    // ---- License Plates -------------------------------------------------
    pub(super) fn license_plates_screen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("New license plate", |ui| {
            ui.horizontal(|ui| {
                ui.label("Barcode");
                ui.text_edit_singleline(&mut self.forms.new_barcode);
                if ui.button("Create").clicked() {
                    self.api.action(
                        "/api/license-plates/add",
                        json!({"barcode": self.forms.new_barcode}),
                        Screen::LicensePlates,
                        "License plate created",
                    );
                    self.forms.new_barcode.clear();
                }
            });
        });
        ui.separator();

        ui.collapsing("Lookup / move", |ui| {
            let lookup_key = "license_plate:lookup".to_owned();
            let mut lookup = self
                .forms
                .drafts
                .get(&lookup_key)
                .cloned()
                .unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label("Barcode");
                ui.text_edit_singleline(&mut lookup);
                if ui.button("Lookup").clicked() && !lookup.trim().is_empty() {
                    self.api.get_license_plate_by_barcode(&lookup);
                }
            });
            self.forms.drafts.insert(lookup_key, lookup);
            if let Some(lp) = &self.data.license_plate_lookup {
                ui.group(|ui| {
                    ui.strong(format!(
                        "Found #{} {}",
                        lp.id,
                        lp.barcode.clone().unwrap_or_default()
                    ));
                    ui.label(format!(
                        "Location: {}",
                        lp.location_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "-".to_owned())
                    ));
                    for content in &lp.contents {
                        ui.label(format!(
                            "batch {} status {} on hand {} reserved {} at loc {}",
                            content.item_batch_id,
                            content.status,
                            content.qty_on_hand,
                            content.qty_reserved,
                            content.location_id
                        ));
                    }
                });
            }
        });
        ui.separator();

        let license_plates = self.data.license_plates.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for lp in &license_plates {
                ui.group(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(format!(
                            "#{} {}",
                            lp.id,
                            lp.barcode.clone().unwrap_or_default()
                        ));
                        ui.label(format!(
                            "location {}",
                            lp.location_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "-".to_owned())
                        ));
                        if lp.deleted.is_some() {
                            ui.colored_label(egui::Color32::LIGHT_RED, "(deleted)");
                        }
                    });
                    if lp.contents.is_empty() {
                        ui.weak("No inventory on this license plate.");
                    } else {
                        for content in &lp.contents {
                            ui.label(format!(
                                "balance {} batch {} status {}: on hand {}, reserved {}, loc {}",
                                content.inventory_balance_id,
                                content.item_batch_id,
                                content.status,
                                content.qty_on_hand,
                                content.qty_reserved,
                                content.location_id
                            ));
                        }
                    }
                    ui.horizontal_wrapped(|ui| {
                        let dest_key = format!("license_plate:{}:dest", lp.id);
                        let reason_key = format!("license_plate:{}:reason", lp.id);
                        let location_options = self.location_options();
                        let mut dest = self
                            .forms
                            .drafts
                            .get(&dest_key)
                            .cloned()
                            .unwrap_or_default();
                        let mut reason = self
                            .forms
                            .drafts
                            .get(&reason_key)
                            .cloned()
                            .unwrap_or_else(|| "license plate move".to_owned());
                        ui.label("Move to location");
                        let to_location_id = Self::entity_picker(
                            ui,
                            ("license_plate_dest", lp.id),
                            &mut dest,
                            &location_options,
                            "Search location",
                        );
                        ui.label("Reason");
                        ui.text_edit_singleline(&mut reason);
                        if ui.small_button("Move LP").clicked() {
                            if let Some(to_location_id) = to_location_id {
                                self.api.action(
                                    "/api/license-plates/move",
                                    json!({
                                        "license_plate_id": lp.id,
                                        "to_location_id": to_location_id,
                                        "reason": reason.clone(),
                                    }),
                                    Screen::LicensePlates,
                                    "License plate moved",
                                );
                            } else {
                                self.toast("Choose a destination location", true, self.now);
                            }
                        }
                        self.forms.drafts.insert(dest_key, dest);
                        self.forms.drafts.insert(reason_key, reason);
                        if lp.deleted.is_none() {
                            self.delete_button(
                                ui,
                                true,
                                format!(
                                    "license plate {}",
                                    lp.barcode.as_deref().unwrap_or("unnamed")
                                ),
                                "/api/license-plates/delete",
                                json!({"license_plate_id": lp.id}),
                                Screen::LicensePlates,
                                "License plate deleted",
                            );
                        } else if ui.small_button("Restore").clicked() {
                            self.api.action(
                                "/api/license-plates/restore",
                                json!({"license_plate_id": lp.id}),
                                Screen::LicensePlates,
                                "License plate restored",
                            );
                        }
                    });
                });
            }
        });
    }

    // ---- Employees -------------------------------------------------------
    pub(super) fn employees_screen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("New employee", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("First");
                ui.text_edit_singleline(&mut self.forms.new_first_name);
                ui.label("Last");
                ui.text_edit_singleline(&mut self.forms.new_last_name);
                ui.label("Title");
                ui.text_edit_singleline(&mut self.forms.new_title);
                ui.label("Type");
                ui.text_edit_singleline(&mut self.forms.new_type);
                if ui.button("Create").clicked() {
                    self.api.action(
                        "/api/employees/add",
                        json!({
                            "first_name": self.forms.new_first_name,
                            "last_name": self.forms.new_last_name,
                            "title": self.forms.new_title,
                            "type": self.forms.new_type,
                        }),
                        Screen::Employees,
                        "Employee created",
                    );
                    self.forms.new_first_name.clear();
                    self.forms.new_last_name.clear();
                    self.forms.new_title.clear();
                }
            });
        });
        ui.separator();

        let employees = self.data.employees.clone();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto())
            .column(Column::initial(180.0).clip(true))
            .column(Column::initial(160.0).clip(true))
            .column(Column::initial(100.0))
            .column(Column::auto())
            .header(26.0, |mut h| {
                for label in ["ID", "Name", "Title", "Type", "Actions"] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|mut body| {
                for e in &employees {
                    body.row(28.0, |mut row| {
                        row.col(|ui| {
                            ui.label(e.id.to_string());
                        });
                        row.col(|ui| {
                            ui.label(format!("{} {}", e.first_name, e.last_name));
                        });
                        row.col(|ui| {
                            ui.label(&e.title);
                        });
                        row.col(|ui| {
                            ui.label(&e.r#type);
                        });
                        row.col(|ui| {
                            if e.deleted.is_none() {
                                self.delete_button(
                                    ui,
                                    true,
                                    format!("employee {} {}", e.first_name, e.last_name),
                                    "/api/employees/delete",
                                    json!({"employee_id": e.id}),
                                    Screen::Employees,
                                    "Employee deleted",
                                );
                            } else if ui.small_button("Restore").clicked() {
                                self.api.action(
                                    "/api/employees/restore",
                                    json!({"employee_id": e.id}),
                                    Screen::Employees,
                                    "Employee restored",
                                );
                            }
                        });
                    });
                }
            });
    }

    // ---- Audits ----------------------------------------------------------
    pub(super) fn audits_screen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("New audit wave", |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut self.forms.new_name);
                ui.label("Description");
                ui.text_edit_singleline(&mut self.forms.new_desc);
                if ui.button("Create").clicked() {
                    self.api.action(
                        "/api/audits/add",
                        json!({"name": self.forms.new_name, "description": self.forms.new_desc}),
                        Screen::Audits,
                        "Audit wave created",
                    );
                    self.forms.new_name.clear();
                    self.forms.new_desc.clear();
                }
            });
        });
        ui.separator();

        let audits = self.data.audits.clone();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto())
            .column(Column::initial(180.0).clip(true))
            .column(Column::remainder().at_least(160.0).clip(true))
            .column(Column::auto())
            .header(26.0, |mut h| {
                for label in ["ID", "Name", "Description", "Actions"] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|mut body| {
                for audit in &audits {
                    body.row(28.0, |mut row| {
                        row.col(|ui| {
                            ui.label(audit.id.to_string());
                        });
                        row.col(|ui| {
                            ui.label(audit.name.clone().unwrap_or_default());
                        });
                        row.col(|ui| {
                            ui.label(audit.description.clone().unwrap_or_default());
                        });
                        row.col(|ui| {
                            if audit.deleted.is_none() {
                                self.delete_button(
                                    ui,
                                    true,
                                    format!(
                                        "audit wave {}",
                                        audit.name.as_deref().unwrap_or("unnamed")
                                    ),
                                    "/api/audits/delete",
                                    json!({"audit_wave_id": audit.id}),
                                    Screen::Audits,
                                    "Audit wave deleted",
                                );
                            } else if ui.small_button("Restore").clicked() {
                                self.api.action(
                                    "/api/audits/restore",
                                    json!({"audit_wave_id": audit.id}),
                                    Screen::Audits,
                                    "Audit wave restored",
                                );
                            }
                        });
                    });
                }
            });
    }
}
