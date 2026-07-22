use super::*;

impl WareboxesApp {
    // ---- Orders ----------------------------------------------------------
    pub(super) fn orders_screen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("New order", |ui| {
            egui::Grid::new("new_order").num_columns(2).show(ui, |ui| {
                ui.label("Order key");
                ui.text_edit_singleline(&mut self.forms.o_key);
                ui.end_row();
                ui.label("Line 1");
                ui.text_edit_singleline(&mut self.forms.o_line1);
                ui.end_row();
                ui.label("City");
                ui.text_edit_singleline(&mut self.forms.o_city);
                ui.end_row();
                ui.label("State");
                ui.text_edit_singleline(&mut self.forms.o_state);
                ui.end_row();
                ui.label("Postal code");
                ui.text_edit_singleline(&mut self.forms.o_zip);
                ui.end_row();
                ui.label("Country");
                ui.text_edit_singleline(&mut self.forms.o_country);
                ui.end_row();
                ui.checkbox(&mut self.forms.o_rush, "Rush");
                ui.end_row();
            });
            if ui.button("Create order").clicked() {
                let body = json!({
                    "order_key": self.forms.o_key,
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
            }
        });
        ui.separator();

        if self.data.order_summaries.is_empty() {
            ui.weak("No active order exceptions in this view.");
        } else {
            ui.horizontal_wrapped(|ui| {
                for summary in &self.data.order_summaries {
                    Self::summary_badge(
                        ui,
                        &summary.label,
                        summary.count,
                        Self::order_summary_color(ui, &summary.key),
                    );
                }
            });
        }
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
        ui.horizontal_wrapped(|ui| {
            ui.label("Search");
            let search_response = ui.add(
                egui::TextEdit::singleline(&mut search)
                    .desired_width(220.0)
                    .hint_text("Order, account, city, state, postal"),
            );
            let search_submitted = search_response.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui.button("Apply").clicked() || search_submitted {
                self.order_offset = 0;
                refresh_orders = true;
            }

            ui.label("Status");
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

            ui.label("Page size");
            let before_limit = self.order_limit;
            egui::ComboBox::from_id_source("orders_page_size")
                .selected_text(self.order_limit.to_string())
                .show_ui(ui, |ui| {
                    for size in ORDER_PAGE_SIZES {
                        ui.selectable_value(&mut self.order_limit, size, size.to_string());
                    }
                });
            if self.order_limit != before_limit {
                self.order_offset = 0;
                refresh_orders = true;
            }

            if ui.button("Reset").clicked() {
                search.clear();
                status_filter.clear();
                self.order_offset = 0;
                refresh_orders = true;
            }
        });
        self.forms.drafts.insert("orders:search".to_owned(), search);
        self.forms
            .drafts
            .insert("orders:status".to_owned(), status_filter);

        let page_start = if self.order_total == 0 {
            0
        } else {
            self.order_offset + 1
        };
        let page_end = (self.order_offset + self.data.orders.len() as i64).min(self.order_total);
        ui.horizontal_wrapped(|ui| {
            ui.weak(format!(
                "Showing {page_start}-{page_end} of {} orders",
                self.order_total
            ));
            let can_prev = self.order_offset > 0;
            if ui
                .add_enabled(can_prev, egui::Button::new("Previous"))
                .clicked()
            {
                self.order_offset = (self.order_offset - self.order_limit).max(0);
                refresh_orders = true;
            }
            let can_next = self.order_offset + self.order_limit < self.order_total;
            if ui
                .add_enabled(can_next, egui::Button::new("Next"))
                .clicked()
            {
                self.order_offset += self.order_limit;
                refresh_orders = true;
            }
        });
        if refresh_orders {
            self.fetch(Screen::Orders);
        }
        ui.separator();

        let order_count = self.data.orders.len();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().at_least(40.0)) // ID
            .column(Column::initial(140.0).at_least(90.0).clip(true)) // Key
            .column(Column::initial(140.0).at_least(100.0).clip(true)) // Account
            .column(Column::initial(160.0).at_least(130.0)) // Status
            .column(Column::remainder().at_least(160.0).clip(true)) // Ship to
            .column(Column::auto().at_least(190.0)) // Actions
            .header(26.0, |mut h| {
                for label in ["ID", "Key", "Account", "Status", "Ship to", "Actions"] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|body| {
                body.rows(34.0, order_count, |mut row| {
                    let o = self.data.orders[row.index()].clone();
                    row.col(|ui| {
                        let color = Self::order_state_color(ui, &o);
                        let selected = self.open_order_ids.contains(&o.id);
                        if ui
                            .selectable_label(
                                selected,
                                egui::RichText::new(o.id.to_string()).strong().color(color),
                            )
                            .on_hover_text("Open order details")
                            .clicked()
                        {
                            self.open_order_ids.insert(o.id);
                            self.api.get_order_detail(o.id);
                        }
                    });
                    row.col(|ui| {
                        let color = Self::order_state_color(ui, &o);
                        let selected = self.open_order_ids.contains(&o.id);
                        if ui
                            .selectable_label(
                                selected,
                                egui::RichText::new(&o.order_key).strong().color(color),
                            )
                            .on_hover_text("Open order details")
                            .clicked()
                        {
                            self.open_order_ids.insert(o.id);
                            self.api.get_order_detail(o.id);
                        }
                    });
                    row.col(|ui| {
                        ui.colored_label(
                            Self::order_state_color(ui, &o),
                            self.order_account_label(&o),
                        );
                    });
                    row.col(|ui| {
                        self.forms.order_status.insert(o.id, o.status);
                        Self::order_state_badge(ui, &o);
                    });
                    row.col(|ui| {
                        ui.colored_label(
                            Self::order_state_color(ui, &o),
                            format!(
                                "{}, {} {}",
                                o.city.clone().unwrap_or_default(),
                                o.state.clone().unwrap_or_default(),
                                o.postal_code.clone().unwrap_or_default()
                            ),
                        );
                    });
                    row.col(|ui| {
                        ui.horizontal(|ui| {
                            if ui.small_button("Details").clicked() {
                                self.open_order_ids.insert(o.id);
                                self.api.get_order_detail(o.id);
                            }
                            if o.status == OrderStatus::Shipped {
                                ui.weak("Shipped");
                            } else if o.deleted.is_none() {
                                self.delete_button(
                                    ui,
                                    true,
                                    format!("order {}", o.order_key),
                                    "/api/orders/delete",
                                    json!({"order_id": o.id}),
                                    Screen::Orders,
                                    "Order deleted",
                                );
                            } else if ui.small_button("Restore").clicked() {
                                self.api.action(
                                    "/api/orders/restore",
                                    json!({"order_id": o.id}),
                                    Screen::Orders,
                                    "Order restored",
                                );
                            }
                        });
                    });
                });
            });

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
        egui::Window::new(format!("Order {}", order.order_key))
            .id(egui::Id::new(("order_detail", order.id)))
            .default_width(720.0)
            .default_height(620.0)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        Self::order_state_badge(ui, order);
                        Self::qty_chip(
                            ui,
                            "Ordered",
                            order.ordered_qty,
                            Self::order_state_color(ui, order),
                        );
                        Self::qty_chip(
                            ui,
                            "Reserved",
                            order.reserved_qty,
                            Self::order_state_color(ui, order),
                        );
                        if order.rush {
                            Self::summary_badge(
                                ui,
                                "Rush",
                                1,
                                egui::Color32::from_rgb(245, 175, 55),
                            );
                        }
                    });
                    ui.separator();

                    ui.heading("Order");
                    egui::Grid::new(("order_detail_meta", order.id))
                        .num_columns(2)
                        .spacing([16.0, 6.0])
                        .show(ui, |ui| {
                            ui.strong("Order key");
                            ui.label(&order.order_key);
                            ui.end_row();
                            ui.strong("Created");
                            ui.label(Self::short_datetime(order.created));
                            ui.end_row();
                            ui.strong("Ship by");
                            ui.label(Self::optional_datetime(order.ship_by));
                            ui.end_row();
                            ui.strong("Confirmed");
                            ui.label(Self::optional_datetime(order.confirmed));
                            ui.end_row();
                            ui.strong("Closed");
                            ui.label(Self::optional_datetime(order.closed));
                            ui.end_row();
                            ui.strong("Account");
                            ui.label(self.order_account_label(order));
                            ui.end_row();
                            ui.strong("Wave");
                            ui.label(
                                order
                                    .wave_id
                                    .map(|id| id.to_string())
                                    .unwrap_or_else(|| "-".to_owned()),
                            );
                            ui.end_row();
                        });

                    ui.add_space(8.0);
                    ui.heading("Ship To");
                    ui.label(
                        egui::RichText::new(format!(
                            "{}{}{}",
                            order.line1.clone().unwrap_or_default(),
                            order
                                .line2
                                .as_ref()
                                .map(|line2| format!(", {line2}"))
                                .unwrap_or_default(),
                            if order.line1.is_some() || order.line2.is_some() {
                                "\n"
                            } else {
                                ""
                            }
                        ))
                        .color(Self::load_detail_text_color(ui)),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{}, {} {} {}",
                            order.city.clone().unwrap_or_default(),
                            order.state.clone().unwrap_or_default(),
                            order.postal_code.clone().unwrap_or_default(),
                            order.country.clone().unwrap_or_default()
                        ))
                        .color(Self::load_detail_text_color(ui)),
                    );

                    ui.add_space(8.0);
                    ui.heading("Items");
                    if order.order_items.is_empty() {
                        ui.weak("No order items.");
                    } else {
                        TableBuilder::new(ui)
                            .striped(true)
                            .resizable(true)
                            .column(Column::auto().at_least(70.0))
                            .column(Column::auto().at_least(80.0))
                            .column(Column::remainder().at_least(180.0).clip(true))
                            .column(Column::auto().at_least(70.0))
                            .column(Column::auto().at_least(90.0))
                            .header(24.0, |mut h| {
                                for label in ["Line", "Item", "Description", "Qty", "Batch"] {
                                    h.col(|ui| {
                                        ui.strong(label);
                                    });
                                }
                            })
                            .body(|body| {
                                body.rows(28.0, order.order_items.len(), |mut row| {
                                    let item = &order.order_items[row.index()];
                                    row.col(|ui| {
                                        ui.label(format!("#{}", item.id));
                                    });
                                    row.col(|ui| {
                                        ui.label(item.item_id.to_string());
                                    });
                                    row.col(|ui| {
                                        ui.label(item.item_description.as_deref().unwrap_or("-"));
                                    });
                                    row.col(|ui| {
                                        ui.label(item.qty.to_string());
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

                    ui.add_space(8.0);
                    ui.heading("Tracking");
                    if order.tracking_numbers.is_empty() {
                        ui.weak("No tracking numbers.");
                    } else {
                        ui.horizontal_wrapped(|ui| {
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

                    ui.add_space(8.0);
                    ui.heading("Reservations");
                    if order.reservations.is_empty() {
                        ui.weak("No reservations.");
                    } else {
                        TableBuilder::new(ui)
                            .striped(true)
                            .resizable(true)
                            .column(Column::auto().at_least(90.0))
                            .column(Column::auto().at_least(80.0))
                            .column(Column::auto().at_least(90.0))
                            .column(Column::auto().at_least(80.0))
                            .column(Column::remainder().at_least(110.0))
                            .header(24.0, |mut h| {
                                for label in ["Batch", "Location", "Qty", "Status", "Created"] {
                                    h.col(|ui| {
                                        ui.strong(label);
                                    });
                                }
                            })
                            .body(|body| {
                                body.rows(28.0, order.reservations.len(), |mut row| {
                                    let reservation = &order.reservations[row.index()];
                                    row.col(|ui| {
                                        ui.label(reservation.item_batch_id.to_string());
                                    });
                                    row.col(|ui| {
                                        self.location_label_ui(ui, reservation.location_id);
                                    });
                                    row.col(|ui| {
                                        ui.label(reservation.qty.to_string());
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

                    ui.add_space(8.0);
                    ui.heading("Activity");
                    if order.activity.is_empty() {
                        ui.weak("No activity recorded yet.");
                    } else {
                        for activity in &order.activity {
                            ui.group(|ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.strong(Self::short_datetime(activity.created));
                                    ui.label(&activity.action);
                                });
                            });
                        }
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if order.status == OrderStatus::Shipped {
                            ui.weak("Shipped orders cannot be changed here.");
                        } else if order.deleted.is_none() {
                            self.delete_button(
                                ui,
                                false,
                                format!("order {}", order.order_key),
                                "/api/orders/delete",
                                json!({"order_id": order.id}),
                                Screen::Orders,
                                "Order deleted",
                            );
                        } else if ui.button("Restore order").clicked() {
                            self.api.action(
                                "/api/orders/restore",
                                json!({"order_id": order.id}),
                                Screen::Orders,
                                "Order restored",
                            );
                        }
                    });
                });
            });
        if !open {
            self.open_order_ids.remove(&order.id);
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

    // ---- Accounts --------------------------------------------------------
    pub(super) fn accounts_screen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("New account", |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut self.forms.new_name);
                ui.label("Email");
                ui.text_edit_singleline(&mut self.forms.new_email);
                if ui.button("Create").clicked() {
                    self.api.action(
                        "/api/accounts/add",
                        json!({"name": self.forms.new_name, "email": self.forms.new_email}),
                        Screen::Accounts,
                        "Account created",
                    );
                    self.forms.new_name.clear();
                    self.forms.new_email.clear();
                }
            });
        });
        ui.separator();
        let accounts = self.data.accounts.clone();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().at_least(40.0)) // ID
            .column(Column::initial(160.0).at_least(100.0).clip(true)) // Name
            .column(Column::remainder().at_least(160.0).clip(true)) // Email
            .column(Column::auto().at_least(90.0)) // Warehouses
            .column(Column::auto().at_least(90.0)) // Actions
            .header(26.0, |mut h| {
                for label in ["ID", "Name", "Email", "Warehouses", "Actions"] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|mut body| {
                for a in &accounts {
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
                            ui.label(a.account_warehouses.len().to_string());
                        });
                        row.col(|ui| {
                            if a.deleted.is_none() {
                                self.delete_button(
                                    ui,
                                    true,
                                    format!("account {}", a.name),
                                    "/api/accounts/delete",
                                    json!({"account_id": a.id}),
                                    Screen::Accounts,
                                    "Account deleted",
                                );
                            } else if ui.small_button("Restore").clicked() {
                                self.api.action(
                                    "/api/accounts/restore",
                                    json!({"account_id": a.id}),
                                    Screen::Accounts,
                                    "Account restored",
                                );
                            }
                        });
                    });
                }
            });
    }

    // ---- Warehouses (read-only, mirrors the original getWarehouses) ------
    pub(super) fn warehouses_screen(&mut self, ui: &mut egui::Ui) {
        let warehouses = self.data.warehouses.clone();
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
                for w in &warehouses {
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
    pub(super) fn items_screen(&mut self, ui: &mut egui::Ui) {
        ui.heading("Item Master");
        ui.weak("Maintain item definitions, packaging units, SKUs, and item barcodes.");
        ui.add_space(8.0);

        ui.collapsing("New item", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Description");
                ui.text_edit_singleline(&mut self.forms.new_desc);
                ui.label("Packaging");
                Self::select_from_allowed(
                    ui,
                    "new_item_packaging_unit",
                    &mut self.forms.new_packaging_unit,
                    &PACKAGING_UNITS,
                );
                if ui.button("Create").clicked() {
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
                }
            });
        });
        ui.separator();

        let items = self.data.items.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for item in &items {
                egui::Frame::default()
                    .fill(if ui.visuals().dark_mode {
                        egui::Color32::from_black_alpha(38)
                    } else {
                        egui::Color32::from_rgb(250, 251, 253)
                    })
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        if ui.visuals().dark_mode {
                            egui::Color32::from_rgb(68, 82, 104)
                        } else {
                            egui::Color32::from_rgb(214, 221, 232)
                        },
                    ))
                    .rounding(egui::Rounding::same(8.0))
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(format!(
                            "#{} {}",
                            item.id,
                            item.description.clone().unwrap_or_default()
                        ));
                        Self::packaging_unit_badge(ui, &item.packaging_unit);
                        if item.deleted.is_some() {
                            ui.colored_label(egui::Color32::LIGHT_RED, "(deleted)");
                        }
                    });

                    ui.add_space(8.0);
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.heading("SKUs");
                            ui.weak(format!("{} assigned", item.skus.len()));
                        });
                        if item.skus.is_empty() {
                            ui.weak("No SKUs assigned.");
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                for sku in &item.skus {
                                    egui::Frame::default()
                                        .fill(if ui.visuals().dark_mode {
                                            egui::Color32::from_black_alpha(70)
                                        } else {
                                            egui::Color32::from_rgb(241, 245, 250)
                                        })
                                        .stroke(egui::Stroke::new(
                                            1.0_f32,
                                            egui::Color32::from_rgb(120, 145, 180),
                                        ))
                                        .rounding(egui::Rounding::same(5.0))
                                        .inner_margin(egui::Margin::symmetric(7.0, 3.0))
                                        .show(ui, |ui| {
                                            ui.label(egui::RichText::new(&sku.name).strong());
                                        });
                                }
                            });
                        }
                        ui.horizontal_wrapped(|ui| {
                            let sku_key = format!("item:{}:sku", item.id);
                            let mut sku =
                                self.forms.drafts.get(&sku_key).cloned().unwrap_or_default();
                            ui.label("Add SKU");
                            ui.add(
                                egui::TextEdit::singleline(&mut sku)
                                    .desired_width(180.0)
                                    .hint_text("SKU"),
                            );
                            if ui.small_button("Add SKU").clicked() && !sku.trim().is_empty() {
                                self.api.action(
                                    "/api/items/skus/add",
                                    json!({"item_id": item.id, "name": sku}),
                                    Screen::Items,
                                    "SKU added",
                                );
                            }
                            self.forms.drafts.insert(sku_key, sku);
                        });
                    });

                    ui.add_space(8.0);
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.heading("Barcodes");
                            ui.weak(format!("{} assigned", item.barcodes.len()));
                        });
                        if item.barcodes.is_empty() {
                            ui.weak("No barcodes assigned.");
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                for barcode in &item.barcodes {
                                    ui.vertical(|ui| {
                                        Self::barcode_preview(
                                            ui,
                                            &barcode.name,
                                            &barcode.r#type,
                                            true,
                                        );
                                        ui.horizontal(|ui| {
                                            if ui.small_button("Save SVG").clicked() {
                                                self.save_barcode_svg(
                                                    &barcode.name,
                                                    &barcode.r#type,
                                                );
                                            }
                                            self.delete_button(
                                                ui,
                                                true,
                                                format!("barcode {}", barcode.name),
                                                "/api/items/barcodes/delete",
                                                json!({"barcode_id": barcode.id}),
                                                Screen::Items,
                                                "Barcode deleted",
                                            );
                                        });
                                    });
                                }
                            });
                        }
                        ui.separator();
                        let bc_key = format!("item:{}:barcode", item.id);
                        let bct_key = format!("item:{}:barcode_type", item.id);
                        let mut barcode =
                            self.forms.drafts.get(&bc_key).cloned().unwrap_or_default();
                        let mut barcode_type = self
                            .forms
                            .drafts
                            .get(&bct_key)
                            .cloned()
                            .unwrap_or_else(|| "code128".to_owned());
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Add Barcode");
                            ui.add(
                                egui::TextEdit::singleline(&mut barcode)
                                    .desired_width(220.0)
                                    .hint_text("Barcode value"),
                            );
                            ui.label("Type");
                            Self::select_from_allowed(
                                ui,
                                format!("item_{}_barcode_type", item.id),
                                &mut barcode_type,
                                &BARCODE_TYPES,
                            );
                            let barcode_error =
                                Self::barcode_validation_error(&barcode, &barcode_type);
                            let can_use_barcode =
                                !barcode.trim().is_empty() && barcode_error.is_none();
                            if ui
                                .add_enabled(can_use_barcode, egui::Button::new("Add Barcode"))
                                .clicked()
                            {
                                self.api.action(
                                    "/api/items/barcodes/add",
                                    json!({"item_id": item.id, "name": barcode, "type": barcode_type}),
                                    Screen::Items,
                                    "Barcode added",
                                );
                            }
                            if ui
                                .add_enabled(can_use_barcode, egui::Button::new("Save Draft SVG"))
                                .clicked()
                            {
                                self.save_barcode_svg(&barcode, &barcode_type);
                            }
                        });
                        if let Some(err) = Self::barcode_validation_error(&barcode, &barcode_type)
                        {
                            ui.colored_label(
                                egui::Color32::from_rgb(210, 60, 60),
                                format!("Invalid barcode: {err}"),
                            );
                        } else if !barcode.trim().is_empty() {
                            Self::barcode_preview(ui, &barcode, &barcode_type, false);
                        }
                        self.forms.drafts.insert(bc_key, barcode);
                        self.forms.drafts.insert(bct_key, barcode_type);
                    });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if item.deleted.is_none() {
                            self.delete_button(
                                ui,
                                true,
                                format!(
                                    "item {}",
                                    item.description.as_deref().unwrap_or("unnamed")
                                ),
                                "/api/items/delete",
                                json!({"item_id": item.id}),
                                Screen::Items,
                                "Item deleted",
                            );
                        } else if ui.small_button("Restore").clicked() {
                            self.api.action(
                                "/api/items/restore",
                                json!({"item_id": item.id}),
                                Screen::Items,
                                "Item restored",
                            );
                        }
                    });
                    });
                ui.add_space(8.0);
            }
        });
    }

    // ---- Locations -------------------------------------------------------
    pub(super) fn locations_screen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("New location", |ui| {
            let warehouse_options = self.warehouse_options();
            ui.horizontal_wrapped(|ui| {
                ui.label("Warehouse");
                let warehouse_id = Self::entity_picker(
                    ui,
                    "new_location_warehouse",
                    &mut self.forms.new_warehouse_id,
                    &warehouse_options,
                    "Search warehouse",
                );
                ui.label("Name");
                ui.text_edit_singleline(&mut self.forms.new_name);
                ui.label("Type");
                ui.text_edit_singleline(&mut self.forms.new_type);
                if ui.button("Create").clicked() {
                    if let Some(warehouse_id) = warehouse_id {
                        self.api.action(
                            "/api/locations/add",
                            json!({
                                "warehouse_id": warehouse_id,
                                "name": self.forms.new_name,
                                "type": self.forms.new_type,
                            }),
                            Screen::Locations,
                            "Location created",
                        );
                        self.forms.new_name.clear();
                    } else {
                        self.toast("Choose a warehouse", true, self.now);
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
                for label in ["ID", "Warehouse", "Scan Code", "Type", "Actions"] {
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
                                loc.warehouse_name
                                    .clone()
                                    .unwrap_or_else(|| self.warehouse_label(loc.warehouse_id)),
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
