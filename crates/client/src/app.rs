//! The eframe/egui application.
//!
//! Each domain (Orders, Users, …) is an independent panel. The sidebar toggles
//! panels on/off; an open panel is a movable/resizable [`egui::Window`] and can
//! be **popped out into its own native OS window** (egui multi-viewport) and
//! docked back. Multiple panels can be visible at once.

mod components;
mod loads;
mod operations;
mod panels;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::mpsc::{channel, Receiver};

use chrono::{
    Datelike, Duration, Local, LocalResult, NaiveDate, NaiveTime, SecondsFormat, TimeZone, Utc,
};
use egui_extras::{Column, TableBuilder};
use serde_json::{json, Value};
use wareboxes_core::dto::{SessionUser, SummaryCount};
use wareboxes_core::models::{
    Account, AuditWave, Employee, InventoryBalance, Item, ItemBatch, LicensePlate, Load,
    LoadFileCategory, LoadLine, LoadLineStatus, LoadNote, LoadStatus, LoadType, Location, Movement,
    Order, OrderStatus, Permission, Role, User, Warehouse,
};

use crate::api::{ApiClient, ApiEvent, Screen};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080";
/// Open panels re-fetch their data on this cadence.
const AUTO_REFRESH_SECS: f64 = 5.0;
const LARGE_PANEL_AUTO_REFRESH_SECS: f64 = 60.0;
const LOAD_CHUNK_SIZE: usize = 1_000;
const DEFAULT_ORDER_PAGE_SIZE: i64 = 100;
const ORDER_PAGE_SIZES: [i64; 8] = [50, 100, 250, 500, 1000, 2500, 5000, 10000];
const ORDER_STATUS_FILTERS: [(&str, &str); 8] = [
    ("", "All Statuses"),
    ("open", "Open"),
    ("processing", "Partial Pick"),
    ("awaiting shipment", "Awaiting Shipment"),
    ("shipped", "Shipped"),
    ("held", "Held"),
    ("cancelled", "Cancelled"),
    ("void", "Void"),
];
const PACKAGING_UNITS: [(&str, &str); 2] = [("each", "Each"), ("case", "Case")];
const BARCODE_TYPES: [(&str, &str); 4] = [
    ("code128", "Code 128"),
    ("gs1-128", "GS1-128"),
    ("upc-a", "UPC-A"),
    ("qr", "QR Code"),
];

/// Per-panel window state.
#[derive(Debug, Clone, Copy, Default)]
struct PanelState {
    open: bool,
    /// Rendered as its own OS-native window (immediate viewport).
    detached: bool,
}

struct Toast {
    message: String,
    error: bool,
    created: f64,
}

#[derive(Clone)]
struct PendingDelete {
    subject: String,
    path: &'static str,
    body: Value,
    refresh: Screen,
    success_message: &'static str,
}

struct LoadFilters<'a> {
    date: &'a str,
    date_mode: &'a str,
    status: &'a str,
    load_type: &'a str,
    account: &'a str,
    search: &'a str,
}

#[derive(Default)]
struct Forms {
    base_url: String,
    email: String,
    password: String,
    // new order
    o_key: String,
    o_line1: String,
    o_city: String,
    o_state: String,
    o_zip: String,
    o_country: String,
    o_rush: bool,
    // new role / permission / account
    new_name: String,
    new_desc: String,
    new_email: String,
    new_packaging_unit: String,
    new_barcode: String,
    new_type: String,
    new_warehouse_id: String,
    new_account_id: String,
    new_first_name: String,
    new_last_name: String,
    new_title: String,
    /// Per-row text drafts, keyed by `"<entity>:<id>:<field>"`.
    drafts: HashMap<String, String>,
    /// Per-row status selection for orders.
    order_status: HashMap<i64, OrderStatus>,
}

#[derive(Default)]
struct Data {
    users: Vec<User>,
    roles: Vec<Role>,
    permissions: Vec<Permission>,
    accounts: Vec<Account>,
    warehouses: Vec<Warehouse>,
    orders: Vec<Order>,
    items: Vec<Item>,
    locations: Vec<Location>,
    loads: Vec<Load>,
    item_batches: Vec<ItemBatch>,
    inventory_balances: Vec<InventoryBalance>,
    movements: Vec<Movement>,
    license_plates: Vec<LicensePlate>,
    license_plate_lookup: Option<LicensePlate>,
    employees: Vec<Employee>,
    audits: Vec<AuditWave>,
    order_summaries: Vec<SummaryCount>,
}

pub struct WareboxesApp {
    api: ApiClient,
    rx: Receiver<ApiEvent>,
    session: Option<SessionUser>,
    panels: BTreeMap<Screen, PanelState>,
    forms: Forms,
    data: Data,
    toasts: Vec<Toast>,
    /// Wall-clock seconds of the current frame (from egui's input time).
    now: f64,
    /// Last time each screen's data was requested, for auto-refresh.
    last_fetch: HashMap<Screen, f64>,
    open_load_ids: BTreeSet<i64>,
    open_order_ids: BTreeSet<i64>,
    settings_open: bool,
    light_mode: bool,
    pending_delete: Option<PendingDelete>,
    loading_loads: bool,
    order_total: i64,
    order_limit: i64,
    order_offset: i64,
}

impl WareboxesApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (tx, rx) = channel();
        let api = ApiClient::new(DEFAULT_BASE_URL.to_owned(), tx, cc.egui_ctx.clone());
        let forms = Forms {
            base_url: api.base_url.clone(),
            o_country: "US".to_owned(),
            new_packaging_unit: "each".to_owned(),
            new_type: "inbound".to_owned(),
            ..Default::default()
        };
        Self {
            api,
            rx,
            session: None,
            panels: BTreeMap::new(),
            forms,
            data: Data::default(),
            toasts: Vec::new(),
            now: 0.0,
            last_fetch: HashMap::new(),
            open_load_ids: BTreeSet::new(),
            open_order_ids: BTreeSet::new(),
            settings_open: false,
            light_mode: false,
            pending_delete: None,
            loading_loads: false,
            order_total: 0,
            order_limit: DEFAULT_ORDER_PAGE_SIZE,
            order_offset: 0,
        }
    }

    /// Request a screen's data and record when, so auto-refresh and the
    /// manual Refresh button share one cadence.
    fn fetch(&mut self, s: Screen) {
        if s == Screen::Orders {
            self.fetch_orders();
        } else if s == Screen::Loads {
            self.loading_loads = true;
            self.api.get_loads_chunk(0, LOAD_CHUNK_SIZE);
            self.api.get_list(Screen::Accounts);
            self.api.get_list(Screen::Warehouses);
        } else {
            self.api.get_list(s);
        }
        match s {
            Screen::Inventory => {
                self.api.get_inventory_balances();
                self.api.get_movements();
                self.api.get_list(Screen::Items);
                self.api.get_list(Screen::Locations);
            }
            Screen::Locations => {
                self.api.get_list(Screen::Warehouses);
            }
            Screen::LicensePlates => {
                self.api.get_list(Screen::Locations);
            }
            _ => {}
        }
        self.last_fetch.insert(s, self.now);
    }

    fn fetch_orders(&self) {
        let search = self
            .forms
            .drafts
            .get("orders:search")
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty());
        let status = self
            .forms
            .drafts
            .get("orders:status")
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty());
        self.api
            .get_orders_page(self.order_limit, self.order_offset, search, status);
    }

    fn has_perm(&self, name: &str) -> bool {
        self.session.as_ref().is_some_and(|s| {
            s.user
                .user_permissions
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case("admin") || p.name.eq_ignore_ascii_case(name))
        })
    }

    fn visible_screens(&self) -> impl Iterator<Item = Screen> + '_ {
        Screen::ALL
            .into_iter()
            .filter(|s| self.has_perm(s.permission()))
    }

    fn panel(&mut self, s: Screen) -> &mut PanelState {
        self.panels.entry(s).or_default()
    }

    /// Toggle a panel and fetch its data when it becomes visible.
    fn set_panel_open(&mut self, s: Screen, open: bool) {
        self.panel(s).open = open;
        if open {
            self.fetch(s);
        }
    }

    /// Re-fetch every open, visible panel whose data is older than the
    /// refresh interval.
    fn auto_refresh(&mut self) {
        let stale: Vec<Screen> = self
            .visible_screens()
            .filter(|s| self.panels.get(s).is_some_and(|p| p.open))
            .filter(|s| {
                self.now - self.last_fetch.get(s).copied().unwrap_or(f64::MIN)
                    >= Self::auto_refresh_secs(*s)
            })
            .collect();
        for s in stale {
            self.fetch(s);
        }
    }

    fn auto_refresh_secs(screen: Screen) -> f64 {
        match screen {
            Screen::Orders | Screen::Loads => LARGE_PANEL_AUTO_REFRESH_SECS,
            _ => AUTO_REFRESH_SECS,
        }
    }

    fn toast(&mut self, message: impl Into<String>, error: bool, now: f64) {
        self.toasts.push(Toast {
            message: message.into(),
            error,
            created: now,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn delete_button(
        &mut self,
        ui: &mut egui::Ui,
        small: bool,
        subject: impl Into<String>,
        path: &'static str,
        body: Value,
        refresh: Screen,
        success_message: &'static str,
    ) {
        let red = Self::danger_text_color(ui);
        let fill = if ui.visuals().dark_mode {
            egui::Color32::from_rgba_unmultiplied(120, 28, 28, 95)
        } else {
            egui::Color32::from_rgb(255, 236, 235)
        };
        let button = egui::Button::new(egui::RichText::new("🗑").color(red).strong())
            .fill(fill)
            .stroke(egui::Stroke::new(1.0_f32, red))
            .min_size(if small {
                egui::vec2(24.0, 22.0)
            } else {
                egui::vec2(34.0, 26.0)
            });
        let clicked = ui.add(button).on_hover_text("Delete").clicked();
        if clicked {
            self.pending_delete = Some(PendingDelete {
                subject: subject.into(),
                path,
                body,
                refresh,
                success_message,
            });
        }
    }

    fn confirmation_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_delete.clone() else {
            return;
        };

        let mut confirmed = false;
        let mut cancelled = false;
        egui::Window::new("Confirm delete")
            .id(egui::Id::new("confirm_delete_dialog"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.label(format!("Delete {}?", pending.subject));
                ui.weak("This may be blocked by the server if the record is still in use.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancelled = true;
                    }
                    let delete = egui::Button::new(
                        egui::RichText::new("🗑 Delete")
                            .color(Self::danger_text_color(ui))
                            .strong(),
                    );
                    if ui.add(delete).clicked() {
                        confirmed = true;
                    }
                });
            });

        if confirmed {
            self.api.action(
                pending.path,
                pending.body,
                pending.refresh,
                pending.success_message,
            );
            self.pending_delete = None;
        } else if cancelled {
            self.pending_delete = None;
        }
    }

    fn apply_visuals(&self, ctx: &egui::Context) {
        if self.light_mode {
            ctx.set_visuals(egui::Visuals::light());
        } else {
            ctx.set_visuals(egui::Visuals::dark());
        }
    }

    fn drain_events(&mut self, now: f64) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                ApiEvent::LoggedIn(s) => {
                    self.api.token = Some(s.token.clone());
                    self.api.tenant_id = Some(s.active_tenant.tenant_id);
                    self.light_mode = s.settings.light_mode;
                    self.session = Some(*s);
                    let first = if self.has_perm("orders") {
                        Screen::Orders
                    } else {
                        Screen::Users
                    };
                    self.set_panel_open(first, true);
                }
                ApiEvent::LoggedOut => {
                    self.session = None;
                    self.api.token = None;
                    self.api.tenant_id = None;
                    self.panels.clear();
                    self.open_load_ids.clear();
                    self.open_order_ids.clear();
                    self.settings_open = false;
                }
                ApiEvent::Users(u) => self.data.users = u,
                ApiEvent::Roles(r) => self.data.roles = r,
                ApiEvent::Permissions(p) => self.data.permissions = p,
                ApiEvent::Accounts(a) => self.data.accounts = a,
                ApiEvent::Warehouses(w) => self.data.warehouses = w,
                ApiEvent::Orders(page) => {
                    self.order_total = page.page.total;
                    self.order_limit = page.page.limit;
                    self.order_offset = page.page.offset;
                    self.data.order_summaries = page.summaries;
                    self.forms.order_status =
                        page.page.items.iter().map(|x| (x.id, x.status)).collect();
                    let mut orders = page.page.items;
                    for order in &mut orders {
                        if self.open_order_ids.contains(&order.id) {
                            if let Some(existing) = self
                                .data
                                .orders
                                .iter()
                                .find(|existing| existing.id == order.id)
                            {
                                order.reservations = existing.reservations.clone();
                                order.activity = existing.activity.clone();
                            }
                        }
                    }
                    self.data.orders = orders;
                    if self.data.orders.is_empty() && self.order_offset > 0 && self.order_total > 0
                    {
                        self.order_offset =
                            ((self.order_total - 1) / self.order_limit.max(1)) * self.order_limit;
                        self.fetch_orders();
                    }
                }
                ApiEvent::OrderDetail(order) => {
                    if let Some(existing) = self
                        .data
                        .orders
                        .iter_mut()
                        .find(|existing| existing.id == order.id)
                    {
                        *existing = order;
                    } else {
                        self.data.orders.push(order);
                        self.data.orders.sort_by(|a, b| {
                            b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id))
                        });
                    }
                }
                ApiEvent::Items(i) => self.data.items = i,
                ApiEvent::Locations(l) => self.data.locations = l,
                ApiEvent::Loads(mut l) => {
                    l.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
                    self.data.loads = l;
                }
                ApiEvent::LoadChunk {
                    loads,
                    offset,
                    limit,
                } => {
                    if offset == 0 {
                        self.data.loads.clear();
                    }
                    self.data.loads.extend(loads.iter().cloned());
                    self.loading_loads = loads.len() == limit;
                    if self.loading_loads {
                        self.api.get_loads_chunk(offset + limit, limit);
                    }
                }
                ApiEvent::LoadDetail(load) => {
                    if let Some(existing) = self
                        .data
                        .loads
                        .iter_mut()
                        .find(|existing| existing.id == load.id)
                    {
                        *existing = load;
                    } else {
                        self.data.loads.push(load);
                        self.data.loads.sort_by(|a, b| {
                            b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id))
                        });
                    }
                }
                ApiEvent::ItemBatches(b) => self.data.item_batches = b,
                ApiEvent::InventoryBalances(b) => self.data.inventory_balances = b,
                ApiEvent::Movements(m) => self.data.movements = m,
                ApiEvent::LicensePlates(lp) => self.data.license_plates = lp,
                ApiEvent::LicensePlateLookup(lp) => self.data.license_plate_lookup = lp,
                ApiEvent::Employees(e) => self.data.employees = e,
                ApiEvent::Audits(a) => self.data.audits = a,
                ApiEvent::ActionDone(msg, screen) => {
                    self.toast(msg, false, now);
                    self.fetch(screen);
                }
                ApiEvent::SettingsSaved(settings) => {
                    self.light_mode = settings.light_mode;
                    if let Some(session) = &mut self.session {
                        session.settings = settings;
                    }
                    self.toast("Settings saved", false, now);
                }
                ApiEvent::Error(e) => self.toast(e, true, now),
            }
        }
    }
}

impl eframe::App for WareboxesApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        self.now = now;
        self.drain_events(now);
        self.apply_visuals(ctx);
        self.toasts.retain(|t| now - t.created < 5.0);

        if self.session.is_none() {
            self.login_view(ctx);
            self.toast_overlay(ctx);
            return;
        }

        self.auto_refresh();
        // Keep ticking even without user input so auto-refresh fires.
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

        self.nav_bar(ctx);
        self.settings_panel(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.panels.values().all(|p| !p.open) {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.weak("Open a panel from the bar above. Panels are movable windows you can pop out.");
                });
            }
        });

        self.show_panels(ctx);
        self.confirmation_dialog(ctx);
        self.toast_overlay(ctx);
    }
}

impl WareboxesApp {
    fn login_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.heading("Wareboxes WMS");
                ui.add_space(20.0);
                egui::Grid::new("login").num_columns(2).show(ui, |ui| {
                    ui.label("Server URL");
                    ui.text_edit_singleline(&mut self.forms.base_url);
                    ui.end_row();
                    ui.label("Email");
                    ui.text_edit_singleline(&mut self.forms.email);
                    ui.end_row();
                    ui.label("Password");
                    ui.add(egui::TextEdit::singleline(&mut self.forms.password).password(true));
                    ui.end_row();
                });
                ui.add_space(12.0);
                let submit = ui.button("Log in").clicked();
                if submit {
                    self.api.base_url = self.forms.base_url.clone();
                    self.api.login(&self.forms.email, &self.forms.password);
                }
            });
        });
    }

    /// Single top bar: panel toggles on the left, account actions on the
    /// right. Replaces the old separate header + left sidebar.
    fn nav_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("nav_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                for s in self.visible_screens().collect::<Vec<_>>() {
                    let mut open = self.panels.get(&s).is_some_and(|p| p.open);
                    if ui.toggle_value(&mut open, s.title()).changed() {
                        self.set_panel_open(s, open);
                    }
                }

                ui.separator();
                if ui.button("Open all").clicked() {
                    for s in self.visible_screens().collect::<Vec<_>>() {
                        self.set_panel_open(s, true);
                    }
                }
                if ui.button("Close all").clicked() {
                    for s in Screen::ALL {
                        self.panel(s).open = false;
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Log out").clicked() {
                        self.api.logout();
                    }
                    if ui.button("Settings").clicked() {
                        self.settings_open = true;
                    }
                    if let Some(s) = &self.session {
                        ui.weak(&s.user.email);
                    }
                });
            });
            ui.add_space(2.0);
        });
    }

    fn settings_panel(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }

        let mut open = self.settings_open;
        egui::Window::new("Settings")
            .id(egui::Id::new("settings_panel"))
            .open(&mut open)
            .resizable(false)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.heading("Display");
                let mut light_mode = self.light_mode;
                if ui.checkbox(&mut light_mode, "Light mode").changed() {
                    self.light_mode = light_mode;
                    self.apply_visuals(ctx);
                    self.api.save_settings(self.light_mode);
                }
            });
        self.settings_open = open;
    }

    /// Render every open panel: docked ones as [`egui::Window`]s, detached
    /// ones as their own native window via an immediate viewport.
    fn show_panels(&mut self, ctx: &egui::Context) {
        let open_screens: Vec<Screen> = self
            .visible_screens()
            .filter(|s| self.panels.get(s).is_some_and(|p| p.open))
            .collect();

        for s in open_screens {
            let detached = self.panels.get(&s).map(|p| p.detached).unwrap_or(false);
            if detached {
                self.show_detached(ctx, s);
            } else {
                self.show_window(ctx, s);
            }
        }
    }

    fn show_window(&mut self, ctx: &egui::Context, s: Screen) {
        let mut open = true;
        let mut pop_out = false;
        egui::Window::new(s.title())
            .id(egui::Id::new(("panel", s)))
            .open(&mut open)
            .resizable(true)
            .default_size([760.0, 520.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("⏏ Pop out").clicked() {
                        pop_out = true;
                    }
                    if ui.button("⟳ Refresh").clicked() {
                        self.fetch(s);
                    }
                });
                ui.separator();
                self.render_screen(s, ui);
            });

        let st = self.panel(s);
        if pop_out {
            st.detached = true;
        }
        if !open {
            st.open = false;
        }
    }

    fn show_detached(&mut self, ctx: &egui::Context, s: Screen) {
        let mut dock_back = false;
        let mut closed = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of(("panel-vp", s)),
            egui::ViewportBuilder::default()
                .with_title(format!("Wareboxes — {}", s.title()))
                .with_inner_size([900.0, 640.0]),
            |vctx, _class| {
                egui::CentralPanel::default().show(vctx, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("⤵ Dock back").clicked() {
                            dock_back = true;
                        }
                        if ui.button("⟳ Refresh").clicked() {
                            self.fetch(s);
                        }
                    });
                    ui.separator();
                    self.render_screen(s, ui);
                });
                if vctx.input(|i| i.viewport().close_requested()) {
                    closed = true;
                }
            },
        );

        let st = self.panel(s);
        if dock_back {
            st.detached = false;
        }
        if closed {
            st.detached = false;
            st.open = false;
        }
    }

    fn render_screen(&mut self, s: Screen, ui: &mut egui::Ui) {
        match s {
            Screen::Orders => self.orders_screen(ui),
            Screen::Users => self.users_screen(ui),
            Screen::Roles => self.roles_screen(ui),
            Screen::Permissions => self.permissions_screen(ui),
            Screen::Accounts => self.accounts_screen(ui),
            Screen::Warehouses => self.warehouses_screen(ui),
            Screen::Items => self.items_screen(ui),
            Screen::Locations => self.locations_screen(ui),
            Screen::Loads => self.loads_screen(ui),
            Screen::Inventory => self.inventory_screen(ui),
            Screen::LicensePlates => self.license_plates_screen(ui),
            Screen::Employees => self.employees_screen(ui),
            Screen::Audits => self.audits_screen(ui),
        }
    }

    fn toast_overlay(&self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
            .show(ctx, |ui| {
                for t in self.toasts.iter().rev().take(5) {
                    let color = if t.error {
                        egui::Color32::from_rgb(180, 60, 60)
                    } else {
                        egui::Color32::from_rgb(50, 120, 70)
                    };
                    egui::Frame::none()
                        .fill(color)
                        .rounding(6.0)
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.colored_label(egui::Color32::WHITE, &t.message);
                        });
                    ui.add_space(4.0);
                }
            });
    }
}
