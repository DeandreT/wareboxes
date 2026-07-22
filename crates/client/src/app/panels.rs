use super::*;

impl WareboxesApp {
    // ---- Orders ----------------------------------------------------------
    pub(super) fn new_order_window(&mut self, ctx: &egui::Context) {
        if !self.new_order_open {
            return;
        }

        let mut open = self.new_order_open;
        let mut submit = false;
        let mut cancel = false;
        let inventory_owner_options = self.inventory_owner_options();
        let mut inventory_owner_id = None;
        egui::Window::new("New order")
            .id(egui::Id::new(("new_order", self.active_workspace_id)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .constrain(false)
            .default_pos(egui::pos2(160.0, 110.0))
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.strong("Order identity");
                egui::Grid::new("new_order_identity")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Order key");
                        ui.add_sized(
                            [280.0, 32.0],
                            egui::TextEdit::singleline(&mut self.forms.o_key),
                        );
                        ui.end_row();
                        ui.label("Inventory owner");
                        inventory_owner_id = Self::entity_picker(
                            ui,
                            "new_order_inventory_owner",
                            &mut self.forms.o_inventory_owner_id,
                            &inventory_owner_options,
                            "Search inventory owner",
                        );
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);
                ui.strong("Ship to");
                egui::Grid::new("new_order_address")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        for (label, value) in [
                            ("Address", &mut self.forms.o_line1),
                            ("City", &mut self.forms.o_city),
                            ("State", &mut self.forms.o_state),
                            ("Postal code", &mut self.forms.o_zip),
                            ("Country", &mut self.forms.o_country),
                        ] {
                            ui.label(label);
                            ui.add_sized([280.0, 32.0], egui::TextEdit::singleline(value));
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
                ui.checkbox(&mut self.forms.o_rush, "Rush order");
                ui.add_space(10.0);
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let create = egui::Button::new(
                        egui::RichText::new("Create order")
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(Self::accent_color(ui));
                    if ui.add(create).clicked() {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if submit {
            if self.forms.o_key.trim().is_empty() {
                self.toast("Enter an order key", true, self.now);
            } else if let Some(inventory_owner_id) = inventory_owner_id {
                let body = json!({
                    "order_key": self.forms.o_key,
                    "inventory_owner_id": inventory_owner_id,
                    "line1": self.forms.o_line1,
                    "city": self.forms.o_city,
                    "state": self.forms.o_state,
                    "postal_code": self.forms.o_zip,
                    "country": self.forms.o_country,
                    "rush": self.forms.o_rush,
                });
                self.api
                    .action("/api/orders/add", body, Screen::Orders, "Order created");
                self.forms.o_key.clear();
                open = false;
            } else {
                self.toast("Choose an inventory owner", true, self.now);
            }
        }
        if cancel {
            open = false;
        }
        self.new_order_open = open;
    }

    pub(super) fn orders_screen(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let new_order = egui::Button::new(
                egui::RichText::new("New order")
                    .strong()
                    .color(egui::Color32::WHITE),
            )
            .fill(Self::accent_color(ui));
            if ui.add(new_order).clicked() {
                self.new_order_open = true;
            }
            ui.separator();

            if self.data.order_summaries.is_empty() {
                ui.weak("No active exceptions");
            } else {
                for summary in &self.data.order_summaries {
                    Self::summary_badge(
                        ui,
                        &summary.label,
                        summary.count,
                        Self::order_summary_color(ui, &summary.key),
                    );
                }
            }
        });
        ui.separator();

        let mut search = self
            .forms
            .drafts
            .get("orders:search")
            .cloned()
            .unwrap_or_default();
        let mut status_filter = self
            .forms
            .drafts
            .get("orders:status")
            .cloned()
            .unwrap_or_default();
        let mut refresh_orders = false;
        egui::Frame::none()
            .fill(ui.visuals().faint_bg_color)
            .rounding(egui::Rounding::same(4.0))
            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let search_response = ui.add(
                        egui::TextEdit::singleline(&mut search)
                            .desired_width(ui.available_width().min(290.0))
                            .hint_text("Order, owner, city, state, or postal code"),
                    );
                    let search_submitted = search_response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if Self::icon_button(ui, Icon::Search, "Apply search").clicked()
                        || search_submitted
                    {
                        self.order_offset = 0;
                        refresh_orders = true;
                    }

                    ui.separator();
                    ui.weak("Status");
                    let before_status = status_filter.clone();
                    Self::select_from_allowed(
                        ui,
                        "orders_status_filter",
                        &mut status_filter,
                        &ORDER_STATUS_FILTERS,
                    );
                    if status_filter != before_status {
                        self.order_offset = 0;
                        refresh_orders = true;
                    }

                    if Self::icon_button(ui, Icon::RotateCcw, "Reset filters").clicked() {
                        search.clear();
                        status_filter.clear();
                        self.order_offset = 0;
                        refresh_orders = true;
                    }
                });
            });
        self.forms.drafts.insert("orders:search".to_owned(), search);
        self.forms
            .drafts
            .insert("orders:status".to_owned(), status_filter);

        if refresh_orders {
            self.fetch(Screen::Orders);
        }
        ui.add_space(2.0);

        let order_count = self.data.orders.len();
        let table_header_height = 24.0;
        let pagination_height = ui.spacing().interact_size.y;
        let table_vertical_spacing = ui.spacing().item_spacing.y * 2.0;
        let table_body_height = (ui.available_height()
            - table_header_height
            - pagination_height
            - table_vertical_spacing)
            .max(0.0);
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .max_scroll_height(table_body_height)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().at_least(34.0)) // ID
            .column(Column::initial(120.0).at_least(76.0).clip(true)) // Key
            .column(Column::initial(128.0).at_least(88.0).clip(true)) // Inventory Owner
            .column(Column::initial(132.0).at_least(104.0)) // Status
            .column(Column::remainder().at_least(120.0).clip(true)) // Ship to
            .column(Column::auto().at_least(32.0)) // Actions
            .header(table_header_height, |mut h| {
                for label in ["ID", "Key", "Inventory Owner", "Status", "Ship to", ""] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|body| {
                body.rows(30.0, order_count, |mut row| {
                    let o = self.data.orders[row.index()].clone();
                    let selected = self.open_order_ids.contains(&o.id);
                    row.set_selected(selected);
                    let mut open_details = false;
                    let mut delete_order = false;
                    let mut restore_order = false;
                    row.col(|ui| {
                        if ui
                            .add(
                                egui::Label::new(egui::RichText::new(o.id.to_string()).weak())
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Open order details")
                            .clicked()
                        {
                            open_details = true;
                        }
                    });
                    row.col(|ui| {
                        if ui
                            .add(
                                egui::Label::new(egui::RichText::new(&o.order_key).strong())
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Open order details")
                            .clicked()
                        {
                            open_details = true;
                        }
                    });
                    row.col(|ui| {
                        if ui
                            .add(
                                egui::Label::new(self.order_inventory_owner_label(&o))
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Open order details")
                            .clicked()
                        {
                            open_details = true;
                        }
                    });
                    row.col(|ui| {
                        self.forms.order_status.insert(o.id, o.status);
                        Self::order_state_badge(ui, &o);
                    });
                    row.col(|ui| {
                        if ui
                            .add(
                                egui::Label::new(format!(
                                    "{}, {} {}",
                                    o.city.clone().unwrap_or_default(),
                                    o.state.clone().unwrap_or_default(),
                                    o.postal_code.clone().unwrap_or_default()
                                ))
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Open order details")
                            .clicked()
                        {
                            open_details = true;
                        }
                    });
                    row.col(|ui| {
                        ui.menu_button(Self::icon(Icon::Ellipsis), |ui| {
                            ui.set_min_width(150.0);
                            if ui.button("Open details").clicked() {
                                open_details = true;
                                ui.close_menu();
                            }
                            if o.deleted.is_some() {
                                if ui.button("Restore order").clicked() {
                                    restore_order = true;
                                    ui.close_menu();
                                }
                            } else if o.status != OrderStatus::Shipped {
                                ui.separator();
                                if ui
                                    .add(egui::Button::new(
                                        egui::RichText::new("Delete order")
                                            .color(Self::danger_text_color(ui)),
                                    ))
                                    .clicked()
                                {
                                    delete_order = true;
                                    ui.close_menu();
                                }
                            }
                        });
                    });

                    if row.response().clicked() {
                        open_details = true;
                    }
                    if open_details {
                        self.open_order_ids.insert(o.id);
                        self.order_detail_focus = Some(o.id);
                        self.api.get_order_detail(o.id);
                    }
                    if delete_order {
                        self.request_delete(
                            format!("order {}", o.order_key),
                            "/api/orders/delete",
                            json!({"order_id": o.id}),
                            Screen::Orders,
                            "Order deleted",
                        );
                    }
                    if restore_order {
                        self.api.action(
                            "/api/orders/restore",
                            json!({"order_id": o.id}),
                            Screen::Orders,
                            "Order restored",
                        );
                    }
                });
            });

        match Self::pagination_footer(
            ui,
            "orders",
            self.order_offset,
            self.order_limit,
            self.data.orders.len(),
            self.order_total,
            &ORDER_PAGE_SIZES,
        ) {
            PaginationAction::None => {}
            PaginationAction::Previous => {
                self.order_offset = (self.order_offset - self.order_limit).max(0);
                self.fetch(Screen::Orders);
            }
            PaginationAction::Next => {
                self.order_offset += self.order_limit;
                self.fetch(Screen::Orders);
            }
            PaginationAction::PageSize(page_size) => {
                self.order_limit = page_size;
                self.order_offset = 0;
                self.fetch(Screen::Orders);
            }
        }

        let open_orders = self
            .open_order_ids
            .iter()
            .filter_map(|id| self.data.orders.iter().find(|order| order.id == *id))
            .cloned()
            .collect::<Vec<_>>();
        for order in open_orders {
            self.order_detail_window(ui.ctx(), &order);
        }
    }

    fn order_detail_window(&mut self, ctx: &egui::Context, order: &Order) {
        let mut open = true;
        let window_id = egui::Id::new(("order_detail", self.active_workspace_id, order.id));
        let mut tab = self
            .order_detail_tabs
            .get(&order.id)
            .copied()
            .unwrap_or_default();
        let mut refresh = false;
        let mut delete_order = false;
        let mut restore_order = false;
        egui::Window::new(format!("Order {}", order.order_key))
            .id(window_id)
            .default_pos(egui::pos2(210.0, 90.0))
            .default_width(800.0)
            .default_height(600.0)
            .constrain(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    Self::order_state_badge(ui, order);
                    Self::qty_chip(ui, "Ordered", order.ordered_qty, Self::accent_color(ui));
                    Self::qty_chip(ui, "Reserved", order.reserved_qty, Self::accent_color(ui));
                    if order.rush {
                        Self::summary_badge(ui, "Rush", 1, egui::Color32::from_rgb(245, 175, 55));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button(Self::icon(Icon::Ellipsis), |ui| {
                            ui.set_min_width(160.0);
                            if ui.button("Refresh details").clicked() {
                                refresh = true;
                                ui.close_menu();
                            }
                            if order.deleted.is_some() {
                                if ui.button("Restore order").clicked() {
                                    restore_order = true;
                                    ui.close_menu();
                                }
                            } else if order.status != OrderStatus::Shipped {
                                ui.separator();
                                if ui
                                    .add(egui::Button::new(
                                        egui::RichText::new("Delete order")
                                            .color(Self::danger_text_color(ui)),
                                    ))
                                    .clicked()
                                {
                                    delete_order = true;
                                    ui.close_menu();
                                }
                            }
                        });
                    });
                });
                ui.separator();

                ui.horizontal(|ui| {
                    ui.selectable_value(&mut tab, OrderDetailTab::Overview, "Overview");
                    ui.selectable_value(
                        &mut tab,
                        OrderDetailTab::Items,
                        format!("Items ({})", order.order_items.len()),
                    );
                    ui.selectable_value(
                        &mut tab,
                        OrderDetailTab::Reservations,
                        format!("Reservations ({})", order.reservations.len()),
                    );
                    ui.selectable_value(
                        &mut tab,
                        OrderDetailTab::Activity,
                        format!("Activity ({})", order.activity.len()),
                    );
                });
                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| match tab {
                    OrderDetailTab::Overview => self.order_overview(ui, order),
                    OrderDetailTab::Items => self.order_items_table(ui, order),
                    OrderDetailTab::Reservations => self.order_reservations_table(ui, order),
                    OrderDetailTab::Activity => self.order_activity(ui, order),
                });
            });

        if self.order_detail_focus == Some(order.id) {
            ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, window_id));
            ctx.request_repaint();
            self.order_detail_focus = None;
        }

        self.order_detail_tabs.insert(order.id, tab);
        if refresh {
            self.api.get_order_detail(order.id);
        }
        if delete_order {
            self.request_delete(
                format!("order {}", order.order_key),
                "/api/orders/delete",
                json!({"order_id": order.id}),
                Screen::Orders,
                "Order deleted",
            );
        }
        if restore_order {
            self.api.action(
                "/api/orders/restore",
                json!({"order_id": order.id}),
                Screen::Orders,
                "Order restored",
            );
        }
        if !open {
            self.open_order_ids.remove(&order.id);
            self.order_detail_tabs.remove(&order.id);
        }
    }

    fn order_overview(&self, ui: &mut egui::Ui, order: &Order) {
        ui.columns(2, |columns| {
            columns[0].strong("Order");
            egui::Grid::new(("order_detail_meta", order.id))
                .num_columns(2)
                .spacing([16.0, 7.0])
                .show(&mut columns[0], |ui| {
                    for (label, value) in [
                        ("Order key", order.order_key.clone()),
                        ("Inventory owner", self.order_inventory_owner_label(order)),
                        ("Created", Self::short_datetime(order.created)),
                        ("Ship by", Self::optional_datetime(order.ship_by)),
                        ("Confirmed", Self::optional_datetime(order.confirmed)),
                        ("Closed", Self::optional_datetime(order.closed)),
                        (
                            "Wave",
                            order
                                .wave_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "-".to_owned()),
                        ),
                    ] {
                        ui.weak(label);
                        ui.label(value);
                        ui.end_row();
                    }
                });

            columns[1].strong("Ship to");
            columns[1].label(format!(
                "{}{}",
                order.line1.clone().unwrap_or_default(),
                order
                    .line2
                    .as_ref()
                    .map(|line| format!("\n{line}"))
                    .unwrap_or_default()
            ));
            columns[1].label(format!(
                "{}, {} {}\n{}",
                order.city.clone().unwrap_or_default(),
                order.state.clone().unwrap_or_default(),
                order.postal_code.clone().unwrap_or_default(),
                order.country.clone().unwrap_or_default()
            ));
            columns[1].add_space(18.0);
            columns[1].strong("Tracking");
            if order.tracking_numbers.is_empty() {
                columns[1].weak("No tracking numbers");
            } else {
                columns[1].horizontal_wrapped(|ui| {
                    for tracking in &order.tracking_numbers {
                        let label = tracking
                            .carrier
                            .as_deref()
                            .filter(|carrier| !carrier.is_empty())
                            .unwrap_or("Tracking");
                        Self::tracking_chip(ui, label, &tracking.tracking_number);
                    }
                });
            }
        });
    }

    fn order_items_table(&self, ui: &mut egui::Ui, order: &Order) {
        if order.order_items.is_empty() {
            Self::empty_state(ui, Icon::Package, "No order items");
            return;
        }
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .column(Column::auto().at_least(70.0))
            .column(Column::auto().at_least(80.0))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::auto().at_least(70.0))
            .column(Column::auto().at_least(90.0))
            .header(26.0, |mut header| {
                for label in ["Line", "Item", "Description", "Qty", "Batch"] {
                    header.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|body| {
                body.rows(30.0, order.order_items.len(), |mut row| {
                    let item = &order.order_items[row.index()];
                    row.col(|ui| {
                        ui.weak(format!("#{}", item.id));
                    });
                    row.col(|ui| {
                        ui.label(item.item_id.to_string());
                    });
                    row.col(|ui| {
                        ui.label(item.item_description.as_deref().unwrap_or("-"));
                    });
                    row.col(|ui| {
                        ui.strong(item.qty.to_string());
                    });
                    row.col(|ui| {
                        ui.label(
                            item.item_batch_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "-".to_owned()),
                        );
                    });
                });
            });
    }

    fn order_reservations_table(&self, ui: &mut egui::Ui, order: &Order) {
        if order.reservations.is_empty() {
            Self::empty_state(ui, Icon::Boxes, "No reservations");
            return;
        }
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(100.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::remainder().at_least(120.0))
            .header(26.0, |mut header| {
                for label in ["Batch", "Location", "Qty", "Status", "Created"] {
                    header.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|body| {
                body.rows(30.0, order.reservations.len(), |mut row| {
                    let reservation = &order.reservations[row.index()];
                    row.col(|ui| {
                        ui.label(reservation.item_batch_id.to_string());
                    });
                    row.col(|ui| {
                        self.location_label_ui(ui, reservation.location_id);
                    });
                    row.col(|ui| {
                        ui.strong(reservation.qty.to_string());
                    });
                    row.col(|ui| {
                        ui.label(Self::title_case(&reservation.status.to_string()));
                    });
                    row.col(|ui| {
                        ui.label(Self::short_datetime(reservation.created));
                    });
                });
            });
    }

    fn order_activity(&self, ui: &mut egui::Ui, order: &Order) {
        if order.activity.is_empty() {
            Self::empty_state(ui, Icon::ClipboardList, "No activity recorded");
            return;
        }
        for activity in &order.activity {
            ui.horizontal_wrapped(|ui| {
                ui.weak(Self::short_datetime(activity.created));
                ui.separator();
                ui.label(&activity.action);
            });
            ui.separator();
        }
    }

    // ---- Users -----------------------------------------------------------
    pub(super) fn users_screen(&mut self, ui: &mut egui::Ui) {
        let users = self.data.users.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for u in &users {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(format!("#{} {}", u.id, u.email));
                        if u.deleted.is_some() {
                            ui.colored_label(egui::Color32::LIGHT_RED, "(deleted)");
                        }
                    });
                    let k = |f: &str| format!("user:{}:{}", u.id, f);
                    let mut first = self
                        .forms
                        .drafts
                        .get(&k("first"))
                        .cloned()
                        .unwrap_or_else(|| u.first_name.clone().unwrap_or_default());
                    let mut last = self
                        .forms
                        .drafts
                        .get(&k("last"))
                        .cloned()
                        .unwrap_or_else(|| u.last_name.clone().unwrap_or_default());
                    let mut phone = self
                        .forms
                        .drafts
                        .get(&k("phone"))
                        .cloned()
                        .unwrap_or_else(|| u.phone.clone().unwrap_or_default());
                    let mut role_in = self
                        .forms
                        .drafts
                        .get(&k("role_id"))
                        .cloned()
                        .unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.label("First");
                        ui.text_edit_singleline(&mut first);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Last");
                        ui.text_edit_singleline(&mut last);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Phone");
                        ui.text_edit_singleline(&mut phone);
                    });
                    ui.label(format!(
                        "Roles: {}",
                        u.user_roles
                            .iter()
                            .map(|r| r.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    ui.label(format!(
                        "Permissions: {}",
                        u.user_permissions
                            .iter()
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            let body = json!({
                                "user_id": u.id,
                                "first_name": first,
                                "last_name": last,
                                "phone": phone,
                            });
                            self.api.action(
                                "/api/users/update",
                                body,
                                Screen::Users,
                                "User updated",
                            );
                        }
                        if u.deleted.is_none() {
                            self.delete_button(
                                ui,
                                false,
                                format!("user {}", u.email),
                                "/api/users/delete",
                                json!({"user_id": u.id}),
                                Screen::Users,
                                "User deleted",
                            );
                        } else if ui.button("Restore").clicked() {
                            self.api.action(
                                "/api/users/restore",
                                json!({"user_id": u.id}),
                                Screen::Users,
                                "User restored",
                            );
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Role id");
                        ui.text_edit_singleline(&mut role_in);
                        let rid = role_in.parse::<i64>().ok();
                        if ui.small_button("Add").clicked() {
                            if let Some(rid) = rid {
                                self.api.action(
                                    "/api/users/roles/add",
                                    json!({"user_id": u.id, "role_id": rid}),
                                    Screen::Users,
                                    "Role added",
                                );
                            }
                        }
                        if ui.small_button("Remove").clicked() {
                            if let Some(rid) = rid {
                                self.api.action(
                                    "/api/users/roles/delete",
                                    json!({"user_id": u.id, "role_id": rid}),
                                    Screen::Users,
                                    "Role removed",
                                );
                            }
                        }
                    });
                    self.forms.drafts.insert(k("first"), first);
                    self.forms.drafts.insert(k("last"), last);
                    self.forms.drafts.insert(k("phone"), phone);
                    self.forms.drafts.insert(k("role_id"), role_in);
                });
            }
        });
    }

    // ---- Roles -----------------------------------------------------------
    pub(super) fn roles_screen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("New role", |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut self.forms.new_name);
                ui.label("Description");
                ui.text_edit_singleline(&mut self.forms.new_desc);
                if ui.button("Create").clicked() {
                    self.api.action(
                        "/api/roles/add",
                        json!({"name": self.forms.new_name, "description": self.forms.new_desc}),
                        Screen::Roles,
                        "Role created",
                    );
                    self.forms.new_name.clear();
                    self.forms.new_desc.clear();
                }
            });
        });
        ui.separator();
        let roles = self.data.roles.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for r in &roles {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(format!("#{} {}", r.id, r.name));
                        if let Some(d) = &r.description {
                            ui.label(d);
                        }
                        if r.is_self_role() {
                            ui.colored_label(egui::Color32::GRAY, "(self role)");
                        }
                        if r.deleted.is_some() {
                            ui.colored_label(egui::Color32::LIGHT_RED, "(deleted)");
                        }
                    });
                    ui.label(format!(
                        "Permissions: {}",
                        r.role_permissions
                            .iter()
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    if !r.is_self_role() {
                        ui.horizontal(|ui| {
                            if r.deleted.is_none() {
                                self.delete_button(
                                    ui,
                                    true,
                                    format!("role {}", r.name),
                                    "/api/roles/delete",
                                    json!({"role_id": r.id}),
                                    Screen::Roles,
                                    "Role deleted",
                                );
                            } else if ui.small_button("Restore").clicked() {
                                self.api.action(
                                    "/api/roles/restore",
                                    json!({"role_id": r.id}),
                                    Screen::Roles,
                                    "Role restored",
                                );
                            }
                            let pk = format!("role:{}:perm", r.id);
                            let mut pin = self.forms.drafts.get(&pk).cloned().unwrap_or_default();
                            ui.label("Perm id");
                            ui.text_edit_singleline(&mut pin);
                            let pid = pin.parse::<i64>().ok();
                            if ui.small_button("Add perm").clicked() {
                                if let Some(pid) = pid {
                                    self.api.action(
                                        "/api/roles/permissions/add",
                                        json!({"role_id": r.id, "permission_id": pid}),
                                        Screen::Roles,
                                        "Permission added",
                                    );
                                }
                            }
                            if ui.small_button("Remove perm").clicked() {
                                if let Some(pid) = pid {
                                    self.api.action(
                                        "/api/roles/permissions/delete",
                                        json!({"role_id": r.id, "permission_id": pid}),
                                        Screen::Roles,
                                        "Permission removed",
                                    );
                                }
                            }
                            self.forms.drafts.insert(pk, pin);
                        });
                    }
                });
            }
        });
    }

    // ---- Permissions -----------------------------------------------------
    pub(super) fn permissions_screen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("New permission", |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut self.forms.new_name);
                ui.label("Description");
                ui.text_edit_singleline(&mut self.forms.new_desc);
                if ui.button("Create").clicked() {
                    self.api.action(
                        "/api/permissions/add",
                        json!({"name": self.forms.new_name, "description": self.forms.new_desc}),
                        Screen::Permissions,
                        "Permission created",
                    );
                    self.forms.new_name.clear();
                    self.forms.new_desc.clear();
                }
            });
        });
        ui.separator();
        let perms = self.data.permissions.clone();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().at_least(40.0)) // ID
            .column(Column::initial(160.0).at_least(100.0).clip(true)) // Name
            .column(Column::remainder().at_least(160.0).clip(true)) // Description
            .column(Column::auto().at_least(90.0)) // Actions
            .header(26.0, |mut h| {
                for label in ["ID", "Name", "Description", "Actions"] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|mut body| {
                for p in &perms {
                    body.row(28.0, |mut row| {
                        row.col(|ui| {
                            ui.label(p.id.to_string());
                        });
                        row.col(|ui| {
                            ui.label(&p.name);
                        });
                        row.col(|ui| {
                            ui.label(p.description.clone().unwrap_or_default());
                        });
                        row.col(|ui| {
                            if p.deleted.is_none() {
                                self.delete_button(
                                    ui,
                                    true,
                                    format!("permission {}", p.name),
                                    "/api/permissions/delete",
                                    json!({"permission_id": p.id}),
                                    Screen::Permissions,
                                    "Permission deleted",
                                );
                            } else if ui.small_button("Restore").clicked() {
                                self.api.action(
                                    "/api/permissions/restore",
                                    json!({"permission_id": p.id}),
                                    Screen::Permissions,
                                    "Permission restored",
                                );
                            }
                        });
                    });
                }
            });
    }

    // ---- Inventory Owners ------------------------------------------------------
    pub(super) fn inventory_owners_screen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("New inventory owner", |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut self.forms.new_name);
                ui.label("Email");
                ui.text_edit_singleline(&mut self.forms.new_email);
                if ui.button("Create").clicked() {
                    self.api.action(
                        "/api/inventory-owners/add",
                        json!({"name": self.forms.new_name, "email": self.forms.new_email}),
                        Screen::InventoryOwners,
                        "Inventory owner created",
                    );
                    self.forms.new_name.clear();
                    self.forms.new_email.clear();
                }
            });
        });
        ui.separator();
        let inventory_owners = self.data.inventory_owners.clone();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().at_least(40.0)) // ID
            .column(Column::initial(160.0).at_least(100.0).clip(true)) // Name
            .column(Column::remainder().at_least(160.0).clip(true)) // Email
            .column(Column::auto().at_least(90.0)) // Facilities
            .column(Column::auto().at_least(90.0)) // Actions
            .header(26.0, |mut h| {
                for label in ["ID", "Name", "Email", "Facilities", "Actions"] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|mut body| {
                for a in &inventory_owners {
                    body.row(28.0, |mut row| {
                        row.col(|ui| {
                            ui.label(a.id.to_string());
                        });
                        row.col(|ui| {
                            ui.label(&a.name);
                        });
                        row.col(|ui| {
                            ui.label(&a.email);
                        });
                        row.col(|ui| {
                            ui.label(a.inventory_owner_facilities.len().to_string());
                        });
                        row.col(|ui| {
                            if a.deleted.is_none() {
                                self.delete_button(
                                    ui,
                                    true,
                                    format!("inventory owner {}", a.name),
                                    "/api/inventory-owners/delete",
                                    json!({"inventory_owner_id": a.id}),
                                    Screen::InventoryOwners,
                                    "Inventory owner deleted",
                                );
                            } else if ui.small_button("Restore").clicked() {
                                self.api.action(
                                    "/api/inventory-owners/restore",
                                    json!({"inventory_owner_id": a.id}),
                                    Screen::InventoryOwners,
                                    "Inventory owner restored",
                                );
                            }
                        });
                    });
                }
            });
    }

    // ---- Facilities (read-only, mirrors the original getFacilities) ------
    pub(super) fn facilities_screen(&mut self, ui: &mut egui::Ui) {
        let facilities = self.data.facilities.clone();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().at_least(40.0)) // ID
            .column(Column::remainder().at_least(160.0).clip(true)) // Name
            .column(Column::initial(200.0).at_least(140.0).clip(true)) // Created
            .column(Column::auto().at_least(80.0)) // Status
            .header(26.0, |mut h| {
                for label in ["ID", "Name", "Created", "Status"] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|mut body| {
                for w in &facilities {
                    body.row(28.0, |mut row| {
                        row.col(|ui| {
                            ui.label(w.id.to_string());
                        });
                        row.col(|ui| {
                            ui.label(w.name.clone().unwrap_or_default());
                        });
                        row.col(|ui| {
                            ui.label(Self::short_datetime(w.created));
                        });
                        row.col(|ui| {
                            if w.deleted.is_some() {
                                ui.colored_label(egui::Color32::LIGHT_RED, "deleted");
                            } else {
                                ui.label("active");
                            }
                        });
                    });
                }
            });
    }

    // ---- Item Master -----------------------------------------------------
    pub(super) fn new_item_window(&mut self, ctx: &egui::Context) {
        if !self.new_item_open {
            return;
        }

        let mut open = self.new_item_open;
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new("New item")
            .id(egui::Id::new(("new_item", self.active_workspace_id)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .constrain(false)
            .default_pos(egui::pos2(190.0, 120.0))
            .fixed_size([420.0, 160.0])
            .show(ctx, |ui| {
                egui::Grid::new("new_item_fields")
                    .num_columns(2)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Description");
                        ui.add_sized(
                            [260.0, 32.0],
                            egui::TextEdit::singleline(&mut self.forms.new_desc),
                        );
                        ui.end_row();
                        ui.label("Packaging");
                        Self::select_from_allowed(
                            ui,
                            "new_item_packaging_unit",
                            &mut self.forms.new_packaging_unit,
                            &PACKAGING_UNITS,
                        );
                        ui.end_row();
                    });
                ui.add_space(10.0);
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let create = egui::Button::new(
                        egui::RichText::new("Create item")
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(Self::accent_color(ui));
                    if ui.add(create).clicked() {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if submit {
            if self.forms.new_desc.trim().is_empty() {
                self.toast("Enter an item description", true, self.now);
            } else {
                self.api.action(
                    "/api/items/add",
                    json!({
                        "description": self.forms.new_desc,
                        "packaging_unit": self.forms.new_packaging_unit,
                    }),
                    Screen::Items,
                    "Item created",
                );
                self.forms.new_desc.clear();
                open = false;
            }
        }
        if cancel {
            open = false;
        }
        self.new_item_open = open;
    }

    pub(super) fn items_screen(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let new_item = egui::Button::new(
                egui::RichText::new("New item")
                    .strong()
                    .color(egui::Color32::WHITE),
            )
            .fill(Self::accent_color(ui));
            if ui.add(new_item).clicked() {
                self.new_item_open = true;
            }
            ui.separator();
            ui.weak(format!("{} items", self.data.items.len()));
        });
        ui.separator();

        let mut search = self
            .forms
            .drafts
            .get("items:search")
            .cloned()
            .unwrap_or_default();
        egui::Frame::none()
            .fill(ui.visuals().faint_bg_color)
            .rounding(egui::Rounding::same(4.0))
            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut search)
                            .desired_width(300.0)
                            .hint_text("Description, SKU, or barcode"),
                    );
                    if Self::icon_button(ui, Icon::RotateCcw, "Clear search").clicked() {
                        search.clear();
                    }
                });
            });
        self.forms
            .drafts
            .insert("items:search".to_owned(), search.clone());
        ui.add_space(6.0);

        let needle = search.trim().to_ascii_lowercase();
        let items = self
            .data
            .items
            .iter()
            .filter(|item| {
                needle.is_empty()
                    || item
                        .description
                        .as_deref()
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .contains(&needle)
                    || item
                        .skus
                        .iter()
                        .any(|sku| sku.name.to_ascii_lowercase().contains(&needle))
                    || item
                        .barcodes
                        .iter()
                        .any(|barcode| barcode.name.to_ascii_lowercase().contains(&needle))
            })
            .cloned()
            .collect::<Vec<_>>();

        if items.is_empty() {
            Self::empty_state(ui, Icon::Package, "No items match this view");
        } else {
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .auto_shrink([false, false])
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::auto().at_least(44.0))
                .column(Column::remainder().at_least(220.0).clip(true))
                .column(Column::initial(110.0).at_least(90.0))
                .column(Column::auto().at_least(70.0))
                .column(Column::auto().at_least(86.0))
                .column(Column::auto().at_least(80.0))
                .column(Column::auto().at_least(54.0))
                .header(26.0, |mut header| {
                    for label in [
                        "ID",
                        "Description",
                        "Packaging",
                        "SKUs",
                        "Barcodes",
                        "Status",
                        "",
                    ] {
                        header.col(|ui| {
                            ui.strong(label);
                        });
                    }
                })
                .body(|body| {
                    body.rows(36.0, items.len(), |mut row| {
                        let item = items[row.index()].clone();
                        let selected = self.open_item_ids.contains(&item.id);
                        row.set_selected(selected);
                        let mut open_details = false;
                        let mut delete_item = false;
                        let mut restore_item = false;
                        row.col(|ui| {
                            if ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(item.id.to_string()).weak(),
                                    )
                                    .sense(egui::Sense::click()),
                                )
                                .clicked()
                            {
                                open_details = true;
                            }
                        });
                        row.col(|ui| {
                            if ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(
                                            item.description.as_deref().unwrap_or("Unnamed item"),
                                        )
                                        .strong(),
                                    )
                                    .sense(egui::Sense::click()),
                                )
                                .clicked()
                            {
                                open_details = true;
                            }
                        });
                        row.col(|ui| {
                            Self::packaging_unit_badge(ui, &item.packaging_unit);
                        });
                        row.col(|ui| {
                            ui.label(item.skus.len().to_string());
                        });
                        row.col(|ui| {
                            ui.label(item.barcodes.len().to_string());
                        });
                        row.col(|ui| {
                            if item.deleted.is_some() {
                                ui.colored_label(Self::danger_text_color(ui), "Deleted");
                            } else {
                                ui.weak("Active");
                            }
                        });
                        row.col(|ui| {
                            ui.menu_button(Self::icon(Icon::Ellipsis), |ui| {
                                ui.set_min_width(150.0);
                                if ui.button("Open item").clicked() {
                                    open_details = true;
                                    ui.close_menu();
                                }
                                if item.deleted.is_some() {
                                    if ui.button("Restore item").clicked() {
                                        restore_item = true;
                                        ui.close_menu();
                                    }
                                } else {
                                    ui.separator();
                                    if ui
                                        .add(egui::Button::new(
                                            egui::RichText::new("Delete item")
                                                .color(Self::danger_text_color(ui)),
                                        ))
                                        .clicked()
                                    {
                                        delete_item = true;
                                        ui.close_menu();
                                    }
                                }
                            });
                        });
                        if row.response().clicked() {
                            open_details = true;
                        }
                        if open_details {
                            self.open_item_ids.insert(item.id);
                            self.item_detail_focus = Some(item.id);
                        }
                        if delete_item {
                            self.request_delete(
                                format!(
                                    "item {}",
                                    item.description.as_deref().unwrap_or("unnamed")
                                ),
                                "/api/items/delete",
                                json!({"item_id": item.id}),
                                Screen::Items,
                                "Item deleted",
                            );
                        }
                        if restore_item {
                            self.api.action(
                                "/api/items/restore",
                                json!({"item_id": item.id}),
                                Screen::Items,
                                "Item restored",
                            );
                        }
                    });
                });
        }

        let open_items = self
            .open_item_ids
            .iter()
            .filter_map(|id| self.data.items.iter().find(|item| item.id == *id))
            .cloned()
            .collect::<Vec<_>>();
        for item in open_items {
            self.item_detail_window(ui.ctx(), &item);
        }
    }

    fn item_detail_window(&mut self, ctx: &egui::Context, item: &Item) {
        let mut open = true;
        let window_id = egui::Id::new(("item_detail", self.active_workspace_id, item.id));
        let mut tab = self
            .item_detail_tabs
            .get(&item.id)
            .copied()
            .unwrap_or_default();
        let mut delete_item = false;
        let mut restore_item = false;
        egui::Window::new(format!(
            "Item #{} · {}",
            item.id,
            item.description.as_deref().unwrap_or("Unnamed")
        ))
        .id(window_id)
        .open(&mut open)
        .constrain(false)
        .default_pos(egui::pos2(230.0, 105.0))
        .default_width(760.0)
        .default_height(560.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                Self::packaging_unit_badge(ui, &item.packaging_unit);
                ui.weak(format!("{} SKUs", item.skus.len()));
                ui.separator();
                ui.weak(format!("{} barcodes", item.barcodes.len()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button(Self::icon(Icon::Ellipsis), |ui| {
                        if item.deleted.is_some() {
                            if ui.button("Restore item").clicked() {
                                restore_item = true;
                                ui.close_menu();
                            }
                        } else if ui
                            .add(egui::Button::new(
                                egui::RichText::new("Delete item")
                                    .color(Self::danger_text_color(ui)),
                            ))
                            .clicked()
                        {
                            delete_item = true;
                            ui.close_menu();
                        }
                    });
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut tab,
                    ItemDetailTab::Skus,
                    format!("SKUs ({})", item.skus.len()),
                );
                ui.selectable_value(
                    &mut tab,
                    ItemDetailTab::Barcodes,
                    format!("Barcodes ({})", item.barcodes.len()),
                );
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| match tab {
                ItemDetailTab::Skus => self.item_skus_editor(ui, item),
                ItemDetailTab::Barcodes => self.item_barcodes_editor(ui, item),
            });
        });

        if self.item_detail_focus == Some(item.id) {
            ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, window_id));
            ctx.request_repaint();
            self.item_detail_focus = None;
        }
        self.item_detail_tabs.insert(item.id, tab);
        if delete_item {
            self.request_delete(
                format!("item {}", item.description.as_deref().unwrap_or("unnamed")),
                "/api/items/delete",
                json!({"item_id": item.id}),
                Screen::Items,
                "Item deleted",
            );
        }
        if restore_item {
            self.api.action(
                "/api/items/restore",
                json!({"item_id": item.id}),
                Screen::Items,
                "Item restored",
            );
        }
        if !open {
            self.open_item_ids.remove(&item.id);
            self.item_detail_tabs.remove(&item.id);
        }
    }

    fn item_skus_editor(&mut self, ui: &mut egui::Ui, item: &Item) {
        let key = format!("item:{}:sku", item.id);
        let mut sku = self.forms.drafts.get(&key).cloned().unwrap_or_default();
        let mut add_sku = false;
        egui::Frame::none()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut sku)
                            .desired_width(260.0)
                            .hint_text("New SKU"),
                    );
                    if ui
                        .add_enabled(!sku.trim().is_empty(), egui::Button::new("Add SKU"))
                        .clicked()
                    {
                        add_sku = true;
                    }
                });
            });
        if add_sku {
            self.api.action(
                "/api/items/skus/add",
                json!({"item_id": item.id, "name": sku}),
                Screen::Items,
                "SKU added",
            );
            sku.clear();
        }
        self.forms.drafts.insert(key, sku);
        ui.add_space(8.0);
        if item.skus.is_empty() {
            Self::empty_state(ui, Icon::Package, "No SKUs assigned");
            return;
        }
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::auto().at_least(60.0))
            .column(Column::remainder().at_least(240.0))
            .column(Column::remainder().at_least(180.0))
            .header(26.0, |mut header| {
                for label in ["ID", "SKU", "Notes"] {
                    header.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|body| {
                body.rows(32.0, item.skus.len(), |mut row| {
                    let sku = &item.skus[row.index()];
                    row.col(|ui| {
                        ui.weak(sku.id.to_string());
                    });
                    row.col(|ui| {
                        ui.strong(&sku.name);
                    });
                    row.col(|ui| {
                        ui.label(sku.notes.as_deref().unwrap_or("-"));
                    });
                });
            });
    }

    fn item_barcodes_editor(&mut self, ui: &mut egui::Ui, item: &Item) {
        let value_key = format!("item:{}:barcode", item.id);
        let type_key = format!("item:{}:barcode_type", item.id);
        let mut barcode = self
            .forms
            .drafts
            .get(&value_key)
            .cloned()
            .unwrap_or_default();
        let mut barcode_type = self
            .forms
            .drafts
            .get(&type_key)
            .cloned()
            .unwrap_or_else(|| "code128".to_owned());
        let mut add_barcode = false;
        let barcode_error = Self::barcode_validation_error(&barcode, &barcode_type);
        let can_use_barcode = !barcode.trim().is_empty() && barcode_error.is_none();
        egui::Frame::none()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut barcode)
                            .desired_width(260.0)
                            .hint_text("Barcode value"),
                    );
                    Self::select_from_allowed(
                        ui,
                        format!("item_{}_barcode_type", item.id),
                        &mut barcode_type,
                        &BARCODE_TYPES,
                    );
                    if ui
                        .add_enabled(can_use_barcode, egui::Button::new("Add barcode"))
                        .clicked()
                    {
                        add_barcode = true;
                    }
                    if ui
                        .add_enabled(can_use_barcode, egui::Button::new("Save draft SVG"))
                        .clicked()
                    {
                        self.save_barcode_svg(&barcode, &barcode_type);
                    }
                });
            });
        if let Some(error) = Self::barcode_validation_error(&barcode, &barcode_type) {
            ui.colored_label(
                Self::danger_text_color(ui),
                format!("Invalid barcode: {error}"),
            );
        } else if !barcode.trim().is_empty() {
            Self::barcode_preview(ui, &barcode, &barcode_type, false);
        }
        if add_barcode {
            self.api.action(
                "/api/items/barcodes/add",
                json!({"item_id": item.id, "name": barcode, "type": barcode_type}),
                Screen::Items,
                "Barcode added",
            );
            barcode.clear();
        }
        self.forms.drafts.insert(value_key, barcode);
        self.forms.drafts.insert(type_key, barcode_type);
        ui.add_space(8.0);
        if item.barcodes.is_empty() {
            Self::empty_state(ui, Icon::ScanBarcode, "No barcodes assigned");
            return;
        }
        for existing in &item.barcodes {
            ui.horizontal(|ui| {
                Self::barcode_preview(ui, &existing.name, &existing.r#type, true);
                ui.vertical(|ui| {
                    ui.strong(&existing.name);
                    ui.weak(Self::barcode_type_label(&existing.r#type));
                    ui.horizontal(|ui| {
                        if ui.button("Save SVG").clicked() {
                            self.save_barcode_svg(&existing.name, &existing.r#type);
                        }
                        self.delete_button(
                            ui,
                            true,
                            format!("barcode {}", existing.name),
                            "/api/items/barcodes/delete",
                            json!({"barcode_id": existing.id}),
                            Screen::Items,
                            "Barcode deleted",
                        );
                    });
                });
            });
            ui.separator();
        }
    }

    // ---- Locations -------------------------------------------------------
    pub(super) fn locations_screen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("New location", |ui| {
            let facility_options = self.facility_options();
            ui.horizontal_wrapped(|ui| {
                ui.label("Facility");
                let facility_id = Self::entity_picker(
                    ui,
                    "new_location_facility",
                    &mut self.forms.new_facility_id,
                    &facility_options,
                    "Search facility",
                );
                ui.label("Name");
                ui.text_edit_singleline(&mut self.forms.new_name);
                ui.label("Type");
                ui.text_edit_singleline(&mut self.forms.new_type);
                if ui.button("Create").clicked() {
                    if let Some(facility_id) = facility_id {
                        self.api.action(
                            "/api/locations/add",
                            json!({
                                "facility_id": facility_id,
                                "name": self.forms.new_name,
                                "type": self.forms.new_type,
                            }),
                            Screen::Locations,
                            "Location created",
                        );
                        self.forms.new_name.clear();
                    } else {
                        self.toast("Choose a facility", true, self.now);
                    }
                }
            });
        });
        ui.separator();

        let locations = self.data.locations.clone();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().at_least(40.0))
            .column(Column::initial(110.0))
            .column(Column::remainder().at_least(140.0).clip(true))
            .column(Column::initial(100.0))
            .column(Column::auto().at_least(90.0))
            .header(26.0, |mut h| {
                for label in ["ID", "Facility", "Scan Code", "Type", "Actions"] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|mut body| {
                for loc in &locations {
                    body.row(28.0, |mut row| {
                        row.col(|ui| {
                            ui.label(loc.id.to_string());
                        });
                        row.col(|ui| {
                            ui.label(
                                loc.facility_name
                                    .clone()
                                    .unwrap_or_else(|| self.facility_label(loc.facility_id)),
                            );
                        });
                        row.col(|ui| {
                            self.location_label_ui(ui, loc.id);
                        });
                        row.col(|ui| {
                            ui.label(&loc.r#type);
                        });
                        row.col(|ui| {
                            if loc.deleted.is_none() {
                                self.delete_button(
                                    ui,
                                    true,
                                    format!(
                                        "location {}",
                                        loc.name.as_deref().unwrap_or("unnamed")
                                    ),
                                    "/api/locations/delete",
                                    json!({"location_id": loc.id}),
                                    Screen::Locations,
                                    "Location deleted",
                                );
                            } else if ui.small_button("Restore").clicked() {
                                self.api.action(
                                    "/api/locations/restore",
                                    json!({"location_id": loc.id}),
                                    Screen::Locations,
                                    "Location restored",
                                );
                            }
                        });
                    });
                }
            });
    }
}
