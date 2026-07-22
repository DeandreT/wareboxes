use super::*;

type FacilityInventoryTotals = (i64, i64, BTreeSet<i64>, usize);

impl WareboxesApp {
    // ---- Inventory -------------------------------------------------------
    pub(super) fn inventory_screen(&mut self, ui: &mut egui::Ui) {
        let total_on_hand: i64 = self
            .data
            .inventory_balances
            .iter()
            .filter(|balance| balance.deleted.is_none())
            .map(|balance| balance.qty_on_hand)
            .sum();
        let total_reserved: i64 = self
            .data
            .inventory_balances
            .iter()
            .filter(|balance| balance.deleted.is_none())
            .map(|balance| balance.qty_reserved)
            .sum();
        ui.horizontal_wrapped(|ui| {
            Self::summary_badge(ui, "On Hand", total_on_hand, Self::success_text_color(ui));
            Self::summary_badge(
                ui,
                "Reserved",
                total_reserved,
                Self::order_summary_color(ui, "processing"),
            );
            Self::summary_badge(
                ui,
                "Available",
                total_on_hand - total_reserved,
                Self::load_status_color(LoadStatus::Received),
            );
        });
        ui.separator();

        let tab_key = "inventory:tab".to_owned();
        let mut tab = self
            .forms
            .drafts
            .get(&tab_key)
            .cloned()
            .unwrap_or_else(|| "actions".to_owned());
        let inventory_tabs = [
            ("actions", "Actions"),
            ("balances", "Balances"),
            ("location", "By Location"),
            ("facility", "By Facility"),
            ("item", "By Item"),
            ("movements", "Movements"),
        ];
        if !inventory_tabs.iter().any(|(value, _)| *value == tab) {
            tab = "actions".to_owned();
        }
        ui.horizontal_wrapped(|ui| {
            for (value, label) in inventory_tabs {
                ui.selectable_value(&mut tab, value.to_owned(), label);
            }
        });
        self.forms.drafts.insert(tab_key, tab.clone());
        ui.separator();

        match tab.as_str() {
            "balances" => self.inventory_balances_tab(ui),
            "location" => self.inventory_location_summary_tab(ui),
            "facility" => self.inventory_facility_summary_tab(ui),
            "item" => self.inventory_item_summary_tab(ui),
            "movements" => self.inventory_movements_tab(ui),
            _ => self.inventory_actions_tab(ui),
        }
    }

    fn inventory_actions_tab(&mut self, ui: &mut egui::Ui) {
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

        ui.collapsing("Move inventory", |ui| {
            let item_options = self.item_options();
            let item_key = "inventory:move:item".to_owned();
            let source_key = "inventory:move:source_balance".to_owned();
            let reason_key = "inventory:move:reason".to_owned();
            let destination_count_key = "inventory:move:destination_count".to_owned();
            let mut item = self
                .forms
                .drafts
                .get(&item_key)
                .cloned()
                .unwrap_or_default();
            let mut source = self
                .forms
                .drafts
                .get(&source_key)
                .cloned()
                .unwrap_or_default();
            let mut reason = self
                .forms
                .drafts
                .get(&reason_key)
                .cloned()
                .unwrap_or_else(|| "inventory move".to_owned());
            let mut destination_count = self
                .forms
                .drafts
                .get(&destination_count_key)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(1)
                .clamp(1, 12);
            let item_before = item.clone();

            let move_scroll_height = ui.available_height().clamp(260.0, 640.0);
            egui::ScrollArea::vertical()
                .id_source("inventory_move_scroll")
                .auto_shrink([false, false])
                .max_height(move_scroll_height)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("Move one loose source balance into one or more locations.")
                            .color(Self::load_detail_text_color(ui)),
                    );
                    ui.add_space(6.0);

            let selected_item_id = egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("1. Select item").strong());
                        Self::entity_picker(
                            ui,
                            "inventory_move_item",
                            &mut item,
                            &item_options,
                            "Search item",
                        )
                    })
                    .inner
                })
                .inner;

            if item != item_before {
                source.clear();
                for idx in 0..destination_count {
                    self.forms
                        .drafts
                        .remove(&format!("inventory:move:dest:{idx}:location"));
                    self.forms
                        .drafts
                        .remove(&format!("inventory:move:dest:{idx}:qty"));
                }
            }

            let source_options = selected_item_id
                .or_else(|| Self::selected_entity_id(&item, &item_options))
                .map(|item_id| self.inventory_source_options_for_item(item_id))
                .unwrap_or_default();
            let source_balance_id = egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("2. Pick source").strong());
                        if source_options.is_empty() {
                            ui.weak("No available loose inventory for this item.");
                            None
                        } else {
                            let picked = Self::entity_picker(
                                ui,
                                "inventory_move_source",
                                &mut source,
                                &source_options,
                                "Search source location",
                            )
                            .or_else(|| Self::selected_entity_id(&source, &source_options));
                            ui.add_space(6.0);
                            let available_width = ui.available_width().max(260.0);
                            let card_width = 190.0_f32;
                            let columns = ((available_width / card_width).floor() as usize).max(1);
                            egui::Grid::new("inventory_move_source_cards")
                                .num_columns(columns)
                                .spacing([8.0, 8.0])
                                .show(ui, |ui| {
                                    for (idx, (balance_id, _)) in
                                        source_options.iter().take(12).enumerate()
                                    {
                                    if let Some(balance) = self
                                        .data
                                        .inventory_balances
                                        .iter()
                                        .find(|balance| balance.id == *balance_id)
                                    {
                                        let available = balance.qty_on_hand - balance.qty_reserved;
                                        let color = Self::inventory_status_color(ui, balance.status);
                                        let selected = picked == Some(*balance_id);
                                        let fill = if selected {
                                            egui::Color32::from_rgba_unmultiplied(
                                                color.r(),
                                                color.g(),
                                                color.b(),
                                                42,
                                            )
                                        } else if ui.visuals().dark_mode {
                                            egui::Color32::from_black_alpha(105)
                                        } else {
                                            egui::Color32::from_rgb(250, 252, 255)
                                        };
                                        egui::Frame::default()
                                            .fill(fill)
                                            .stroke(egui::Stroke::new(
                                                if selected { 2.0_f32 } else { 1.0_f32 },
                                                color,
                                            ))
                                            .rounding(egui::Rounding::same(8.0))
                                            .inner_margin(egui::Margin::symmetric(9.0, 7.0))
                                            .show(ui, |ui| {
                                                ui.set_width(card_width - 24.0);
                                                ui.vertical(|ui| {
                                                    ui.add(
                                                        egui::Label::new(
                                                            egui::RichText::new(self.location_label(
                                                                balance.location_id,
                                                            ))
                                                            .strong()
                                                            .color(Self::load_detail_text_color(ui)),
                                                        )
                                                        .truncate(true),
                                                    )
                                                    .on_hover_text(self.location_hover_text(
                                                        balance.location_id,
                                                    ));
                                                    ui.colored_label(
                                                        color,
                                                        Self::inventory_status_label(balance.status),
                                                    );
                                                    ui.label(format!(
                                                        "{available} available / {} reserved",
                                                        balance.qty_reserved
                                                    ));
                                                });
                                            });
                                    }
                                        if (idx + 1) % columns == 0 {
                                            ui.end_row();
                                        }
                                    }
                                });
                            picked
                        }
                    })
                    .inner
                })
                .inner;

            let selected_balance = source_balance_id.and_then(|id| {
                self.data
                    .inventory_balances
                    .iter()
                    .find(|balance| balance.id == id)
                    .cloned()
            });
            let source_location_id = selected_balance.as_ref().map(|balance| balance.location_id);
            let location_options = self
                .data
                .locations
                .iter()
                .filter(|location| location.deleted.is_none() && location.active)
                .filter(|location| Some(location.id) != source_location_id)
                .map(|location| (location.id, self.location_label(location.id)))
                .collect::<Vec<_>>();

            let mut remove_destination = None;
            let mut add_destination = false;
            let mut destination_rows = Vec::new();
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("3. Build destination plan").strong());
                        if ui.button("+ Add destination").clicked() {
                            add_destination = true;
                        }
                    });
                    ui.add_space(6.0);
                    egui::Grid::new("inventory_move_destinations")
                        .num_columns(4)
                        .spacing([8.0, 8.0])
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("To Location");
                            ui.strong("Qty");
                            ui.strong("Preview");
                            ui.end_row();
                            for idx in 0..destination_count {
                                let dest_key = format!("inventory:move:dest:{idx}:location");
                                let qty_key = format!("inventory:move:dest:{idx}:qty");
                                let mut dest = self
                                    .forms
                                    .drafts
                                    .get(&dest_key)
                                    .cloned()
                                    .unwrap_or_default();
                                let mut qty = self
                                    .forms
                                    .drafts
                                    .get(&qty_key)
                                    .cloned()
                                    .unwrap_or_default();
                                let selected_location_id = Self::entity_picker(
                                    ui,
                                    format!("inventory_move_dest_{idx}"),
                                    &mut dest,
                                    &location_options,
                                    "Search location",
                                )
                                .or_else(|| Self::selected_entity_id(&dest, &location_options));
                                ui.add(
                                    egui::TextEdit::singleline(&mut qty)
                                        .desired_width(80.0)
                                        .hint_text("Qty"),
                                );
                                let parsed_qty = qty.trim().parse::<i64>().ok();
                                match (selected_location_id, parsed_qty) {
                                    (Some(location_id), Some(qty)) if qty > 0 => {
                                        ui.label(format!(
                                            "{} -> {}",
                                            qty,
                                            self.location_label(location_id)
                                        ))
                                        .on_hover_text(self.location_hover_text(location_id));
                                    }
                                    (Some(location_id), _) => {
                                        ui.label(
                                            egui::RichText::new(self.location_label(location_id))
                                                .weak(),
                                        )
                                        .on_hover_text(self.location_hover_text(location_id));
                                    }
                                    _ => {
                                        ui.weak("Waiting for location and quantity");
                                    }
                                }
                                if destination_count > 1 && ui.button("Remove").clicked() {
                                    remove_destination = Some(idx);
                                }
                                ui.end_row();

                                self.forms.drafts.insert(dest_key, dest);
                                self.forms.drafts.insert(qty_key, qty);
                                destination_rows.push((idx, selected_location_id, parsed_qty));
                            }
                        });
                });

            if add_destination {
                destination_count = (destination_count + 1).min(12);
            }
            if let Some(remove_idx) = remove_destination {
                for idx in remove_idx..destination_count.saturating_sub(1) {
                    let next_location = self
                        .forms
                        .drafts
                        .remove(&format!("inventory:move:dest:{}:location", idx + 1))
                        .unwrap_or_default();
                    let next_qty = self
                        .forms
                        .drafts
                        .remove(&format!("inventory:move:dest:{}:qty", idx + 1))
                        .unwrap_or_default();
                    self.forms
                        .drafts
                        .insert(format!("inventory:move:dest:{idx}:location"), next_location);
                    self.forms
                        .drafts
                        .insert(format!("inventory:move:dest:{idx}:qty"), next_qty);
                }
                destination_count = destination_count.saturating_sub(1).max(1);
                self.forms.drafts.remove(&format!(
                    "inventory:move:dest:{destination_count}:location"
                ));
                self.forms
                    .drafts
                    .remove(&format!("inventory:move:dest:{destination_count}:qty"));
            }

            let mut parsed_destinations = Vec::new();
            let mut duplicate_destination = false;
            let mut destination_ids = BTreeSet::new();
            for (_, location_id, qty) in &destination_rows {
                if let (Some(location_id), Some(qty)) = (location_id, qty) {
                    if *qty > 0 {
                        if !destination_ids.insert(*location_id) {
                            duplicate_destination = true;
                        }
                        parsed_destinations.push((*location_id, *qty));
                    }
                }
            }
            let planned_total = parsed_destinations
                .iter()
                .map(|(_, qty)| *qty)
                .sum::<i64>();

            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("4. Review and move").strong());
                        if let Some(balance) = &selected_balance {
                            let available = balance.qty_on_hand - balance.qty_reserved;
                            let color = Self::inventory_status_color(ui, balance.status);
                            ui.horizontal_wrapped(|ui| {
                                Self::summary_badge(ui, "Available", available, color);
                                Self::summary_badge(
                                    ui,
                                    "Planned",
                                    planned_total,
                                    Self::order_summary_color(ui, "processing"),
                                );
                                Self::summary_badge(
                                    ui,
                                    "Remaining",
                                    available - planned_total,
                                    if planned_total > available {
                                        Self::danger_text_color(ui)
                                    } else {
                                        Self::success_text_color(ui)
                                    },
                                );
                            });
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} from {}",
                                    self.item_batch_label(balance.item_batch_id),
                                    self.location_label(balance.location_id)
                                ))
                                .color(Self::load_detail_text_color(ui)),
                            )
                            .on_hover_text(self.location_hover_text(balance.location_id));
                        } else {
                            ui.weak("Choose an item and source balance first.");
                        }
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Reason");
                            ui.add(
                                egui::TextEdit::singleline(&mut reason)
                                    .desired_width(260.0)
                                    .hint_text("Optional reason"),
                            );
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("Move inventory").strong(),
                                    )
                                    .min_size(egui::vec2(140.0, 30.0)),
                                )
                                .clicked()
                            {
                                match &selected_balance {
                                    None => self.toast("Choose a source location", true, self.now),
                                    Some(balance) if balance.license_plate_id.is_some() => {
                                        self.toast(
                                            "Use the License Plates panel to move license-plated stock",
                                            true,
                                            self.now,
                                        );
                                    }
                                    Some(_) if parsed_destinations.is_empty() => {
                                        self.toast(
                                            "Add at least one destination quantity",
                                            true,
                                            self.now,
                                        );
                                    }
                                    Some(_) if duplicate_destination => {
                                        self.toast(
                                            "Destination locations must be unique",
                                            true,
                                            self.now,
                                        );
                                    }
                                    Some(balance)
                                        if planned_total > balance.qty_on_hand - balance.qty_reserved =>
                                    {
                                        self.toast(
                                            format!(
                                                "Only {} units are available to move",
                                                balance.qty_on_hand - balance.qty_reserved
                                            ),
                                            true,
                                            self.now,
                                        );
                                    }
                                    Some(balance) => {
                                        let destinations = parsed_destinations
                                            .iter()
                                            .map(|(to_location_id, qty)| {
                                                json!({
                                                    "to_location_id": to_location_id,
                                                    "qty": qty,
                                                })
                                            })
                                            .collect::<Vec<_>>();
                                        let body = json!({
                                            "from_inventory_balance_id": balance.id,
                                            "destinations": destinations,
                                            "reason": reason.trim(),
                                        });
                                        self.api.action(
                                            "/api/inventory/movements/split",
                                            body,
                                            Screen::Inventory,
                                            "Inventory moved",
                                        );
                                        for idx in 0..destination_count {
                                            self.forms.drafts.remove(&format!(
                                                "inventory:move:dest:{idx}:location"
                                            ));
                                            self.forms.drafts.remove(&format!(
                                                "inventory:move:dest:{idx}:qty"
                                            ));
                                        }
                                        destination_count = 1;
                                    }
                                }
                            }
                        });
                    });
                });
                });

            self.forms.drafts.insert(item_key, item);
            self.forms.drafts.insert(source_key, source);
            self.forms.drafts.insert(reason_key, reason);
            self.forms
                .drafts
                .insert(destination_count_key, destination_count.to_string());
        });
    }

    fn inventory_balances_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Balances");
        let balances = self.data.inventory_balances.clone();
        let balance_count = balances.len();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().at_least(60.0))
            .column(Column::initial(220.0).at_least(140.0).clip(true))
            .column(Column::initial(180.0).at_least(120.0).clip(true))
            .column(Column::auto().at_least(95.0))
            .column(Column::auto().at_least(80.0))
            .column(Column::auto().at_least(80.0))
            .column(Column::auto().at_least(80.0))
            .header(24.0, |mut h| {
                for label in [
                    "ID",
                    "Item batch",
                    "Location",
                    "Status",
                    "On hand",
                    "Reserved",
                    "Available",
                ] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|body| {
                body.rows(28.0, balance_count, |mut row| {
                    let balance = balances[row.index()].clone();
                    row.col(|ui| {
                        ui.label(balance.id.to_string());
                    });
                    row.col(|ui| {
                        ui.label(self.item_batch_label(balance.item_batch_id));
                    });
                    row.col(|ui| {
                        self.location_label_ui(ui, balance.location_id);
                    });
                    row.col(|ui| {
                        ui.label(Self::inventory_status_label(balance.status));
                    });
                    row.col(|ui| {
                        ui.label(balance.qty_on_hand.to_string());
                    });
                    row.col(|ui| {
                        ui.label(balance.qty_reserved.to_string());
                    });
                    row.col(|ui| {
                        ui.strong((balance.qty_on_hand - balance.qty_reserved).to_string());
                    });
                });
            });
    }

    fn inventory_location_summary_tab(&self, ui: &mut egui::Ui) {
        ui.heading("On Hand by Location");
        let mut totals: BTreeMap<(i64, i64), (i64, i64, usize)> = BTreeMap::new();
        for balance in self
            .data
            .inventory_balances
            .iter()
            .filter(|balance| balance.deleted.is_none())
        {
            let Some(item_id) = self.item_id_for_batch(balance.item_batch_id) else {
                continue;
            };
            let entry = totals.entry((item_id, balance.location_id)).or_default();
            entry.0 += balance.qty_on_hand;
            entry.1 += balance.qty_reserved;
            entry.2 += 1;
        }
        let rows = totals.into_iter().collect::<Vec<_>>();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::initial(180.0).at_least(120.0).clip(true))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .header(24.0, |mut h| {
                for label in [
                    "Item",
                    "Location",
                    "Balances",
                    "On hand",
                    "Reserved",
                    "Available",
                ] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|body| {
                body.rows(28.0, rows.len(), |mut row| {
                    let ((item_id, location_id), (on_hand, reserved, balance_count)) =
                        rows[row.index()];
                    row.col(|ui| {
                        ui.label(self.item_label(item_id));
                    });
                    row.col(|ui| {
                        self.location_label_ui(ui, location_id);
                    });
                    row.col(|ui| {
                        ui.label(balance_count.to_string());
                    });
                    row.col(|ui| {
                        ui.label(on_hand.to_string());
                    });
                    row.col(|ui| {
                        ui.label(reserved.to_string());
                    });
                    row.col(|ui| {
                        ui.strong((on_hand - reserved).to_string());
                    });
                });
            });
    }

    fn inventory_facility_summary_tab(&self, ui: &mut egui::Ui) {
        ui.heading("On Hand by Facility");
        let mut totals: BTreeMap<(i64, i64), FacilityInventoryTotals> = BTreeMap::new();
        for balance in self
            .data
            .inventory_balances
            .iter()
            .filter(|balance| balance.deleted.is_none())
        {
            let Some(item_id) = self.item_id_for_batch(balance.item_batch_id) else {
                continue;
            };
            let entry = totals.entry((item_id, balance.facility_id)).or_default();
            entry.0 += balance.qty_on_hand;
            entry.1 += balance.qty_reserved;
            entry.2.insert(balance.location_id);
            entry.3 += 1;
        }
        let rows = totals.into_iter().collect::<Vec<_>>();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::initial(180.0).at_least(120.0).clip(true))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .header(24.0, |mut h| {
                for label in [
                    "Item",
                    "Facility",
                    "Locations",
                    "Balances",
                    "On hand",
                    "Reserved",
                    "Available",
                ] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|body| {
                body.rows(28.0, rows.len(), |mut row| {
                    let ((item_id, facility_id), (on_hand, reserved, locations, balance_count)) =
                        &rows[row.index()];
                    row.col(|ui| {
                        ui.label(self.item_label(*item_id));
                    });
                    row.col(|ui| {
                        ui.label(self.facility_label(*facility_id));
                    });
                    row.col(|ui| {
                        ui.label(locations.len().to_string());
                    });
                    row.col(|ui| {
                        ui.label(balance_count.to_string());
                    });
                    row.col(|ui| {
                        ui.label(on_hand.to_string());
                    });
                    row.col(|ui| {
                        ui.label(reserved.to_string());
                    });
                    row.col(|ui| {
                        ui.strong((on_hand - reserved).to_string());
                    });
                });
            });
    }

    fn inventory_item_summary_tab(&self, ui: &mut egui::Ui) {
        ui.heading("On Hand by Item");
        let batch_items = self
            .data
            .item_batches
            .iter()
            .map(|batch| (batch.id, batch.item_id))
            .collect::<HashMap<_, _>>();
        let mut totals: BTreeMap<i64, (i64, i64, BTreeSet<i64>, BTreeSet<i64>)> = BTreeMap::new();
        for balance in self
            .data
            .inventory_balances
            .iter()
            .filter(|balance| balance.deleted.is_none())
        {
            let Some(item_id) = batch_items.get(&balance.item_batch_id).copied() else {
                continue;
            };
            let entry = totals.entry(item_id).or_default();
            entry.0 += balance.qty_on_hand;
            entry.1 += balance.qty_reserved;
            entry.2.insert(balance.item_batch_id);
            entry.3.insert(balance.location_id);
        }
        let rows = totals.into_iter().collect::<Vec<_>>();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .header(24.0, |mut h| {
                for label in [
                    "Item",
                    "Batches",
                    "Locations",
                    "On hand",
                    "Reserved",
                    "Available",
                ] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|body| {
                body.rows(28.0, rows.len(), |mut row| {
                    let (item_id, (on_hand, reserved, batches, locations)) = &rows[row.index()];
                    row.col(|ui| {
                        ui.label(self.item_label(*item_id));
                    });
                    row.col(|ui| {
                        ui.label(batches.len().to_string());
                    });
                    row.col(|ui| {
                        ui.label(locations.len().to_string());
                    });
                    row.col(|ui| {
                        ui.label(on_hand.to_string());
                    });
                    row.col(|ui| {
                        ui.label(reserved.to_string());
                    });
                    row.col(|ui| {
                        ui.strong((on_hand - reserved).to_string());
                    });
                });
            });
    }

    fn inventory_movements_tab(&mut self, ui: &mut egui::Ui) {
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
                    ui.horizontal(|ui| {
                        ui.label("Location:");
                        if let Some(location_id) = lp.location_id {
                            self.location_label_ui(ui, location_id);
                        } else {
                            ui.label("-");
                        }
                    });
                    for content in &lp.contents {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!(
                                "batch {} status {} on hand {} reserved {} at loc",
                                content.item_batch_id,
                                content.status,
                                content.qty_on_hand,
                                content.qty_reserved,
                            ));
                            self.location_label_ui(ui, content.location_id);
                        });
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
                        ui.label("location");
                        if let Some(location_id) = lp.location_id {
                            self.location_label_ui(ui, location_id);
                        } else {
                            ui.label("-");
                        }
                        if lp.deleted.is_some() {
                            ui.colored_label(egui::Color32::LIGHT_RED, "(deleted)");
                        }
                    });
                    if lp.contents.is_empty() {
                        ui.weak("No inventory on this license plate.");
                    } else {
                        for content in &lp.contents {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(format!(
                                    "balance {} batch {} status {}: on hand {}, reserved {}, loc",
                                    content.inventory_balance_id,
                                    content.item_batch_id,
                                    content.status,
                                    content.qty_on_hand,
                                    content.qty_reserved,
                                ));
                                self.location_label_ui(ui, content.location_id);
                            });
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
