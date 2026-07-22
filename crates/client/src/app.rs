//! The eframe/egui application.
//!
//! Each domain (Orders, Users, …) is an independent movable panel. Named
//! workspaces retain separate sets of open panels and window arrangements, and
//! panels can also be popped out into native OS windows (egui multi-viewport).

mod components;
mod loads;
mod operations;
mod panels;
mod theme;
mod workspaces;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::mpsc::{channel, Receiver};

use chrono::{
    Datelike, Duration, Local, LocalResult, NaiveDate, NaiveTime, SecondsFormat, TimeZone, Utc,
};
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wareboxes_core::dto::{SessionUser, SummaryCount};
use wareboxes_core::models::{
    AuditWave, Employee, Facility, InventoryBalance, InventoryOwner, InventoryTransaction, Item,
    ItemBatch, LicensePlate, Load, LoadFileCategory, LoadLine, LoadLineStatus, LoadNote,
    LoadStatus, LoadType, Location, Order, OrderStatus, Permission, Role, User,
};

use crate::api::{ApiClient, ApiEvent, Screen};

const LOCAL_BASE_URL: &str = "http://127.0.0.1:8080";
const WORKSPACE_STORAGE_KEY: &str = "wareboxes_panel_workspaces_v1";
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
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct PanelState {
    open: bool,
    /// Rendered as its own OS-native window (immediate viewport).
    detached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PanelWorkspace {
    id: u64,
    name: String,
    #[serde(default)]
    layout_generation: u64,
    panels: BTreeMap<Screen, PanelState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedWorkspaces {
    workspaces: Vec<PanelWorkspace>,
    active_workspace_id: u64,
    next_workspace_id: u64,
}

impl PanelWorkspace {
    fn new(id: u64, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            layout_generation: 0,
            panels: BTreeMap::new(),
        }
    }
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
    inventory_owner: &'a str,
    search: &'a str,
}

#[derive(Default)]
struct Forms {
    base_url: String,
    email: String,
    password: String,
    // new order
    o_key: String,
    o_inventory_owner_id: String,
    o_line1: String,
    o_city: String,
    o_state: String,
    o_zip: String,
    o_country: String,
    o_rush: bool,
    // new role / permission / inventory owner
    new_name: String,
    new_desc: String,
    new_email: String,
    new_packaging_unit: String,
    new_barcode: String,
    new_type: String,
    new_facility_id: String,
    new_inventory_owner_id: String,
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
    inventory_owners: Vec<InventoryOwner>,
    facilities: Vec<Facility>,
    orders: Vec<Order>,
    items: Vec<Item>,
    locations: Vec<Location>,
    loads: Vec<Load>,
    item_batches: Vec<ItemBatch>,
    inventory_balances: Vec<InventoryBalance>,
    inventory_transactions: Vec<InventoryTransaction>,
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
    workspaces: Vec<PanelWorkspace>,
    active_workspace_id: u64,
    next_workspace_id: u64,
    workspace_editor_open: bool,
    workspace_name_draft: String,
    pending_workspace_delete: Option<u64>,
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
        let api = ApiClient::new(default_base_url(), tx, cc.egui_ctx.clone());
        let mut forms = Forms {
            base_url: api.base_url.clone(),
            o_country: "US".to_owned(),
            new_packaging_unit: "each".to_owned(),
            new_type: "inbound".to_owned(),
            ..Default::default()
        };
        prefill_demo_login(&mut forms);
        let saved = cc.storage.and_then(|storage| {
            eframe::get_value::<SavedWorkspaces>(storage, WORKSPACE_STORAGE_KEY)
        });
        let (mut workspaces, requested_active_id, requested_next_id) = saved
            .map(|saved| {
                (
                    saved.workspaces,
                    saved.active_workspace_id,
                    saved.next_workspace_id,
                )
            })
            .unwrap_or_else(|| (vec![PanelWorkspace::new(1, "Operations")], 1, 2));
        let mut workspace_ids = BTreeSet::new();
        workspaces.retain(|workspace| workspace.id > 0 && workspace_ids.insert(workspace.id));
        if workspaces.is_empty() {
            workspaces.push(PanelWorkspace::new(1, "Operations"));
        }
        for workspace in &mut workspaces {
            if workspace.name.trim().is_empty() {
                workspace.name = format!("Workspace {}", workspace.id);
            }
            for panel in workspace.panels.values_mut() {
                panel.detached = false;
            }
        }
        let active_workspace_id = workspaces
            .iter()
            .find(|workspace| workspace.id == requested_active_id)
            .map(|workspace| workspace.id)
            .unwrap_or(workspaces[0].id);
        let next_workspace_id = requested_next_id.max(
            workspaces
                .iter()
                .map(|workspace| workspace.id)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
        Self {
            api,
            rx,
            session: None,
            workspaces,
            active_workspace_id,
            next_workspace_id,
            workspace_editor_open: false,
            workspace_name_draft: String::new(),
            pending_workspace_delete: None,
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
            self.api.get_list(Screen::InventoryOwners);
        } else if s == Screen::Loads {
            self.loading_loads = true;
            self.api.get_loads_chunk(0, LOAD_CHUNK_SIZE);
            self.api.get_list(Screen::InventoryOwners);
            self.api.get_list(Screen::Facilities);
        } else {
            self.api.get_list(s);
        }
        match s {
            Screen::Inventory => {
                self.api.get_inventory_balances();
                self.api.get_inventory_transactions();
                self.api.get_list(Screen::InventoryOwners);
                self.api.get_list(Screen::Items);
                self.api.get_list(Screen::Locations);
            }
            Screen::Locations => {
                self.api.get_list(Screen::Facilities);
            }
            Screen::LicensePlates => {
                self.api.get_list(Screen::Locations);
                self.api.get_list(Screen::InventoryOwners);
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

    /// Re-fetch every open, visible panel whose data is older than the
    /// refresh interval.
    fn auto_refresh(&mut self) {
        let stale: Vec<Screen> = self
            .visible_screens()
            .filter(|s| {
                self.active_workspace()
                    .panels
                    .get(s)
                    .is_some_and(|panel| panel.open)
            })
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
        ctx.set_style(Self::theme_style(self.light_mode));
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
                    for workspace in &mut self.workspaces {
                        for panel in workspace.panels.values_mut() {
                            panel.detached = false;
                        }
                    }
                    self.workspace_editor_open = false;
                    self.pending_workspace_delete = None;
                    self.open_load_ids.clear();
                    self.open_order_ids.clear();
                    self.settings_open = false;
                }
                ApiEvent::Users(u) => self.data.users = u,
                ApiEvent::Roles(r) => self.data.roles = r,
                ApiEvent::Permissions(p) => self.data.permissions = p,
                ApiEvent::InventoryOwners(a) => self.data.inventory_owners = a,
                ApiEvent::Facilities(w) => self.data.facilities = w,
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
                ApiEvent::InventoryTransactions(transactions) => {
                    self.data.inventory_transactions = transactions;
                }
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

#[cfg(not(target_arch = "wasm32"))]
fn default_base_url() -> String {
    LOCAL_BASE_URL.to_owned()
}

#[cfg(target_arch = "wasm32")]
fn default_base_url() -> String {
    option_env!("WAREBOXES_API_URL")
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            web_sys::window()
                .and_then(|window| window.location().origin().ok())
                .filter(|origin| !origin.is_empty() && origin != "null")
        })
        .unwrap_or_else(|| LOCAL_BASE_URL.to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
fn prefill_demo_login(_forms: &mut Forms) {}

#[cfg(target_arch = "wasm32")]
fn prefill_demo_login(forms: &mut Forms) {
    if let Some(email) = option_env!("WAREBOXES_DEMO_EMAIL") {
        forms.email = email.to_owned();
    }
    if let Some(password) = option_env!("WAREBOXES_DEMO_PASSWORD") {
        forms.password = password.to_owned();
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

        egui::CentralPanel::default().show(ctx, |_ui| {});
        self.show_panels(ctx);
        self.workspace_editor(ctx);
        self.workspace_delete_confirmation(ctx);
        self.confirmation_dialog(ctx);
        self.toast_overlay(ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(
            storage,
            WORKSPACE_STORAGE_KEY,
            &SavedWorkspaces {
                workspaces: self.workspaces.clone(),
                active_workspace_id: self.active_workspace_id,
                next_workspace_id: self.next_workspace_id,
            },
        );
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(5)
    }
}

impl WareboxesApp {
    fn login_view(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(egui::Margin::same(16.0)))
            .show(ctx, |ui| {
                let width = ui.available_width().min(380.0);
                #[cfg(target_arch = "wasm32")]
                let height: f32 = 286.0;
                #[cfg(not(target_arch = "wasm32"))]
                let height: f32 = 354.0;
                let size = egui::vec2(width, height.min(ui.available_height()));
                let rect = egui::Rect::from_center_size(ui.max_rect().center(), size);

                ui.allocate_ui_at_rect(rect, |ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("WAREBOXES")
                                .size(28.0)
                                .strong()
                                .color(Self::accent_color(ui)),
                        );
                        ui.label(egui::RichText::new("Warehouse operations").size(15.0));
                        ui.add_space(26.0);

                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            ui.strong("Server URL");
                            ui.add_sized(
                                [width, 36.0],
                                egui::TextEdit::singleline(&mut self.forms.base_url),
                            );
                            ui.add_space(8.0);
                        }

                        ui.strong("Email");
                        ui.add_sized(
                            [width, 36.0],
                            egui::TextEdit::singleline(&mut self.forms.email),
                        );
                        ui.add_space(8.0);
                        ui.strong("Password");
                        let password = ui.add_sized(
                            [width, 36.0],
                            egui::TextEdit::singleline(&mut self.forms.password).password(true),
                        );
                        ui.add_space(14.0);

                        let can_submit =
                            !self.forms.email.trim().is_empty() && !self.forms.password.is_empty();
                        let button = egui::Button::new(
                            egui::RichText::new("Log in")
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(Self::accent_color(ui))
                        .min_size(egui::vec2(width, 38.0));
                        let clicked = ui.add_enabled(can_submit, button).clicked();
                        let enter = password.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if can_submit && (clicked || enter) {
                            self.api.base_url = self.forms.base_url.clone();
                            self.api.login(&self.forms.email, &self.forms.password);
                        }
                    });
                });
            });
    }

    fn accent_color(ui: &egui::Ui) -> egui::Color32 {
        if ui.visuals().dark_mode {
            egui::Color32::from_rgb(49, 181, 143)
        } else {
            egui::Color32::from_rgb(8, 122, 99)
        }
    }

    fn nav_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("app_shell")
            .frame(
                egui::Frame::side_top_panel(ctx.style().as_ref())
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("WAREBOXES")
                            .size(17.0)
                            .strong()
                            .color(Self::accent_color(ui)),
                    );
                    ui.separator();
                    if let Some(session) = &self.session {
                        ui.strong(&session.active_tenant.name);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Log out").clicked() {
                            self.api.logout();
                        }
                        if ui
                            .add_sized([32.0, 32.0], egui::Button::new("⚙"))
                            .on_hover_text("Settings")
                            .clicked()
                        {
                            self.settings_open = true;
                        }
                        if let Some(session) = &self.session {
                            let tenant_is_identity = session
                                .active_tenant
                                .name
                                .eq_ignore_ascii_case(&session.user.email);
                            if ui.available_width() > 260.0 && !tenant_is_identity {
                                ui.weak(&session.user.email);
                            }
                        }
                    });
                });

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                let workspace_tabs = self
                    .workspaces
                    .iter()
                    .map(|workspace| (workspace.id, workspace.name.clone()))
                    .collect::<Vec<_>>();
                let mut selected_workspace = None;
                let mut create_workspace = false;
                let mut duplicate_workspace = false;
                let mut edit_workspace = false;
                let mut reset_layout = false;
                ui.horizontal_wrapped(|ui| {
                    ui.weak("WORKSPACE");
                    for (id, name) in workspace_tabs {
                        if ui
                            .add(egui::Button::new(name).selected(id == self.active_workspace_id))
                            .clicked()
                        {
                            selected_workspace = Some(id);
                        }
                    }
                    if ui
                        .add_sized([30.0, 30.0], egui::Button::new("+"))
                        .on_hover_text("New workspace")
                        .clicked()
                    {
                        create_workspace = true;
                    }
                    ui.menu_button("...", |ui| {
                        if ui.button("Rename workspace").clicked() {
                            edit_workspace = true;
                            ui.close_menu();
                        }
                        if ui.button("Duplicate workspace").clicked() {
                            duplicate_workspace = true;
                            ui.close_menu();
                        }
                        if ui.button("Reset window layout").clicked() {
                            reset_layout = true;
                            ui.close_menu();
                        }
                    });
                });

                if let Some(id) = selected_workspace {
                    self.active_workspace_id = id;
                }
                if create_workspace {
                    self.create_workspace();
                } else if duplicate_workspace {
                    self.duplicate_active_workspace();
                } else if edit_workspace {
                    self.open_workspace_editor();
                }
                if reset_layout {
                    let workspace = self.active_workspace_mut();
                    workspace.layout_generation = workspace.layout_generation.saturating_add(1);
                    for panel in workspace.panels.values_mut() {
                        panel.detached = false;
                    }
                }

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                let visible = self.visible_screens().collect::<Vec<_>>();
                let operational = visible
                    .iter()
                    .copied()
                    .filter(|screen| screen.permission() != "admin")
                    .collect::<Vec<_>>();
                let administration = visible
                    .iter()
                    .copied()
                    .filter(|screen| screen.permission() == "admin")
                    .collect::<Vec<_>>();
                let mut panel_changes = Vec::new();
                let mut close_all = false;
                ui.horizontal_wrapped(|ui| {
                    for screen in operational {
                        let mut open = self
                            .active_workspace()
                            .panels
                            .get(&screen)
                            .is_some_and(|panel| panel.open);
                        if ui.toggle_value(&mut open, screen.title()).changed() {
                            panel_changes.push((screen, open));
                        }
                    }

                    if !administration.is_empty() {
                        ui.menu_button("Administration", |ui| {
                            ui.set_min_width(190.0);
                            for screen in administration {
                                let mut open = self
                                    .active_workspace()
                                    .panels
                                    .get(&screen)
                                    .is_some_and(|panel| panel.open);
                                if ui.toggle_value(&mut open, screen.title()).changed() {
                                    panel_changes.push((screen, open));
                                }
                            }
                        });
                    }
                    ui.separator();
                    if ui.button("Close all").clicked() {
                        close_all = true;
                    }
                });

                for (screen, open) in panel_changes {
                    self.set_panel_open(screen, open);
                }
                if close_all {
                    for panel in self.active_workspace_mut().panels.values_mut() {
                        panel.open = false;
                        panel.detached = false;
                    }
                }
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

    /// Render the active workspace's open panels as floating windows or
    /// detached native viewports.
    fn show_panels(&mut self, ctx: &egui::Context) {
        let workspace_id = self.active_workspace_id;
        let layout_generation = self.active_workspace().layout_generation;
        let open_screens: Vec<Screen> = self
            .visible_screens()
            .filter(|s| {
                self.active_workspace()
                    .panels
                    .get(s)
                    .is_some_and(|panel| panel.open)
            })
            .collect();

        for s in open_screens {
            let detached = self
                .active_workspace()
                .panels
                .get(&s)
                .is_some_and(|panel| panel.detached);
            if detached {
                self.show_detached(ctx, workspace_id, layout_generation, s);
            } else {
                self.show_window(ctx, workspace_id, layout_generation, s);
            }
        }
    }

    fn show_window(
        &mut self,
        ctx: &egui::Context,
        workspace_id: u64,
        layout_generation: u64,
        s: Screen,
    ) {
        let panel_index = Screen::ALL
            .iter()
            .position(|screen| *screen == s)
            .unwrap_or(0) as f32;
        let offset = (panel_index % 5.0) * 26.0;
        let mut open = true;
        let mut pop_out = false;
        let mut refresh = false;
        egui::Window::new(s.title())
            .id(egui::Id::new(("panel", workspace_id, layout_generation, s)))
            .open(&mut open)
            .resizable(true)
            .constrain(false)
            .default_pos(egui::pos2(16.0 + offset, 80.0 + offset))
            .default_size([820.0, 560.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_sized([30.0, 30.0], egui::Button::new("⏏"))
                        .on_hover_text("Pop out into a separate window")
                        .clicked()
                    {
                        pop_out = true;
                    }
                    if ui
                        .add_sized([30.0, 30.0], egui::Button::new("⟳"))
                        .on_hover_text("Refresh")
                        .clicked()
                    {
                        refresh = true;
                    }
                });
                ui.separator();
                self.render_screen(s, ui);
            });

        if refresh {
            self.fetch(s);
        }
        let panel = self.panel(s);
        if pop_out {
            panel.detached = true;
        }
        if !open {
            panel.open = false;
        }
    }

    fn show_detached(
        &mut self,
        ctx: &egui::Context,
        workspace_id: u64,
        layout_generation: u64,
        s: Screen,
    ) {
        let mut dock_back = false;
        let mut closed = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of(("panel-vp", workspace_id, layout_generation, s)),
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

        if closed {
            let st = self.panel(s);
            st.detached = false;
            st.open = false;
        } else if dock_back {
            self.panel(s).detached = false;
        }
    }

    fn render_screen(&mut self, s: Screen, ui: &mut egui::Ui) {
        match s {
            Screen::Orders => self.orders_screen(ui),
            Screen::Users => self.users_screen(ui),
            Screen::Roles => self.roles_screen(ui),
            Screen::Permissions => self.permissions_screen(ui),
            Screen::InventoryOwners => self.inventory_owners_screen(ui),
            Screen::Facilities => self.facilities_screen(ui),
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
