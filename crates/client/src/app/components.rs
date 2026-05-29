use super::*;

#[derive(Clone, Default)]
struct PickerState {
    open: bool,
    highlighted: usize,
}

impl WareboxesApp {
    pub(super) fn entity_picker(
        ui: &mut egui::Ui,
        id: impl std::hash::Hash,
        draft: &mut String,
        options: &[(i64, String)],
        hint: &str,
    ) -> Option<i64> {
        ui.push_id(id, |ui| {
            let state_id = ui.id().with("picker_state");
            let mut state = ui
                .data(|data| data.get_temp::<PickerState>(state_id))
                .unwrap_or_default();
            let mut selected = None;
            ui.vertical(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(draft)
                        .desired_width(230.0)
                        .hint_text(hint),
                );

                if response.clicked() || response.gained_focus() || response.changed() {
                    state.open = true;
                }
                if response.changed() {
                    state.highlighted = 0;
                }

                let needle = draft.trim().to_ascii_lowercase();
                let matches = options
                    .iter()
                    .filter(|(option_id, label)| {
                        needle.is_empty()
                            || label.to_ascii_lowercase().contains(&needle)
                            || option_id.to_string().contains(&needle)
                    })
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>();

                let key_target_active = response.has_focus() || response.lost_focus();
                if key_target_active {
                    let arrow_down = ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                    });
                    let arrow_up = ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                    });
                    let enter = ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    });
                    let escape = ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                    });

                    if arrow_down {
                        state.open = true;
                        if !matches.is_empty() {
                            state.highlighted = (state.highlighted + 1).min(matches.len() - 1);
                        }
                    }
                    if arrow_up {
                        state.open = true;
                        if !matches.is_empty() {
                            state.highlighted = state.highlighted.saturating_sub(1);
                        }
                    }
                    if enter && state.open {
                        if let Some((option_id, label)) = matches.get(state.highlighted) {
                            *draft = label.clone();
                            selected = Some(*option_id);
                            state.open = false;
                        }
                    }
                    if escape {
                        state.open = false;
                    }
                }

                if state.highlighted >= matches.len() {
                    state.highlighted = matches.len().saturating_sub(1);
                }

                if state.open {
                    let width = response.rect.width().max(230.0);
                    egui::Frame::popup(ui.style())
                        .inner_margin(egui::Margin::symmetric(4.0, 4.0))
                        .show(ui, |ui| {
                            ui.set_min_width(width);
                            if options.is_empty() {
                                ui.add_sized(
                                    [width, 24.0],
                                    egui::Label::new(
                                        egui::RichText::new("No options loaded").weak(),
                                    ),
                                );
                                return;
                            }

                            for (idx, (option_id, label)) in matches.iter().enumerate() {
                                let highlighted = idx == state.highlighted;
                                let fill = highlighted.then(|| {
                                    if ui.visuals().dark_mode {
                                        egui::Color32::from_rgb(54, 74, 105)
                                    } else {
                                        egui::Color32::from_rgb(225, 236, 251)
                                    }
                                });
                                let mut button = egui::Button::new(label).frame(false).wrap(false);
                                if let Some(fill) = fill {
                                    button = button.fill(fill);
                                }
                                if ui
                                    .add_sized([width, 24.0], button)
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    *draft = label.clone();
                                    selected = Some(*option_id);
                                    state.open = false;
                                }
                            }
                            if matches.is_empty() {
                                ui.add_sized(
                                    [width, 24.0],
                                    egui::Label::new(egui::RichText::new("No matches").weak()),
                                );
                            }
                        });
                }
            });
            ui.data_mut(|data| data.insert_temp(state_id, state));
            selected.or_else(|| Self::selected_entity_id(draft, options))
        })
        .inner
    }

    pub(super) fn selected_entity_id(draft: &str, options: &[(i64, String)]) -> Option<i64> {
        let value = draft.trim();
        if value.is_empty() {
            return None;
        }
        if let Some((id, _)) = options
            .iter()
            .find(|(_, label)| label.eq_ignore_ascii_case(value))
        {
            return Some(*id);
        }
        value.parse::<i64>().ok()
    }

    pub(super) fn account_options(&self) -> Vec<(i64, String)> {
        self.data
            .accounts
            .iter()
            .map(|account| (account.id, account.name.clone()))
            .collect()
    }

    pub(super) fn warehouse_options(&self) -> Vec<(i64, String)> {
        self.data
            .warehouses
            .iter()
            .map(|warehouse| {
                (
                    warehouse.id,
                    warehouse
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Warehouse {}", warehouse.id)),
                )
            })
            .collect()
    }

    pub(super) fn item_options(&self) -> Vec<(i64, String)> {
        self.data
            .items
            .iter()
            .map(|item| {
                (
                    item.id,
                    item.description
                        .clone()
                        .unwrap_or_else(|| format!("Item {}", item.id)),
                )
            })
            .collect()
    }

    pub(super) fn load_options(&self) -> Vec<(i64, String)> {
        self.data
            .loads
            .iter()
            .map(|load| {
                let reference = load
                    .reference_number
                    .as_deref()
                    .or(load.invoice_number.as_deref())
                    .unwrap_or("-");
                (
                    load.id,
                    format!(
                        "{} {} {}",
                        self.load_account_label(load),
                        Self::load_type_label(load.r#type),
                        reference
                    ),
                )
            })
            .collect()
    }

    pub(super) fn location_options(&self) -> Vec<(i64, String)> {
        self.data
            .locations
            .iter()
            .map(|location| {
                let name = location
                    .name
                    .as_deref()
                    .or(location.barcode.as_deref())
                    .unwrap_or("Unnamed location");
                let warehouse = location
                    .warehouse_name
                    .clone()
                    .unwrap_or_else(|| self.warehouse_label(location.warehouse_id));
                (location.id, format!("{name} - {warehouse}"))
            })
            .collect()
    }

    pub(super) fn load_matches_filters(
        &self,
        load: &Load,
        date_filter: &str,
        date_mode: &str,
        status_filter: &str,
        type_filter: &str,
        account_filter: &str,
        search_filter: &str,
    ) -> bool {
        if let Some(selected_date) = Self::parse_filter_date(date_filter) {
            let date_matches = [
                Some(load.created),
                load.expected_time,
                load.appointment_time,
                load.actual_time,
                load.arrival,
                load.departure,
                load.closed,
            ]
            .into_iter()
            .flatten()
            .any(|ts| Self::timestamp_matches_date_filter(ts, selected_date, date_mode));
            if !date_matches {
                return false;
            }
        }

        if !status_filter.trim().is_empty() && load.status.as_str() != status_filter.trim() {
            return false;
        }
        if !type_filter.trim().is_empty() && load.r#type.as_str() != type_filter.trim() {
            return false;
        }

        let account_filter = account_filter.trim().to_ascii_lowercase();
        if !account_filter.is_empty()
            && !load.account_id.to_string().contains(&account_filter)
            && !self
                .load_account_label(load)
                .to_ascii_lowercase()
                .contains(&account_filter)
        {
            return false;
        }

        let search_filter = search_filter.trim().to_ascii_lowercase();
        if !search_filter.is_empty() {
            let haystack = format!(
                "{} {} {} {} {} {}",
                load.id,
                load.reference_number.as_deref().unwrap_or_default(),
                load.invoice_number.as_deref().unwrap_or_default(),
                load.carrier.as_deref().unwrap_or_default(),
                load.trailer_number.as_deref().unwrap_or_default(),
                load.seal_number.as_deref().unwrap_or_default()
            )
            .to_ascii_lowercase();
            if !haystack.contains(&search_filter) {
                return false;
            }
        }

        true
    }

    pub(super) fn parse_filter_date(value: &str) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()
    }

    pub(super) fn timestamp_matches_date_filter(
        ts: wareboxes_core::models::Timestamp,
        selected_date: NaiveDate,
        date_mode: &str,
    ) -> bool {
        let ts_date = ts.with_timezone(&Local).date_naive();
        match date_mode {
            "year" => ts_date.year() == selected_date.year(),
            "month" => {
                ts_date.year() == selected_date.year() && ts_date.month() == selected_date.month()
            }
            _ => ts_date == selected_date,
        }
    }

    pub(super) fn shift_month(date: NaiveDate, months: i32) -> NaiveDate {
        let month_index = date.year() * 12 + date.month0() as i32 + months;
        let year = month_index.div_euclid(12);
        let month = month_index.rem_euclid(12) as u32 + 1;
        NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(date)
    }

    pub(super) fn days_in_month(year: i32, month: u32) -> u32 {
        let first_next_month = if month == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(year, month + 1, 1)
        };
        first_next_month
            .map(|date| (date - Duration::days(1)).day())
            .unwrap_or(30)
    }

    pub(super) fn account_label(&self, id: i64) -> String {
        self.data
            .accounts
            .iter()
            .find(|account| account.id == id)
            .map(|account| account.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    pub(super) fn warehouse_label(&self, id: i64) -> String {
        self.data
            .warehouses
            .iter()
            .find(|warehouse| warehouse.id == id)
            .and_then(|warehouse| warehouse.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    pub(super) fn load_account_label(&self, load: &Load) -> String {
        load.account_name
            .clone()
            .unwrap_or_else(|| self.account_label(load.account_id))
    }

    pub(super) fn load_warehouse_label(&self, load: &Load) -> String {
        load.warehouse_name
            .clone()
            .unwrap_or_else(|| self.warehouse_label(load.warehouse_id))
    }

    pub(super) fn order_account_label(&self, order: &Order) -> String {
        order
            .account_name
            .clone()
            .or_else(|| order.account_id.map(|id| self.account_label(id)))
            .unwrap_or_else(|| "-".to_owned())
    }

    pub(super) fn item_label(&self, id: i64) -> String {
        self.data
            .items
            .iter()
            .find(|item| item.id == id)
            .and_then(|item| {
                item.description
                    .as_ref()
                    .map(|description| (description, item.id))
            })
            .map(|(description, id)| format!("{} ({})", description, id))
            .unwrap_or_else(|| id.to_string())
    }

    pub(super) fn load_status_badge(ui: &mut egui::Ui, status: LoadStatus) {
        ui.label(
            egui::RichText::new(Self::load_status_label(status))
                .strong()
                .color(Self::load_status_color(status)),
        );
    }

    pub(super) fn detail_text(ui: &mut egui::Ui, text: &str, strong: bool, size: f32) {
        let mut text = egui::RichText::new(text)
            .color(Self::load_detail_text_color(ui))
            .size(size);
        if strong {
            text = text.strong();
        }
        ui.label(text);
    }

    pub(super) fn qty_chip(ui: &mut egui::Ui, label: &str, qty: i64, color: egui::Color32) {
        let dark_mode = ui.visuals().dark_mode;
        let fill = if dark_mode {
            egui::Color32::from_black_alpha(145)
        } else {
            egui::Color32::from_rgb(252, 253, 255)
        };
        let text_color = Self::load_detail_text_color(ui);
        egui::Frame::default()
            .fill(fill)
            .stroke(egui::Stroke::new(1.0_f32, color))
            .rounding(egui::Rounding::same(5.0))
            .inner_margin(egui::Margin::symmetric(6.0, 3.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(label)
                            .small()
                            .size(13.0)
                            .color(text_color),
                    );
                    ui.label(
                        egui::RichText::new(qty.to_string())
                            .strong()
                            .size(14.0)
                            .color(text_color),
                    );
                });
            });
    }

    pub(super) fn select_from_allowed(
        ui: &mut egui::Ui,
        id: impl std::hash::Hash,
        value: &mut String,
        options: &[(&str, &str)],
    ) {
        if !options.iter().any(|(allowed, _)| *allowed == value) {
            *value = options[0].0.to_owned();
        }
        let selected = options
            .iter()
            .find_map(|(allowed, label)| (*allowed == value).then_some(*label))
            .unwrap_or(options[0].1);
        egui::ComboBox::from_id_source(id)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for (allowed, label) in options {
                    ui.selectable_value(value, (*allowed).to_owned(), *label);
                }
            });
    }

    pub(super) fn packaging_unit_label(value: &str) -> &str {
        PACKAGING_UNITS
            .iter()
            .find_map(|(allowed, label)| (*allowed == value).then_some(*label))
            .unwrap_or(value)
    }

    pub(super) fn packaging_unit_badge(ui: &mut egui::Ui, value: &str) {
        let (fill, stroke, text) = match value {
            "case" => (
                egui::Color32::from_rgb(255, 239, 198),
                egui::Color32::from_rgb(210, 132, 25),
                egui::Color32::from_rgb(120, 76, 18),
            ),
            _ => (
                egui::Color32::from_rgb(220, 242, 255),
                egui::Color32::from_rgb(42, 132, 190),
                egui::Color32::from_rgb(20, 82, 125),
            ),
        };
        egui::Frame::default()
            .fill(fill)
            .stroke(egui::Stroke::new(1.0_f32, stroke))
            .rounding(egui::Rounding::same(999.0))
            .inner_margin(egui::Margin::symmetric(8.0, 3.0))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(Self::packaging_unit_label(value))
                        .strong()
                        .color(text),
                );
            });
    }

    pub(super) fn barcode_type_label(value: &str) -> &str {
        BARCODE_TYPES
            .iter()
            .find_map(|(allowed, label)| (*allowed == value).then_some(*label))
            .unwrap_or(value)
    }

    pub(super) fn barcode_validation_error(value: &str, barcode_type: &str) -> Option<String> {
        if value.trim().is_empty() {
            return None;
        }
        let result = if barcode_type == "qr" {
            wareboxes_barcodes::encode_qr(value).map(|_| ())
        } else {
            wareboxes_barcodes::encode(barcode_type, value).map(|_| ())
        };
        result.err().map(|err| err.to_string())
    }

    pub(super) fn barcode_preview(
        ui: &mut egui::Ui,
        value: &str,
        barcode_type: &str,
        compact: bool,
    ) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        let fill = if ui.visuals().dark_mode {
            egui::Color32::from_black_alpha(85)
        } else {
            egui::Color32::from_rgb(248, 250, 253)
        };
        let stroke = if ui.visuals().dark_mode {
            egui::Color32::from_rgb(105, 130, 165)
        } else {
            egui::Color32::from_rgb(185, 197, 216)
        };
        egui::Frame::default()
            .fill(fill)
            .stroke(egui::Stroke::new(1.0_f32, stroke))
            .rounding(egui::Rounding::same(6.0))
            .inner_margin(egui::Margin::same(6.0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} ({})",
                            value,
                            Self::barcode_type_label(barcode_type)
                        ))
                        .small()
                        .color(Self::load_detail_text_color(ui)),
                    );
                    let size = if barcode_type == "qr" {
                        let side = if compact { 132.0 } else { 184.0 };
                        egui::vec2(side, side)
                    } else if compact {
                        egui::vec2(132.0, 34.0)
                    } else {
                        egui::vec2(184.0, 54.0)
                    };
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    let painter = ui.painter().with_clip_rect(rect);
                    painter.rect_filled(rect, egui::Rounding::same(3.0), egui::Color32::WHITE);
                    if barcode_type == "qr" {
                        if let Ok(encoded) = wareboxes_barcodes::encode_qr(value) {
                            Self::paint_qr_matrix(&painter, rect.shrink(4.0), &encoded)
                        }
                    } else if let Ok(encoded) = wareboxes_barcodes::encode(barcode_type, value) {
                        Self::paint_encoded_barcode(&painter, rect.shrink(4.0), &encoded);
                    }
                });
            });
    }

    pub(super) fn paint_encoded_barcode(
        painter: &egui::Painter,
        rect: egui::Rect,
        encoded: &wareboxes_barcodes::EncodedBarcode,
    ) {
        let ink = egui::Color32::BLACK;
        let total = encoded.total_modules().max(1) as f32;
        let module_width = rect.width() / total;
        let mut x = rect.left();
        for module in &encoded.modules {
            let width = module.width as f32 * module_width;
            if module.black {
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x, rect.top()),
                        egui::pos2((x + width).min(rect.right()), rect.bottom()),
                    ),
                    egui::Rounding::ZERO,
                    ink,
                );
            }
            x += width;
        }
    }

    pub(super) fn paint_qr_matrix(
        painter: &egui::Painter,
        rect: egui::Rect,
        encoded: &wareboxes_barcodes::EncodedQr,
    ) {
        let side = rect.width().min(rect.height());
        let quiet_zone = 4.0_f32;
        let module = side / (encoded.size as f32 + quiet_zone * 2.0);
        let origin = egui::pos2(
            rect.center().x - side / 2.0 + quiet_zone * module,
            rect.center().y - side / 2.0 + quiet_zone * module,
        );
        for y in 0..encoded.size {
            for x in 0..encoded.size {
                if encoded.module(x, y) {
                    let min =
                        egui::pos2(origin.x + x as f32 * module, origin.y + y as f32 * module);
                    painter.rect_filled(
                        egui::Rect::from_min_size(min, egui::vec2(module, module)),
                        egui::Rounding::ZERO,
                        egui::Color32::BLACK,
                    );
                }
            }
        }
    }

    pub(super) fn save_barcode_svg(&mut self, value: &str, barcode_type: &str) {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (value, barcode_type);
            self.toast(
                "Saving barcode SVGs is only available in the desktop app",
                true,
                self.now,
            );
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let filename = format!(
                "{}-{}.svg",
                Self::sanitize_filename(value),
                Self::sanitize_filename(barcode_type)
            );
            let dir = std::path::Path::new("barcode_exports");
            let path = dir.join(filename);
            let result = std::fs::create_dir_all(dir)
                .and_then(|_| {
                    wareboxes_barcodes::svg(barcode_type, value)
                        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))
                })
                .and_then(|svg| std::fs::write(&path, svg));
            match result {
                Ok(()) => self.toast(format!("Saved {}", path.display()), false, self.now),
                Err(err) => {
                    self.toast(format!("Failed to save barcode SVG: {err}"), true, self.now)
                }
            }
        }
    }

    pub(super) fn sanitize_filename(value: &str) -> String {
        let sanitized = value
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                    ch
                } else {
                    '_'
                }
            })
            .collect::<String>();
        if sanitized.trim_matches('_').is_empty() {
            "barcode".to_owned()
        } else {
            sanitized
        }
    }

    pub(super) fn tracking_chip(ui: &mut egui::Ui, label: &str, value: &str) {
        let dark_mode = ui.visuals().dark_mode;
        let fill = if dark_mode {
            egui::Color32::from_black_alpha(145)
        } else {
            egui::Color32::from_rgb(252, 253, 255)
        };
        let text_color = Self::load_detail_text_color(ui);
        egui::Frame::default()
            .fill(fill)
            .stroke(egui::Stroke::new(
                1.0_f32,
                Self::load_status_color(LoadStatus::Scheduled),
            ))
            .rounding(egui::Rounding::same(5.0))
            .inner_margin(egui::Margin::symmetric(6.0, 3.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(label)
                            .small()
                            .size(13.0)
                            .color(text_color),
                    );
                    ui.label(
                        egui::RichText::new(value)
                            .strong()
                            .size(14.0)
                            .color(text_color),
                    );
                });
            });
    }

    pub(super) fn load_detail_text_color(ui: &egui::Ui) -> egui::Color32 {
        if ui.visuals().dark_mode {
            egui::Color32::from_rgb(246, 248, 252)
        } else {
            egui::Color32::from_rgb(28, 34, 45)
        }
    }

    pub(super) fn outlined_load_text_colors(ui: &egui::Ui) -> (egui::Color32, egui::Color32) {
        if ui.visuals().dark_mode {
            (
                egui::Color32::from_rgb(252, 253, 255),
                egui::Color32::from_black_alpha(225),
            )
        } else {
            (
                egui::Color32::from_rgb(22, 28, 38),
                egui::Color32::from_white_alpha(235),
            )
        }
    }

    pub(super) fn success_text_color(ui: &egui::Ui) -> egui::Color32 {
        if ui.visuals().dark_mode {
            egui::Color32::LIGHT_GREEN
        } else {
            egui::Color32::from_rgb(22, 124, 70)
        }
    }

    pub(super) fn danger_text_color(ui: &egui::Ui) -> egui::Color32 {
        if ui.visuals().dark_mode {
            egui::Color32::LIGHT_RED
        } else {
            egui::Color32::from_rgb(182, 42, 37)
        }
    }

    pub(super) fn load_status_label(status: LoadStatus) -> &'static str {
        match status {
            LoadStatus::Planned => "Planned",
            LoadStatus::Scheduled => "Scheduled",
            LoadStatus::Arrived => "Arrived",
            LoadStatus::Receiving => "Receiving",
            LoadStatus::Received => "Received",
            LoadStatus::Rejected => "Rejected",
            LoadStatus::Closed => "Closed",
            LoadStatus::Cancelled => "Cancelled",
        }
    }

    pub(super) fn can_arrive_load(load: &Load) -> bool {
        load.r#type == LoadType::Inbound
            && matches!(load.status, LoadStatus::Planned | LoadStatus::Scheduled)
            && load.deleted.is_none()
    }

    pub(super) fn load_card_milestone(load: &Load) -> Option<&'static str> {
        match load.status {
            LoadStatus::Arrived | LoadStatus::Receiving => Some("Arrived"),
            LoadStatus::Received => Some("Fully Received"),
            LoadStatus::Rejected => Some("Rejected"),
            LoadStatus::Closed => Some("Closed Out"),
            _ if load.receive_completed => Some("Fully Received"),
            _ => None,
        }
    }

    pub(super) fn load_type_label(load_type: LoadType) -> &'static str {
        match load_type {
            LoadType::Inbound => "Inbound",
            LoadType::Outbound => "Outbound",
        }
    }

    pub(super) fn load_line_status_label(status: LoadLineStatus) -> &'static str {
        match status {
            LoadLineStatus::Pending => "Pending",
            LoadLineStatus::Partial => "Partial",
            LoadLineStatus::Received => "Received",
            LoadLineStatus::Rejected => "Rejected",
            LoadLineStatus::Missing => "Missing",
        }
    }

    pub(super) fn order_status_label(status: OrderStatus) -> &'static str {
        match status {
            OrderStatus::AwaitingShipment => "Awaiting Shipment",
            OrderStatus::Shipped => "Shipped",
            OrderStatus::Cancelled => "Cancelled",
            OrderStatus::Held => "Held",
            OrderStatus::Processing => "Partial Pick",
            OrderStatus::Open => "Open",
            OrderStatus::Void => "Void",
        }
    }

    pub(super) fn title_case(value: &str) -> String {
        value
            .split_whitespace()
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => {
                        let mut out = first.to_uppercase().to_string();
                        out.push_str(chars.as_str());
                        out
                    }
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(super) fn order_visual_state_label(order: &Order) -> &'static str {
        if order.out_of_stock
            && matches!(
                order.status,
                OrderStatus::Open | OrderStatus::AwaitingShipment
            )
        {
            "Out Of Stock"
        } else {
            Self::order_status_label(order.status)
        }
    }

    pub(super) fn order_state_color(ui: &egui::Ui, order: &Order) -> egui::Color32 {
        if order.out_of_stock
            && matches!(
                order.status,
                OrderStatus::Open | OrderStatus::AwaitingShipment
            )
        {
            return if ui.visuals().dark_mode {
                egui::Color32::from_rgb(255, 120, 210)
            } else {
                egui::Color32::from_rgb(170, 42, 128)
            };
        }
        Self::order_status_color(ui, order.status)
    }

    pub(super) fn order_status_color(ui: &egui::Ui, status: OrderStatus) -> egui::Color32 {
        match status {
            OrderStatus::Processing => egui::Color32::from_rgb(236, 145, 35),
            OrderStatus::Shipped => {
                if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(80, 220, 135)
                } else {
                    egui::Color32::from_rgb(20, 128, 72)
                }
            }
            OrderStatus::Cancelled => {
                if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(255, 100, 95)
                } else {
                    egui::Color32::from_rgb(190, 45, 42)
                }
            }
            OrderStatus::Held => {
                if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(175, 145, 255)
                } else {
                    egui::Color32::from_rgb(95, 72, 185)
                }
            }
            OrderStatus::AwaitingShipment => {
                if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(80, 205, 225)
                } else {
                    egui::Color32::from_rgb(20, 125, 150)
                }
            }
            OrderStatus::Open => {
                if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(210, 218, 230)
                } else {
                    egui::Color32::from_rgb(68, 78, 95)
                }
            }
            OrderStatus::Void => egui::Color32::from_rgb(125, 130, 140),
        }
    }

    pub(super) fn order_summary_color(ui: &egui::Ui, key: &str) -> egui::Color32 {
        match key {
            "out_of_stock" => {
                if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(255, 120, 210)
                } else {
                    egui::Color32::from_rgb(170, 42, 128)
                }
            }
            "processing" => Self::order_status_color(ui, OrderStatus::Processing),
            "held" => Self::order_status_color(ui, OrderStatus::Held),
            "awaiting shipment" => Self::order_status_color(ui, OrderStatus::AwaitingShipment),
            "open" => Self::order_status_color(ui, OrderStatus::Open),
            "cancelled" => Self::order_status_color(ui, OrderStatus::Cancelled),
            "void" => Self::order_status_color(ui, OrderStatus::Void),
            _ => Self::load_detail_text_color(ui),
        }
    }

    pub(super) fn summary_badge(ui: &mut egui::Ui, label: &str, count: i64, color: egui::Color32) {
        let fill = if ui.visuals().dark_mode {
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 44)
        } else {
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 28)
        };
        egui::Frame::default()
            .fill(fill)
            .stroke(egui::Stroke::new(1.0_f32, color))
            .rounding(egui::Rounding::same(7.0))
            .inner_margin(egui::Margin::symmetric(8.0, 4.0))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(format!("{label}: {count}"))
                        .strong()
                        .color(color),
                );
            });
    }

    pub(super) fn order_state_badge(ui: &mut egui::Ui, order: &Order) {
        let color = Self::order_state_color(ui, order);
        let fill = if ui.visuals().dark_mode {
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 38)
        } else {
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 26)
        };
        egui::Frame::default()
            .fill(fill)
            .stroke(egui::Stroke::new(1.0_f32, color))
            .rounding(egui::Rounding::same(5.0))
            .inner_margin(egui::Margin::symmetric(6.0, 2.0))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(Self::order_visual_state_label(order))
                        .strong()
                        .color(color),
                );
            });
    }

    pub(super) fn short_line_status_label(status: LoadLineStatus) -> &'static str {
        match status {
            LoadLineStatus::Pending => "Pend",
            LoadLineStatus::Partial => "Part",
            LoadLineStatus::Received => "Recv",
            LoadLineStatus::Rejected => "Rej",
            LoadLineStatus::Missing => "Miss",
        }
    }

    pub(super) fn line_status_counts(load: &Load) -> String {
        LoadLineStatus::ALL
            .iter()
            .filter_map(|status| {
                let count = load
                    .lines
                    .iter()
                    .filter(|line| line.status == *status)
                    .count();
                (count > 0).then(|| format!("{}{}", Self::short_line_status_label(*status), count))
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(super) fn fit_card_text(text: &str, max_width: f32, font_size: f32) -> String {
        let max_chars = (max_width / (font_size * 0.58)).floor().max(1.0) as usize;
        let char_count = text.chars().count();
        if char_count <= max_chars {
            return text.to_owned();
        }
        if max_chars <= 3 {
            return text.chars().take(max_chars).collect();
        }
        let mut trimmed = text.chars().take(max_chars - 3).collect::<String>();
        trimmed.push_str("...");
        trimmed
    }

    pub(super) fn paint_outlined_text(
        painter: &egui::Painter,
        pos: egui::Pos2,
        align: egui::Align2,
        text: &str,
        font: egui::FontId,
        color: egui::Color32,
        outline: egui::Color32,
    ) {
        for offset in [
            egui::vec2(-1.0, 0.0),
            egui::vec2(1.0, 0.0),
            egui::vec2(0.0, -1.0),
            egui::vec2(0.0, 1.0),
        ] {
            painter.text(pos + offset, align, text, font.clone(), outline);
        }
        painter.text(pos, align, text, font, color);
    }

    pub(super) fn load_status_color(status: LoadStatus) -> egui::Color32 {
        match status {
            LoadStatus::Planned => egui::Color32::from_rgb(124, 136, 160),
            LoadStatus::Scheduled => egui::Color32::from_rgb(58, 145, 240),
            LoadStatus::Arrived => egui::Color32::from_rgb(45, 184, 104),
            LoadStatus::Receiving => egui::Color32::from_rgb(226, 166, 32),
            LoadStatus::Received => egui::Color32::from_rgb(20, 150, 152),
            LoadStatus::Rejected => egui::Color32::from_rgb(220, 72, 63),
            LoadStatus::Closed => egui::Color32::from_rgb(104, 87, 214),
            LoadStatus::Cancelled => egui::Color32::from_rgb(82, 82, 92),
        }
    }

    pub(super) fn load_line_color(status: LoadLineStatus) -> egui::Color32 {
        match status {
            LoadLineStatus::Pending => egui::Color32::from_rgb(170, 180, 195),
            LoadLineStatus::Partial => egui::Color32::from_rgb(245, 205, 85),
            LoadLineStatus::Received => egui::Color32::from_rgb(75, 220, 135),
            LoadLineStatus::Rejected => egui::Color32::from_rgb(255, 100, 95),
            LoadLineStatus::Missing => egui::Color32::from_rgb(255, 165, 75),
        }
    }

    pub(super) fn short_datetime(ts: wareboxes_core::models::Timestamp) -> String {
        ts.with_timezone(&Local)
            .format("%Y-%m-%d %H:%M %Z")
            .to_string()
    }

    pub(super) fn optional_datetime(ts: Option<wareboxes_core::models::Timestamp>) -> String {
        ts.map(Self::short_datetime)
            .unwrap_or_else(|| "-".to_owned())
    }

    pub(super) fn arrival_draft_default(load: &Load) -> (String, String) {
        let arrival = load.arrival.unwrap_or_else(Utc::now).with_timezone(&Local);
        (
            arrival.format("%Y-%m-%d").to_string(),
            arrival.format("%H:%M").to_string(),
        )
    }

    pub(super) fn arrival_rfc3339(date: &str, time: &str) -> Option<String> {
        let date = Self::parse_filter_date(date)?;
        let time = NaiveTime::parse_from_str(time.trim(), "%H:%M")
            .or_else(|_| NaiveTime::parse_from_str(time.trim(), "%H:%M:%S"))
            .ok()?;
        let arrival = match Local.from_local_datetime(&date.and_time(time)) {
            LocalResult::Single(arrival) => arrival.with_timezone(&Utc),
            LocalResult::Ambiguous(earliest, _) => earliest.with_timezone(&Utc),
            LocalResult::None => return None,
        };
        if arrival > Utc::now() {
            return None;
        }
        Some(arrival.to_rfc3339_opts(SecondsFormat::Secs, true))
    }

    pub(super) fn load_progress(load: &Load) -> (i64, i64) {
        let expected = load.lines.iter().map(|line| line.expected_qty).sum::<i64>();
        let resolved = load
            .lines
            .iter()
            .map(|line| line.received_qty + line.rejected_qty + line.missing_qty)
            .sum::<i64>();
        (resolved, expected)
    }
}
