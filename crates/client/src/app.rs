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
use lucide_icons::Icon;
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
    #[serde(default)]
    layout: WorkspaceLayout,
    panels: BTreeMap<Screen, PanelState>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
enum WorkspaceLayout {
    #[default]
    Free,
    Cascade,
    Tile,
}

#[derive(Debug, Clone, Copy)]
struct WindowPlacement {
    layout: WorkspaceLayout,
    docked_index: usize,
    docked_count: usize,
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
            layout: WorkspaceLayout::Free,
            panels: BTreeMap::new(),
        }
    }
}

struct Toast {
    message: String,
    error: bool,
    created: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaginationAction {
    None,
    Previous,
    Next,
    PageSize(i64),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum OrderDetailTab {
    #[default]
    Overview,
    Items,
    Reservations,
    Activity,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ItemDetailTab {
    #[default]
    Skus,
    Barcodes,
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
    open_item_ids: BTreeSet<i64>,
    order_detail_tabs: HashMap<i64, OrderDetailTab>,
    order_detail_focus: Option<i64>,
    item_detail_tabs: HashMap<i64, ItemDetailTab>,
    item_detail_focus: Option<i64>,
    new_order_open: bool,
    new_item_open: bool,
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
        Self::install_fonts(&cc.egui_ctx);
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
            open_item_ids: BTreeSet::new(),
            order_detail_tabs: HashMap::new(),
            order_detail_focus: None,
            item_detail_tabs: HashMap::new(),
            item_detail_focus: None,
            new_order_open: false,
            new_item_open: false,
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
            Screen::Employees => {
                self.api.get_list(Screen::Facilities);
            }
            Screen::Audits => {
                self.api.get_list(Screen::Facilities);
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
        let button = egui::Button::new(Self::icon(Icon::Trash2).color(red).strong())
            .fill(fill)
            .stroke(egui::Stroke::new(1.0_f32, red))
            .min_size(if small {
                egui::vec2(24.0, 22.0)
            } else {
                egui::vec2(34.0, 26.0)
            });
        let clicked = ui.add(button).on_hover_text("Delete").clicked();
        if clicked {
            self.request_delete(subject, path, body, refresh, success_message);
        }
    }

    fn request_delete(
        &mut self,
        subject: impl Into<String>,
        path: &'static str,
        body: Value,
        refresh: Screen,
        success_message: &'static str,
    ) {
        self.pending_delete = Some(PendingDelete {
            subject: subject.into(),
            path,
            body,
            refresh,
            success_message,
        });
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
                        egui::RichText::new("Delete")
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
                    self.open_item_ids.clear();
                    self.order_detail_tabs.clear();
                    self.order_detail_focus = None;
                    self.item_detail_tabs.clear();
                    self.item_detail_focus = None;
                    self.new_order_open = false;
                    self.new_item_open = false;
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
    std::env::var("WAREBOXES_API_URL")
        .ok()
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| LOCAL_BASE_URL.to_owned())
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
fn prefill_demo_login(forms: &mut Forms) {
    if let Ok(email) = std::env::var("WAREBOXES_DEMO_EMAIL") {
        forms.email = email;
    }
    if let Ok(password) = std::env::var("WAREBOXES_DEMO_PASSWORD") {
        forms.password = password;
    }
}

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
        self.new_order_window(ctx);
        self.new_item_window(ctx);
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
                    .inner_margin(egui::Margin::symmetric(8.0, 4.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("WAREBOXES")
                            .size(16.0)
                            .strong()
                            .color(Self::accent_color(ui)),
                    );
                    ui.separator();
                    if let Some(session) = &self.session {
                        ui.strong(&session.active_tenant.name);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if Self::icon_button(ui, Icon::LogOut, "Log out").clicked() {
                            self.api.logout();
                        }
                        if Self::icon_button(ui, Icon::Settings, "Settings").clicked() {
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

                ui.separator();

                let workspace_tabs = self
                    .workspaces
                    .iter()
                    .map(|workspace| (workspace.id, workspace.name.clone()))
                    .collect::<Vec<_>>();
                let mut selected_workspace = None;
                let mut create_workspace = false;
                let mut duplicate_workspace = false;
                let mut edit_workspace = false;
                let mut requested_layout = None;
                let current_layout = self.active_workspace().layout;

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
                let compact_navigation = ui.available_width() < 900.0;

                ui.horizontal(|ui| {
                    ui.label(Self::icon(Icon::SquareStack))
                        .on_hover_text("Workspaces");
                    for (id, name) in workspace_tabs {
                        if ui
                            .add(egui::Button::new(name).selected(id == self.active_workspace_id))
                            .clicked()
                        {
                            selected_workspace = Some(id);
                        }
                    }
                    if Self::icon_button(ui, Icon::Plus, "New workspace").clicked() {
                        create_workspace = true;
                    }
                    ui.menu_button(Self::icon(Icon::Ellipsis), |ui| {
                        ui.set_min_width(190.0);
                        if ui.button("Rename workspace").clicked() {
                            edit_workspace = true;
                            ui.close_menu();
                        }
                        if ui.button("Duplicate workspace").clicked() {
                            duplicate_workspace = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        for (layout, label) in [
                            (WorkspaceLayout::Free, "Free placement"),
                            (WorkspaceLayout::Cascade, "Cascade windows"),
                            (WorkspaceLayout::Tile, "Tile windows"),
                        ] {
                            if ui
                                .selectable_label(current_layout == layout, label)
                                .clicked()
                            {
                                requested_layout = Some(layout);
                                ui.close_menu();
                            }
                        }
                    });
                    ui.separator();
                    if compact_navigation {
                        ui.menu_button("Panels", |ui| {
                            ui.set_min_width(190.0);
                            ui.strong("Operations");
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
                                ui.separator();
                                ui.strong("Administration");
                            }
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
                    } else {
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
                    }
                    if Self::icon_button(ui, Icon::PanelTopClose, "Close all panels").clicked() {
                        close_all = true;
                    }
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
                if let Some(layout) = requested_layout {
                    self.arrange_active_workspace(layout);
                }

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
        let layout = self.active_workspace().layout;
        let open_screens: Vec<Screen> = self
            .visible_screens()
            .filter(|s| {
                self.active_workspace()
                    .panels
                    .get(s)
                    .is_some_and(|panel| panel.open)
            })
            .collect();
        let docked_screens = open_screens
            .iter()
            .copied()
            .filter(|screen| {
                !self
                    .active_workspace()
                    .panels
                    .get(screen)
                    .is_some_and(|panel| panel.detached)
            })
            .collect::<Vec<_>>();

        for s in open_screens {
            let detached = self
                .active_workspace()
                .panels
                .get(&s)
                .is_some_and(|panel| panel.detached);
            if detached {
                self.show_detached(ctx, workspace_id, layout_generation, s);
            } else {
                let docked_index = docked_screens
                    .iter()
                    .position(|screen| *screen == s)
                    .unwrap_or(0);
                self.show_window(
                    ctx,
                    workspace_id,
                    layout_generation,
                    s,
                    WindowPlacement {
                        layout,
                        docked_index,
                        docked_count: docked_screens.len(),
                    },
                );
            }
        }
    }

    fn show_window(
        &mut self,
        ctx: &egui::Context,
        workspace_id: u64,
        layout_generation: u64,
        s: Screen,
        placement: WindowPlacement,
    ) {
        let panel_index = Screen::ALL
            .iter()
            .position(|screen| *screen == s)
            .unwrap_or(0);
        let (default_pos, default_size) = Self::default_window_geometry(
            ctx,
            placement.layout,
            s,
            panel_index,
            placement.docked_index,
            placement.docked_count,
        );
        let mut open = true;
        let mut pop_out = false;
        let mut refresh = false;
        let panel_bounds = ctx.available_rect();
        egui::Window::new(s.title())
            .id(egui::Id::new(("panel", workspace_id, layout_generation, s)))
            .open(&mut open)
            .resizable(true)
            .constrain_to(panel_bounds)
            .default_pos(default_pos)
            .default_size(default_size)
            .show(ctx, |ui| {
                (pop_out, refresh) = self.panel_toolbar(ui, s, false);
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

    fn default_window_geometry(
        ctx: &egui::Context,
        layout: WorkspaceLayout,
        screen: Screen,
        panel_index: usize,
        docked_index: usize,
        docked_count: usize,
    ) -> (egui::Pos2, egui::Vec2) {
        let viewport = ctx.available_rect();
        let requested_size = match screen {
            Screen::Orders => egui::vec2(980.0, 620.0),
            Screen::Items => egui::vec2(920.0, 640.0),
            Screen::Loads | Screen::Inventory => egui::vec2(1080.0, 680.0),
            Screen::LicensePlates => egui::vec2(940.0, 620.0),
            _ => egui::vec2(840.0, 560.0),
        };
        let preferred_size = egui::vec2(
            requested_size.x.min((viewport.width() - 20.0).max(1.0)),
            requested_size.y.min((viewport.height() - 20.0).max(1.0)),
        );

        match layout {
            WorkspaceLayout::Free => {
                let offset = (panel_index % 6) as f32 * 28.0;
                (
                    egui::pos2(
                        viewport.left() + 10.0 + offset,
                        viewport.top() + 10.0 + offset,
                    ),
                    preferred_size,
                )
            }
            WorkspaceLayout::Cascade => {
                let offset = (docked_index % 8) as f32 * 32.0;
                (
                    egui::pos2(
                        viewport.left() + 10.0 + offset,
                        viewport.top() + 10.0 + offset,
                    ),
                    preferred_size,
                )
            }
            WorkspaceLayout::Tile => {
                let count = docked_count.max(1);
                let requested_columns = match count {
                    1 => 1,
                    2..=4 => 2,
                    _ => 3,
                };
                let fitting_columns = (viewport.width() / 360.0).floor().max(1.0) as usize;
                let columns = requested_columns.min(fitting_columns);
                let rows = count.div_ceil(columns);
                let column = docked_index % columns;
                let row = docked_index / columns;
                let gap = 8.0;
                let width = (viewport.width() - gap * (columns as f32 + 1.0)) / columns as f32;
                let height = (viewport.height() - gap * (rows as f32 + 1.0)) / rows as f32;
                (
                    egui::pos2(
                        viewport.left() + gap + column as f32 * (width + gap),
                        viewport.top() + gap + row as f32 * (height + gap),
                    ),
                    egui::vec2(width, height),
                )
            }
        }
    }

    fn panel_toolbar(&self, ui: &mut egui::Ui, screen: Screen, detached: bool) -> (bool, bool) {
        let mut viewport_action = false;
        let mut refresh = false;
        ui.horizontal(|ui| {
            let (icon, tooltip) = if detached {
                (Icon::PanelTopOpen, "Dock back into workspace")
            } else {
                (Icon::ExternalLink, "Open in a separate window")
            };
            if Self::icon_button(ui, icon, tooltip).clicked() {
                viewport_action = true;
            }
            if Self::icon_button(ui, Icon::RefreshCw, "Refresh panel").clicked() {
                refresh = true;
            }
            if let Some(last_fetch) = self.last_fetch.get(&screen) {
                let age = (self.now - last_fetch).max(0.0);
                ui.separator();
                if age < 0.8 {
                    ui.add(egui::Spinner::new().size(15.0));
                    ui.weak("Updating");
                } else {
                    ui.weak(format!("Updated {}", Self::relative_age(age)));
                }
            }
        });
        (viewport_action, refresh)
    }

    fn relative_age(seconds: f64) -> String {
        if seconds < 5.0 {
            "just now".to_owned()
        } else if seconds < 60.0 {
            format!("{}s ago", seconds as u64)
        } else {
            format!("{}m ago", (seconds / 60.0) as u64)
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
                    let (dock, refresh) = self.panel_toolbar(ui, s, true);
                    dock_back = dock;
                    if refresh {
                        self.fetch(s);
                    }
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
                    let age = self.now - t.created;
                    let visible = age < 4.55;
                    let opacity = ctx.animate_bool_with_time(
                        egui::Id::new(("toast_animation", t.created.to_bits())),
                        visible,
                        0.18,
                    );
                    let color = if t.error {
                        Self::danger_text_color(ui)
                    } else {
                        Self::accent_color(ui)
                    };
                    ui.scope(|ui| {
                        ui.set_opacity(opacity);
                        egui::Frame::none()
                            .fill(ui.visuals().window_fill)
                            .stroke(egui::Stroke::new(1.0_f32, color))
                            .rounding(4.0)
                            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let icon = if t.error {
                                        Icon::AlertTriangle
                                    } else {
                                        Icon::CheckCircle2
                                    };
                                    ui.label(Self::icon(icon).color(color));
                                    ui.label(&t.message);
                                });
                            });
                    });
                    ui.add_space(4.0);
                }
            });
    }
}
