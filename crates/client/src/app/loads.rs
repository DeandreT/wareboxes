use super::*;

impl WareboxesApp {
    // ---- Loads -----------------------------------------------------------
    pub(super) fn loads_screen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("New load", |ui| {
            let warehouse_options = self.warehouse_options();
            let account_options = self.account_options();
            ui.horizontal_wrapped(|ui| {
                ui.label("Warehouse");
                let warehouse_id = Self::entity_picker(
                    ui,
                    "new_load_warehouse",
                    &mut self.forms.new_warehouse_id,
                    &warehouse_options,
                    "Search warehouse",
                );
                ui.label("Account");
                let account_id = Self::entity_picker(
                    ui,
                    "new_load_account",
                    &mut self.forms.new_account_id,
                    &account_options,
                    "Search account",
                );
                ui.label("Type");
                egui::ComboBox::from_id_source("new_load_type")
                    .selected_text(
                        LoadType::parse(&self.forms.new_type)
                            .map(Self::load_type_label)
                            .unwrap_or("Inbound"),
                    )
                    .show_ui(ui, |ui| {
                        for load_type in LoadType::ALL {
                            ui.selectable_value(
                                &mut self.forms.new_type,
                                load_type.as_str().to_owned(),
                                Self::load_type_label(load_type),
                            );
                        }
                    });
                if ui.button("Create").clicked() {
                    if let (Some(warehouse_id), Some(account_id)) = (warehouse_id, account_id) {
                        self.api.action(
                            "/api/loads/add",
                            json!({
                                "warehouse_id": warehouse_id,
                                "account_id": account_id,
                                "type": self.forms.new_type,
                            }),
                            Screen::Loads,
                            "Load created",
                        );
                    } else {
                        self.toast("Choose a warehouse and account", true, self.now);
                    }
                }
            });
        });
        ui.separator();

        let date_key = "loads:filter:date".to_owned();
        let date_mode_key = "loads:filter:date_mode".to_owned();
        let status_key = "loads:filter:status".to_owned();
        let type_key = "loads:filter:type".to_owned();
        let account_key = "loads:filter:account".to_owned();
        let search_key = "loads:filter:search".to_owned();

        let mut date_filter = self
            .forms
            .drafts
            .get(&date_key)
            .cloned()
            .unwrap_or_default();
        let mut date_mode = self
            .forms
            .drafts
            .get(&date_mode_key)
            .cloned()
            .unwrap_or_else(|| "day".to_owned());
        let mut status_filter = self
            .forms
            .drafts
            .get(&status_key)
            .cloned()
            .unwrap_or_default();
        let mut type_filter = self
            .forms
            .drafts
            .get(&type_key)
            .cloned()
            .unwrap_or_default();
        let mut account_filter = self
            .forms
            .drafts
            .get(&account_key)
            .cloned()
            .unwrap_or_default();
        let mut search_filter = self
            .forms
            .drafts
            .get(&search_key)
            .cloned()
            .unwrap_or_default();

        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Filters");
                ui.label("Date");
                self.load_date_filter_ui(ui, &mut date_filter, &mut date_mode);
                ui.label("Status");
                egui::ComboBox::from_id_source("loads_status_filter")
                    .selected_text(if status_filter.is_empty() {
                        "Any Status"
                    } else {
                        LoadStatus::parse(&status_filter)
                            .map(Self::load_status_label)
                            .unwrap_or(status_filter.as_str())
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut status_filter, String::new(), "Any Status");
                        for status in LoadStatus::ALL {
                            ui.selectable_value(
                                &mut status_filter,
                                status.as_str().to_owned(),
                                Self::load_status_label(status),
                            );
                        }
                    });
                ui.label("Type");
                egui::ComboBox::from_id_source("loads_type_filter")
                    .selected_text(if type_filter.is_empty() {
                        "Any Type"
                    } else {
                        LoadType::parse(&type_filter)
                            .map(Self::load_type_label)
                            .unwrap_or(type_filter.as_str())
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut type_filter, String::new(), "Any Type");
                        for load_type in LoadType::ALL {
                            ui.selectable_value(
                                &mut type_filter,
                                load_type.as_str().to_owned(),
                                Self::load_type_label(load_type),
                            );
                        }
                    });
                ui.label("Account");
                ui.add(egui::TextEdit::singleline(&mut account_filter).hint_text("ID or Name"));
                ui.label("Search");
                ui.add(
                    egui::TextEdit::singleline(&mut search_filter)
                        .hint_text("Reference, Invoice, Carrier, Trailer"),
                );
                if ui.small_button("Clear").clicked() {
                    date_filter.clear();
                    date_mode = "day".to_owned();
                    status_filter.clear();
                    type_filter.clear();
                    account_filter.clear();
                    search_filter.clear();
                }
            });
        });

        self.forms.drafts.insert(date_key, date_filter.clone());
        self.forms.drafts.insert(date_mode_key, date_mode.clone());
        self.forms.drafts.insert(status_key, status_filter.clone());
        self.forms.drafts.insert(type_key, type_filter.clone());
        self.forms
            .drafts
            .insert(account_key, account_filter.clone());
        self.forms.drafts.insert(search_key, search_filter.clone());

        let load_indices = self
            .data
            .loads
            .iter()
            .enumerate()
            .filter(|load| {
                self.load_matches_filters(
                    load.1,
                    &LoadFilters {
                        date: &date_filter,
                        date_mode: &date_mode,
                        status: &status_filter,
                        load_type: &type_filter,
                        account: &account_filter,
                        search: &search_filter,
                    },
                )
            })
            .map(|(idx, _)| idx)
            .collect::<Vec<_>>();
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong(format!("{} loads", load_indices.len()));
            if self.loading_loads {
                ui.spinner();
                ui.weak("Loading more...");
            }
        });

        let available_width = ui.available_width().max(1.0);
        let spacing = 8.0_f32;
        let target_card = 136.0_f32;
        let min_card = 96.0_f32;
        let max_card = 170.0_f32;
        let columns = ((available_width + spacing) / (target_card + spacing))
            .floor()
            .max(1.0) as usize;
        let card_side = ((available_width - spacing * (columns.saturating_sub(1) as f32))
            / columns as f32)
            .clamp(min_card.min(available_width), max_card.min(available_width));
        let row_count = load_indices.len().div_ceil(columns);
        let row_height = card_side + spacing;

        egui::ScrollArea::vertical()
            .id_source("loads_card_grid_scroll")
            .show_rows(ui, row_height, row_count, |ui, row_range| {
                for row in row_range {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = spacing;
                        for col in 0..columns {
                            let Some(load_idx) = load_indices.get(row * columns + col) else {
                                break;
                            };
                            let load = self.data.loads[*load_idx].clone();
                            self.load_card(ui, &load, card_side);
                        }
                    });
                    ui.add_space(spacing);
                }
            });

        let open_loads = self
            .open_load_ids
            .iter()
            .filter_map(|id| self.data.loads.iter().find(|load| load.id == *id).cloned())
            .collect::<Vec<_>>();
        for load in open_loads {
            self.load_detail_window(ui.ctx(), &load);
        }
    }

    pub(super) fn load_card(&mut self, ui: &mut egui::Ui, load: &Load, side: f32) {
        let status_color = Self::load_status_color(load.status);
        let dark_mode = ui.visuals().dark_mode;
        let fill = egui::Color32::from_rgba_unmultiplied(
            status_color.r(),
            status_color.g(),
            status_color.b(),
            if dark_mode { 76 } else { 108 },
        );
        let (text_color, outline_color) = Self::outlined_load_text_colors(ui);
        let selected = self.open_load_ids.contains(&load.id);
        let stroke = if selected {
            egui::Stroke::new(2.0_f32, text_color)
        } else {
            egui::Stroke::new(1.2_f32, status_color)
        };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect(rect, egui::Rounding::same(8.0), fill, stroke);
        if response.clicked() {
            self.open_load_ids.insert(load.id);
            self.api.get_load_detail(load.id);
        }
        response.on_hover_cursor(egui::CursorIcon::PointingHand);

        let margin = if side < 76.0 { 4.0 } else { 7.0 };
        let content_rect = rect.shrink(margin);
        let x = content_rect.left();
        let max_text_width = content_rect.width();
        let mut y = content_rect.top();

        let account = Self::fit_card_text(&self.load_account_label(load), max_text_width, 15.0);
        Self::paint_outlined_text(
            &painter,
            egui::pos2(x, y),
            egui::Align2::LEFT_TOP,
            &account,
            egui::FontId::proportional(15.0),
            text_color,
            outline_color,
        );
        y += 17.0;

        let load_label = Self::fit_card_text(
            &format!("#{} {}", load.id, Self::load_type_label(load.r#type)),
            max_text_width,
            13.0,
        );
        Self::paint_outlined_text(
            &painter,
            egui::pos2(x, y),
            egui::Align2::LEFT_TOP,
            &load_label,
            egui::FontId::proportional(13.0),
            text_color,
            outline_color,
        );
        y += 15.0;

        if side >= 104.0 {
            let reference = Self::fit_card_text(
                load.reference_number
                    .as_deref()
                    .or(load.invoice_number.as_deref())
                    .unwrap_or("-"),
                max_text_width,
                12.0,
            );
            Self::paint_outlined_text(
                &painter,
                egui::pos2(x, y),
                egui::Align2::LEFT_TOP,
                &reference,
                egui::FontId::proportional(12.0),
                text_color,
                outline_color,
            );
            y += 15.0;
        }

        let (resolved, expected) = Self::load_progress(load);
        let progress = if expected > 0 {
            (resolved as f32 / expected as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if side >= 110.0 {
            let progress_rect =
                egui::Rect::from_min_size(egui::pos2(x, y + 2.0), egui::vec2(max_text_width, 10.0));
            painter.rect_filled(
                progress_rect,
                egui::Rounding::same(3.0),
                if dark_mode {
                    egui::Color32::from_black_alpha(115)
                } else {
                    egui::Color32::from_white_alpha(165)
                },
            );
            painter.rect_filled(
                egui::Rect::from_min_size(
                    progress_rect.min,
                    egui::vec2(progress_rect.width() * progress, progress_rect.height()),
                ),
                egui::Rounding::same(3.0),
                if dark_mode {
                    egui::Color32::from_white_alpha(155)
                } else {
                    egui::Color32::from_rgba_unmultiplied(
                        text_color.r(),
                        text_color.g(),
                        text_color.b(),
                        145,
                    )
                },
            );
            Self::paint_outlined_text(
                &painter,
                progress_rect.center(),
                egui::Align2::CENTER_CENTER,
                &format!("{resolved}/{expected}"),
                egui::FontId::proportional(10.5),
                text_color,
                outline_color,
            );
            y += 16.0;
        }

        if side >= 124.0 {
            let counts = Self::fit_card_text(&Self::line_status_counts(load), max_text_width, 10.5);
            Self::paint_outlined_text(
                &painter,
                egui::pos2(x, y),
                egui::Align2::LEFT_TOP,
                &counts,
                egui::FontId::proportional(10.5),
                text_color,
                outline_color,
            );
        }

        let footer = Self::load_status_label(load.status);
        let footer = Self::fit_card_text(
            &format!("{} {}", Self::short_datetime(load.created), footer),
            max_text_width,
            10.5,
        );
        Self::paint_outlined_text(
            &painter,
            egui::pos2(content_rect.left(), content_rect.bottom()),
            egui::Align2::LEFT_BOTTOM,
            &footer,
            egui::FontId::proportional(10.5),
            text_color,
            outline_color,
        );
        if let Some(milestone) = Self::load_card_milestone(load) {
            let milestone = Self::fit_card_text(milestone, max_text_width * 0.7, 10.5);
            Self::paint_outlined_text(
                &painter,
                egui::pos2(content_rect.right(), content_rect.bottom()),
                egui::Align2::RIGHT_BOTTOM,
                &milestone,
                egui::FontId::proportional(10.5),
                text_color,
                outline_color,
            );
        }
    }

    pub(super) fn load_line_visual(&self, ui: &mut egui::Ui, line: &LoadLine) {
        let line_color = Self::load_line_color(line.status);
        let dark_mode = ui.visuals().dark_mode;
        let fill = egui::Color32::from_rgba_unmultiplied(
            line_color.r(),
            line_color.g(),
            line_color.b(),
            if dark_mode { 58 } else { 42 },
        );
        let stroke = egui::Stroke::new(1.0_f32, Self::load_line_color(line.status));
        let resolved = line.received_qty + line.rejected_qty + line.missing_qty;
        let progress = if line.expected_qty > 0 {
            (resolved as f32 / line.expected_qty as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };

        egui::Frame::default()
            .fill(fill)
            .stroke(stroke)
            .inner_margin(egui::Margin::same(8.0))
            .rounding(egui::Rounding::same(6.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    Self::detail_text(ui, &format!("Line #{}", line.id), true, 15.0);
                    Self::detail_text(ui, Self::load_line_status_label(line.status), true, 15.0);
                    Self::detail_text(
                        ui,
                        &format!("Item {}", self.item_label(line.item_id)),
                        false,
                        14.0,
                    );
                    if let Some(lot) = &line.lot {
                        Self::detail_text(ui, &format!("Lot {lot}"), false, 14.0);
                    }
                    if let Some(serial) = &line.serial {
                        Self::detail_text(ui, &format!("Serial {serial}"), false, 14.0);
                    }
                    if let Some(expiration) = line.expiration {
                        Self::detail_text(
                            ui,
                            &format!("Expires {}", Self::short_datetime(expiration)),
                            false,
                            14.0,
                        );
                    }
                });
                ui.add(egui::ProgressBar::new(progress).text(format!(
                    "{} resolved of {} expected",
                    resolved, line.expected_qty
                )));
                ui.horizontal(|ui| {
                    Self::qty_chip(ui, "Expected", line.expected_qty, egui::Color32::LIGHT_GRAY);
                    Self::qty_chip(
                        ui,
                        "Received",
                        line.received_qty,
                        Self::load_line_color(LoadLineStatus::Received),
                    );
                    Self::qty_chip(
                        ui,
                        "Rejected",
                        line.rejected_qty,
                        Self::load_line_color(LoadLineStatus::Rejected),
                    );
                    Self::qty_chip(
                        ui,
                        "Missing",
                        line.missing_qty,
                        Self::load_line_color(LoadLineStatus::Missing),
                    );
                    if line.missing_confirmed_by.is_some() {
                        ui.colored_label(Self::success_text_color(ui), "Missing Confirmed");
                    }
                });
            });
    }

    pub(super) fn load_order_visual(&self, ui: &mut egui::Ui, order: &Order) {
        let dark_mode = ui.visuals().dark_mode;
        let fill = if dark_mode {
            egui::Color32::from_black_alpha(78)
        } else {
            egui::Color32::from_rgb(248, 250, 253)
        };
        let stroke = if dark_mode {
            egui::Color32::from_rgb(105, 130, 165)
        } else {
            egui::Color32::from_rgb(185, 197, 216)
        };
        egui::Frame::default()
            .fill(fill)
            .stroke(egui::Stroke::new(1.0_f32, stroke))
            .inner_margin(egui::Margin::same(8.0))
            .rounding(egui::Rounding::same(6.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    Self::detail_text(ui, &order.order_key, true, 15.0);
                    Self::detail_text(ui, Self::order_status_label(order.status), true, 14.0);
                    if order.rush {
                        ui.colored_label(Self::danger_text_color(ui), "Rush");
                    }
                    Self::detail_text(
                        ui,
                        &format!("Items {}", order.order_items.len()),
                        false,
                        14.0,
                    );
                    if let Some(ship_by) = order.ship_by {
                        Self::detail_text(
                            ui,
                            &format!("Ship by {}", Self::short_datetime(ship_by)),
                            false,
                            14.0,
                        );
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    let address = [
                        order.line1.as_deref(),
                        order.city.as_deref(),
                        order.state.as_deref(),
                        order.postal_code.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .filter(|part| !part.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(", ");
                    Self::detail_text(
                        ui,
                        &format!(
                            "Ship to {}",
                            if address.is_empty() {
                                "-"
                            } else {
                                address.as_str()
                            }
                        ),
                        false,
                        13.0,
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("Tracking")
                            .strong()
                            .color(Self::load_detail_text_color(ui)),
                    );
                    if order.tracking_numbers.is_empty() {
                        ui.weak("None");
                    } else {
                        for tracking in &order.tracking_numbers {
                            let carrier = tracking.carrier.as_deref().unwrap_or("Carrier");
                            let service = tracking.service.as_deref().unwrap_or("service");
                            Self::tracking_chip(
                                ui,
                                &format!("{carrier} {service}"),
                                &tracking.tracking_number,
                            );
                        }
                    }
                });
            });
    }

    pub(super) fn load_note_visual(
        &mut self,
        ui: &mut egui::Ui,
        note: &LoadNote,
        display_number: usize,
    ) {
        let dark_mode = ui.visuals().dark_mode;
        egui::Frame::default()
            .fill(if dark_mode {
                egui::Color32::from_black_alpha(85)
            } else {
                egui::Color32::from_rgb(248, 250, 253)
            })
            .stroke(egui::Stroke::new(
                1.0_f32,
                if dark_mode {
                    egui::Color32::from_rgb(115, 135, 165)
                } else {
                    egui::Color32::from_rgb(188, 198, 214)
                },
            ))
            .inner_margin(egui::Margin::same(8.0))
            .rounding(egui::Rounding::same(6.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    Self::detail_text(ui, &format!("Note #{}", display_number), true, 14.0);
                    Self::detail_text(
                        ui,
                        &format!("Created {}", Self::short_datetime(note.created)),
                        false,
                        13.0,
                    );
                    if note.deleted.is_some() {
                        ui.colored_label(Self::danger_text_color(ui), "Deleted");
                    } else {
                        self.delete_button(
                            ui,
                            true,
                            format!("load note #{}", display_number),
                            "/api/loads/notes/delete",
                            json!({"load_note_id": note.id}),
                            Screen::Loads,
                            "Load note deleted",
                        );
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(&note.note)
                        .color(Self::load_detail_text_color(ui))
                        .size(14.0),
                );
            });
    }

    pub(super) fn load_detail_window(&mut self, ctx: &egui::Context, load: &Load) {
        let mut open = true;
        egui::Window::new(format!("Load #{} details", load.id))
            .id(egui::Id::new(("load_detail", load.id)))
            .open(&mut open)
            .resizable(true)
            .default_width(920.0)
            .default_height(680.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        Self::load_status_badge(ui, load.status);
                        ui.strong(format!("{} Load", Self::load_type_label(load.r#type)));
                        ui.label(format!("Created {}", Self::short_datetime(load.created)));
                        ui.label(format!(
                            "Warehouse {}",
                            self.load_warehouse_label(load)
                        ));
                        ui.label(format!("Account {}", self.load_account_label(load)));
                        if load.r#type == LoadType::Inbound && load.receive_completed {
                            ui.colored_label(Self::success_text_color(ui), "Receive Complete");
                        }
                        if load.deleted.is_some() {
                            ui.colored_label(Self::danger_text_color(ui), "Deleted");
                        }
                    });

                    ui.separator();
                    ui.collapsing("Header", |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!(
                                "Reference {}",
                                load.reference_number.as_deref().unwrap_or("-")
                            ));
                            ui.label(format!(
                                "Invoice {}",
                                load.invoice_number.as_deref().unwrap_or("-")
                            ));
                            ui.label(format!(
                                "Carrier {}",
                                load.carrier.as_deref().unwrap_or("-")
                            ));
                            ui.label(format!(
                                "Trailer {}",
                                load.trailer_number.as_deref().unwrap_or("-")
                            ));
                            ui.label(format!(
                                "Seal {}",
                                load.seal_number.as_deref().unwrap_or("-")
                            ));
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!(
                                "Expected {}",
                                Self::optional_datetime(load.expected_time)
                            ));
                            ui.label(format!(
                                "Appointment {}",
                                Self::optional_datetime(load.appointment_time)
                            ));
                            ui.label(format!("Arrival {}", Self::optional_datetime(load.arrival)));
                            ui.label(format!("Closed {}", Self::optional_datetime(load.closed)));
                        });
                    });

                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        let invoice_key = format!("load:{}:invoice", load.id);
                        let arrival_date_key = format!("load:{}:arrival_date", load.id);
                        let arrival_time_key = format!("load:{}:arrival_time", load.id);
                        let mut invoice = self
                            .forms
                            .drafts
                            .get(&invoice_key)
                            .cloned()
                            .unwrap_or_else(|| load.invoice_number.clone().unwrap_or_default());
                        let (default_arrival_date, default_arrival_time) =
                            Self::arrival_draft_default(load);
                        let mut arrival_date = self
                            .forms
                            .drafts
                            .get(&arrival_date_key)
                            .cloned()
                            .unwrap_or(default_arrival_date);
                        let mut arrival_time = self
                            .forms
                            .drafts
                            .get(&arrival_time_key)
                            .cloned()
                            .unwrap_or(default_arrival_time);
                        ui.label("Invoice #");
                        ui.text_edit_singleline(&mut invoice);
                        if ui.small_button("Save invoice").clicked() {
                            self.api.action(
                                "/api/loads/update",
                                json!({"load_id": load.id, "invoice_number": invoice}),
                                Screen::Loads,
                                "Invoice updated",
                            );
                        }
                        if Self::can_arrive_load(load) {
                            ui.label("Arrival Date");
                            self.date_picker_ui(
                                ui,
                                &format!("load_{}_arrival", load.id),
                                &mut arrival_date,
                                "Pick Arrival Date",
                                Some(Local::now().date_naive()),
                                false,
                            );
                            ui.label("Time");
                            ui.add(
                                egui::TextEdit::singleline(&mut arrival_time)
                                    .desired_width(72.0)
                                    .hint_text("HH:MM"),
                            );
                            if ui.small_button("Arrive").clicked() {
                                if let Some(arrival) =
                                    Self::arrival_rfc3339(&arrival_date, &arrival_time)
                                {
                                    let arrive_path =
                                        format!("/api/mobile/inbound/loads/{}/arrive", load.id);
                                    self.api.action(
                                        &arrive_path,
                                        json!({
                                            "invoice_number": invoice,
                                            "arrival": arrival,
                                        }),
                                        Screen::Loads,
                                        "Load arrived",
                                    );
                                } else {
                                    self.toast(
                                        "Arrival date/time must be a valid non-future date and HH:MM time",
                                        true,
                                        self.now,
                                    );
                                }
                            }
                        }
                        self.forms.drafts.insert(invoice_key, invoice);
                        self.forms.drafts.insert(arrival_date_key, arrival_date);
                        self.forms.drafts.insert(arrival_time_key, arrival_time);
                    });

                    ui.collapsing("Invoice file metadata", |ui| {
                        let original_key = format!("load:{}:file_original", load.id);
                        let path_key = format!("load:{}:file_path", load.id);
                        let content_type_key = format!("load:{}:file_content_type", load.id);
                        let mut original = self
                            .forms
                            .drafts
                            .get(&original_key)
                            .cloned()
                            .unwrap_or_default();
                        let mut path = self
                            .forms
                            .drafts
                            .get(&path_key)
                            .cloned()
                            .unwrap_or_default();
                        let mut content_type = self
                            .forms
                            .drafts
                            .get(&content_type_key)
                            .cloned()
                            .unwrap_or_else(|| "image/jpeg".to_owned());
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Original name");
                            ui.text_edit_singleline(&mut original);
                            ui.label("Path/URI");
                            ui.text_edit_singleline(&mut path);
                            ui.label("Content type");
                            ui.text_edit_singleline(&mut content_type);
                            if ui.small_button("Attach invoice").clicked()
                                && !original.trim().is_empty()
                                && !path.trim().is_empty()
                            {
                                self.api.action(
                                    "/api/loads/files/add",
                                    json!({
                                        "load_id": load.id,
                                        "original_name": original.clone(),
                                        "name": original.clone(),
                                        "path": path.clone(),
                                        "content_type": content_type.clone(),
                                        "category": LoadFileCategory::Invoice.as_str(),
                                    }),
                                    Screen::Loads,
                                    "Invoice file attached",
                                );
                            }
                        });
                        self.forms.drafts.insert(original_key, original);
                        self.forms.drafts.insert(path_key, path);
                        self.forms.drafts.insert(content_type_key, content_type);
                        for file in &load.files {
                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "#{} {} {}",
                                    file.id, file.category, file.original_name
                                ));
                                ui.weak(&file.path);
                                self.delete_button(
                                    ui,
                                    true,
                                    format!("load file {}", file.original_name),
                                    "/api/loads/files/delete",
                                    json!({"file_id": file.id}),
                                    Screen::Loads,
                                    "Load file deleted",
                                );
                            });
                        }
                    });

                    ui.separator();
                    match load.r#type {
                        LoadType::Inbound => {
                            ui.heading("Line items");
                            if load.lines.is_empty() {
                                ui.weak("No line items on this load.");
                            }
                            for line in &load.lines {
                                self.load_line_visual(ui, line);
                                ui.add_space(6.0);
                            }
                        }
                        LoadType::Outbound => {
                            ui.heading("Orders and tracking");
                            if load.orders.is_empty() {
                                ui.weak("No orders are linked to this outbound load.");
                            }
                            for order in &load.orders {
                                self.load_order_visual(ui, order);
                                ui.add_space(6.0);
                            }
                        }
                    }

                    ui.separator();
                    ui.collapsing("Load notes and actions", |ui| {
                        if load.notes.is_empty() {
                            ui.weak("No notes on this load.");
                        } else {
                            let mut notes = load.notes.clone();
                            notes.sort_by(|a, b| {
                                b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id))
                            });
                            for (idx, note) in notes.iter().enumerate() {
                                self.load_note_visual(ui, note, idx + 1);
                                ui.add_space(6.0);
                            }
                        }
                        ui.horizontal_wrapped(|ui| {
                            let note_key = format!("load:{}:note", load.id);
                            let mut note = self
                                .forms
                                .drafts
                                .get(&note_key)
                                .cloned()
                                .unwrap_or_default();
                            ui.label("Note");
                            ui.text_edit_singleline(&mut note);
                            if ui.small_button("Add note").clicked() && !note.trim().is_empty() {
                                self.api.action(
                                    "/api/loads/notes/add",
                                    json!({"load_id": load.id, "note": note}),
                                    Screen::Loads,
                                    "Load note added",
                                );
                            }
                            self.forms.drafts.insert(note_key, note);
                        });
                        ui.horizontal(|ui| {
                            if load.r#type == LoadType::Inbound {
                                let mut completed = load.receive_completed;
                                if ui.checkbox(&mut completed, "Receive complete").changed() {
                                    self.api.action(
                                        "/api/loads/update",
                                        json!({"load_id": load.id, "receive_completed": completed}),
                                        Screen::Loads,
                                        "Load updated",
                                    );
                                }
                            }
                            if load.deleted.is_none() {
                                self.delete_button(
                                    ui,
                                    true,
                                    format!("load #{}", load.id),
                                    "/api/loads/delete",
                                    json!({"load_id": load.id}),
                                    Screen::Loads,
                                    "Load deleted",
                                );
                            } else if ui.small_button("Restore").clicked() {
                                self.api.action(
                                    "/api/loads/restore",
                                    json!({"load_id": load.id}),
                                    Screen::Loads,
                                    "Load restored",
                                );
                            }
                        });
                    });
                });
            });
        if !open {
            self.open_load_ids.remove(&load.id);
        }
    }

    pub(super) fn load_date_filter_ui(
        &mut self,
        ui: &mut egui::Ui,
        date_filter: &mut String,
        date_mode: &mut String,
    ) {
        if !matches!(date_mode.as_str(), "day" | "month" | "year") {
            *date_mode = "day".to_owned();
        }

        egui::ComboBox::from_id_source("loads_date_mode_filter")
            .selected_text(match date_mode.as_str() {
                "month" => "Month",
                "year" => "Year",
                _ => "Day",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(date_mode, "day".to_owned(), "Day");
                ui.selectable_value(date_mode, "month".to_owned(), "Month");
                ui.selectable_value(date_mode, "year".to_owned(), "Year");
            });

        self.date_picker_ui(ui, "loads_filter", date_filter, "Pick Date", None, true);
    }

    pub(super) fn date_picker_ui(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        date_value: &mut String,
        empty_label: &str,
        max_date: Option<NaiveDate>,
        allow_clear: bool,
    ) {
        let today = Local::now().date_naive();
        let selected = Self::parse_filter_date(date_value).unwrap_or_else(|| {
            max_date
                .map(|max_date| today.min(max_date))
                .unwrap_or(today)
        });
        let calendar_month_key = format!("{id}:calendar_month");
        let calendar_view_key = format!("{id}:calendar_view");
        let mut calendar_view = self
            .forms
            .drafts
            .get(&calendar_view_key)
            .cloned()
            .unwrap_or_else(|| "day".to_owned());
        if !matches!(calendar_view.as_str(), "day" | "month" | "year") {
            calendar_view = "day".to_owned();
        }
        let mut calendar_month = self
            .forms
            .drafts
            .get(&calendar_month_key)
            .and_then(|s| Self::parse_filter_date(&format!("{s}-01")))
            .and_then(|d| NaiveDate::from_ymd_opt(d.year(), d.month(), 1))
            .or_else(|| NaiveDate::from_ymd_opt(selected.year(), selected.month(), 1))
            .unwrap_or(today);

        let button_text = if date_value.trim().is_empty() {
            empty_label.to_owned()
        } else {
            selected.format("%Y-%m-%d").to_string()
        };

        ui.menu_button(button_text, |ui| {
            ui.set_min_width(250.0);
            ui.horizontal(|ui| {
                if ui.small_button("<").clicked() {
                    calendar_month = match calendar_view.as_str() {
                        "month" => Self::shift_month(calendar_month, -12),
                        "year" => Self::shift_month(calendar_month, -144),
                        _ => Self::shift_month(calendar_month, -1),
                    };
                }
                if ui.button(calendar_month.format("%B").to_string()).clicked() {
                    calendar_view = "month".to_owned();
                }
                if ui.button(calendar_month.format("%Y").to_string()).clicked() {
                    calendar_view = "year".to_owned();
                }
                if ui.small_button(">").clicked() {
                    calendar_month = match calendar_view.as_str() {
                        "month" => Self::shift_month(calendar_month, 12),
                        "year" => Self::shift_month(calendar_month, 144),
                        _ => Self::shift_month(calendar_month, 1),
                    };
                }
                let today_enabled = max_date.map_or(true, |max_date| today <= max_date);
                if ui
                    .add_enabled(today_enabled, egui::Button::new("Today"))
                    .clicked()
                {
                    *date_value = today.format("%Y-%m-%d").to_string();
                    calendar_month =
                        NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap_or(today);
                    calendar_view = "day".to_owned();
                    ui.close_menu();
                }
            });
            ui.add_space(4.0);
            match calendar_view.as_str() {
                "month" => {
                    egui::Grid::new((id, "month_picker_grid"))
                        .spacing([10.0, 6.0])
                        .show(ui, |ui| {
                            for month in 1..=12 {
                                let month_date =
                                    NaiveDate::from_ymd_opt(calendar_month.year(), month, 1)
                                        .unwrap_or(calendar_month);
                                let enabled =
                                    max_date.map_or(true, |max_date| month_date <= max_date);
                                if ui
                                    .add_enabled(
                                        enabled,
                                        egui::Button::new(month_date.format("%b").to_string()),
                                    )
                                    .clicked()
                                {
                                    calendar_month = month_date;
                                    calendar_view = "day".to_owned();
                                }
                                if month % 4 == 0 {
                                    ui.end_row();
                                }
                            }
                        });
                }
                "year" => {
                    let start_year = calendar_month.year() - calendar_month.year().rem_euclid(12);
                    egui::Grid::new((id, "year_picker_grid"))
                        .spacing([10.0, 6.0])
                        .show(ui, |ui| {
                            for offset in 0..12 {
                                let year = start_year + offset;
                                let year_date =
                                    NaiveDate::from_ymd_opt(year, 1, 1).unwrap_or(calendar_month);
                                let enabled =
                                    max_date.map_or(true, |max_date| year_date <= max_date);
                                if ui
                                    .add_enabled(enabled, egui::Button::new(year.to_string()))
                                    .clicked()
                                {
                                    calendar_month =
                                        NaiveDate::from_ymd_opt(year, calendar_month.month(), 1)
                                            .unwrap_or(calendar_month);
                                    calendar_view = "month".to_owned();
                                }
                                if (offset + 1) % 4 == 0 {
                                    ui.end_row();
                                }
                            }
                        });
                }
                _ => {
                    egui::Grid::new((id, "day_picker_grid"))
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            for weekday in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] {
                                ui.weak(weekday);
                            }
                            ui.end_row();

                            let first_weekday =
                                calendar_month.weekday().num_days_from_monday() as usize;
                            let days =
                                Self::days_in_month(calendar_month.year(), calendar_month.month());
                            let mut day = 1_u32;
                            for week in 0..6 {
                                for weekday in 0..7 {
                                    if (week == 0 && weekday < first_weekday) || day > days {
                                        ui.label("");
                                        continue;
                                    }

                                    let cell_date = NaiveDate::from_ymd_opt(
                                        calendar_month.year(),
                                        calendar_month.month(),
                                        day,
                                    )
                                    .unwrap_or(calendar_month);
                                    let is_selected =
                                        !date_value.is_empty() && cell_date == selected;
                                    let enabled =
                                        max_date.map_or(true, |max_date| cell_date <= max_date);
                                    if ui
                                        .add_enabled(
                                            enabled,
                                            egui::SelectableLabel::new(
                                                is_selected,
                                                day.to_string(),
                                            ),
                                        )
                                        .clicked()
                                    {
                                        *date_value = cell_date.format("%Y-%m-%d").to_string();
                                        calendar_view = "day".to_owned();
                                        ui.close_menu();
                                    }
                                    day += 1;
                                }
                                ui.end_row();
                            }
                        });
                }
            }
        });

        if allow_clear && ui.small_button("No Date").clicked() {
            date_value.clear();
        }

        self.forms.drafts.insert(
            calendar_month_key,
            calendar_month.format("%Y-%m").to_string(),
        );
        self.forms.drafts.insert(calendar_view_key, calendar_view);
    }
}
