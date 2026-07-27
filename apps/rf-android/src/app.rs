use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;
use lucide_icons::Icon;
use wareboxes_api_contract::v1::{ErrorReason, ErrorResponse};

use crate::command_store::{CommandStore, ExecutionScope};
use crate::transport::{NetworkEvent, ServerEndpoint};
use crate::workflow::{
    Activity, Location, PutawayClaim, PutawayKind, PutawayWork, PutawayWorkflow, ReleaseReason,
    ScanStage, Transition, WorkflowEffect,
};

mod heartbeat;
mod session;

const ICON_FONT: &str = "lucide";
const RF_SESSION_PATH: &str = "/api/v1/rf/sessions";

struct RfSession {
    endpoint: ServerEndpoint,
    token: String,
    tenant_name: String,
    scope: ExecutionScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionGate {
    SignedOut,
    SigningIn,
    Recovering,
    Ready,
}

pub struct RfApp {
    workflow: PutawayWorkflow,
    effects: VecDeque<WorkflowEffect>,
    command_store: Option<CommandStore>,
    execution_scope: Option<ExecutionScope>,
    storage_error: Option<String>,
    release_confirmation: bool,
    network_tx: Sender<NetworkEvent>,
    network_rx: Receiver<NetworkEvent>,
    session: Option<RfSession>,
    session_gate: SessionGate,
    expected_auth_request_id: Option<String>,
    expected_claim_request_id: Option<String>,
    lease_check_task_id: Option<i64>,
    lease_rejection_check: bool,
    reauth_scope: Option<ExecutionScope>,
    reauth_notice: Option<String>,
    server_url: String,
    server_configured: bool,
    email: String,
    password: String,
    reveal_password: bool,
    edit_server: bool,
    auth_error: Option<String>,
    connectivity_notice: Option<String>,
    device_id: String,
    scan_focus: Option<(i64, ScanStage)>,
    field_focus_pending: bool,
    heartbeat: heartbeat::HeartbeatRuntime,
}

impl RfApp {
    pub fn new(creation_context: &eframe::CreationContext<'_>) -> Self {
        let store = CommandStore::open_in_memory();
        Self::initialize(creation_context, store)
    }

    pub fn new_persistent(
        creation_context: &eframe::CreationContext<'_>,
        path: impl AsRef<Path>,
    ) -> Self {
        Self::initialize(creation_context, CommandStore::open(path))
    }

    pub fn new_without_storage(
        creation_context: &eframe::CreationContext<'_>,
        message: impl Into<String>,
    ) -> Self {
        Self::install_style(creation_context);
        let (network_tx, network_rx) = mpsc::channel();
        Self {
            workflow: PutawayWorkflow::default(),
            effects: VecDeque::new(),
            command_store: None,
            execution_scope: None,
            storage_error: Some(message.into()),
            release_confirmation: false,
            network_tx,
            network_rx,
            session: None,
            session_gate: SessionGate::SignedOut,
            expected_auth_request_id: None,
            expected_claim_request_id: None,
            lease_check_task_id: None,
            lease_rejection_check: false,
            reauth_scope: None,
            reauth_notice: None,
            server_url: default_server_url(),
            server_configured: false,
            email: String::new(),
            password: String::new(),
            reveal_password: false,
            edit_server: false,
            auth_error: None,
            connectivity_notice: None,
            device_id: format!("rf-{}", uuid::Uuid::new_v4()),
            scan_focus: None,
            field_focus_pending: true,
            heartbeat: heartbeat::HeartbeatRuntime::new(),
        }
    }

    fn initialize(
        creation_context: &eframe::CreationContext<'_>,
        store: Result<CommandStore, crate::command_store::CommandStoreError>,
    ) -> Self {
        Self::install_style(creation_context);
        let (network_tx, network_rx) = mpsc::channel();
        let fallback_server_url = default_server_url();
        let (command_store, storage_error, device_id, server_url, server_configured) = match store {
            Ok(store) => match store.device_profile() {
                Ok(profile) => {
                    let server_configured = profile.server_url.is_some();
                    (
                        Some(store),
                        None,
                        profile.device_id,
                        profile.server_url.unwrap_or(fallback_server_url),
                        server_configured,
                    )
                }
                Err(error) => (
                    None,
                    Some(error.to_string()),
                    String::new(),
                    fallback_server_url,
                    false,
                ),
            },
            Err(error) => (
                None,
                Some(error.to_string()),
                String::new(),
                fallback_server_url,
                false,
            ),
        };
        Self {
            workflow: PutawayWorkflow::default(),
            effects: VecDeque::new(),
            command_store,
            execution_scope: None,
            storage_error,
            release_confirmation: false,
            network_tx,
            network_rx,
            session: None,
            session_gate: SessionGate::SignedOut,
            expected_auth_request_id: None,
            expected_claim_request_id: None,
            lease_check_task_id: None,
            lease_rejection_check: false,
            reauth_scope: None,
            reauth_notice: None,
            edit_server: server_url.is_empty(),
            server_url,
            server_configured,
            email: String::new(),
            password: String::new(),
            reveal_password: false,
            auth_error: None,
            connectivity_notice: None,
            device_id,
            scan_focus: None,
            field_focus_pending: true,
            heartbeat: heartbeat::HeartbeatRuntime::new(),
        }
    }

    fn install_style(creation_context: &eframe::CreationContext<'_>) {
        Self::install_fonts(&creation_context.egui_ctx);
        creation_context.egui_ctx.set_theme(egui::Theme::Dark);
        creation_context
            .egui_ctx
            .set_style_of(egui::Theme::Dark, Self::style());
    }

    fn install_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();
        let fallbacks = fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        fonts.font_data.insert(
            ICON_FONT.to_owned(),
            egui::FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES).into(),
        );
        let icon_family = fonts
            .families
            .entry(egui::FontFamily::Name(ICON_FONT.into()))
            .or_default();
        icon_family.push(ICON_FONT.to_owned());
        icon_family.extend(fallbacks);
        ctx.set_fonts(fonts);
    }

    fn style() -> egui::Style {
        let mut style = egui::Style::default();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 10.0);
        style.spacing.interact_size = egui::vec2(48.0, 48.0);
        style.spacing.window_margin = egui::Margin::same(12);
        style.text_styles = [
            (
                egui::TextStyle::Heading,
                egui::FontId::new(22.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Body,
                egui::FontId::new(17.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Button,
                egui::FontId::new(17.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Monospace,
                egui::FontId::new(18.0, egui::FontFamily::Monospace),
            ),
            (
                egui::TextStyle::Small,
                egui::FontId::new(14.0, egui::FontFamily::Proportional),
            ),
        ]
        .into();

        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::from_rgb(14, 18, 17);
        visuals.window_fill = egui::Color32::from_rgb(22, 27, 25);
        visuals.extreme_bg_color = egui::Color32::from_rgb(8, 11, 10);
        visuals.faint_bg_color = egui::Color32::from_rgb(28, 34, 32);
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(4);
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(4);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(4);
        visuals.widgets.open.corner_radius = egui::CornerRadius::same(4);
        style.visuals = visuals;
        style
    }

    fn icon(icon: Icon) -> egui::RichText {
        egui::RichText::new(icon.unicode().to_string()).font(egui::FontId::new(
            19.0,
            egui::FontFamily::Name(ICON_FONT.into()),
        ))
    }

    fn accent() -> egui::Color32 {
        egui::Color32::from_rgb(45, 190, 139)
    }

    fn warning() -> egui::Color32 {
        egui::Color32::from_rgb(241, 179, 70)
    }

    fn danger() -> egui::Color32 {
        egui::Color32::from_rgb(245, 104, 93)
    }

    fn command_identity(operation: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4();
        (
            id.to_string(),
            format!("rf-{operation}-{}", uuid::Uuid::new_v4()),
        )
    }

    fn emit(&mut self, effect: Option<WorkflowEffect>) {
        if let Some(effect) = effect {
            self.effects.push_back(effect);
        }
    }

    fn emit_transition(&mut self, transition: Transition) {
        if let Transition::Effect(effect) = transition {
            self.effects.push_back(effect);
        }
    }

    fn signed_out_view(&mut self, root_ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(root_ui.style()).inner_margin(egui::Margin::same(16)))
            .show(root_ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("rf_sign_in")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new("WAREBOXES RF")
                                .size(25.0)
                                .strong()
                                .color(Self::accent()),
                        );
                        if self.edit_server {
                            self.server_setup(ui);
                        } else {
                            self.sign_in_form(ui);
                        }
                    });
            });
    }

    fn storage_failure_view(&self, root_ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(root_ui.style()).inner_margin(egui::Margin::same(16)))
            .show(root_ui, |ui| {
                ui.label(
                    egui::RichText::new("WAREBOXES RF")
                        .size(25.0)
                        .strong()
                        .color(Self::accent()),
                );
                ui.add_space(16.0);
                ui.heading("Device storage unavailable");
                ui.label("Work cannot be recorded safely. Close and reopen the app.");
                ui.add_space(12.0);
                Self::message_band(
                    ui,
                    Self::danger(),
                    Icon::ShieldAlert,
                    "Do not scan or move inventory on this device.",
                );
            });
    }

    fn server_setup(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.heading("Connect this device");
        ui.label("Enter the secure server address provided by your administrator.");
        ui.add_space(20.0);
        ui.strong("Server address");
        let response = ui.add_sized(
            [ui.available_width(), 52.0],
            egui::TextEdit::singleline(&mut self.server_url)
                .hint_text("https://wms.example.com")
                .id(egui::Id::new("rf_server_url")),
        );
        if self.field_focus_pending {
            response.request_focus();
            self.field_focus_pending = false;
        }
        if let Some(error) = self.auth_error.as_deref() {
            ui.add_space(8.0);
            Self::message_band(ui, Self::danger(), Icon::AlertTriangle, error);
        }
        ui.add_space(14.0);
        let can_connect = !self.server_url.trim().is_empty();
        let connect = ui.add_enabled(
            can_connect,
            egui::Button::new(egui::RichText::new("Connect").strong())
                .fill(Self::primary_fill(can_connect))
                .min_size(egui::vec2(ui.available_width(), 56.0)),
        );
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if connect.clicked() || enter {
            match ServerEndpoint::parse(&self.server_url) {
                Ok(endpoint) => {
                    if self.persist_server_endpoint(&endpoint) {
                        self.auth_error = None;
                        self.edit_server = false;
                        self.password.clear();
                        self.field_focus_pending = true;
                    }
                }
                Err(error) => self.auth_error = Some(format!("{error}.")),
            }
        }
        if self.server_configured
            && ui
                .add_sized(
                    [ui.available_width(), 48.0],
                    Self::secondary_button("Cancel", ui.available_width(), 48.0),
                )
                .clicked()
        {
            if let Some(Ok(Some(server_url))) =
                self.command_store.as_ref().map(CommandStore::server_url)
            {
                self.server_url = server_url;
            }
            self.auth_error = None;
            self.edit_server = false;
            self.field_focus_pending = true;
        }
    }

    fn persist_server_endpoint(&mut self, endpoint: &ServerEndpoint) -> bool {
        let normalized = endpoint.display();
        match self
            .command_store
            .as_mut()
            .map(|store| store.set_server_url(Some(&normalized)))
        {
            Some(Ok(_)) => {
                self.server_url = normalized;
                self.server_configured = true;
                true
            }
            Some(Err(_)) => {
                self.auth_error =
                    Some("Finish or recover saved work before changing the server.".into());
                false
            }
            None => {
                self.auth_error = Some("Device storage is unavailable.".into());
                false
            }
        }
    }

    fn sign_in_form(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.heading(if self.reauth_scope.is_some() {
            "Sign in to recover work"
        } else {
            "Sign in"
        });
        ui.add_space(8.0);
        egui::containers::Sides::new().height(48.0).show(
            ui,
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(Icon::Server).color(Self::accent()));
                    ui.label(
                        egui::RichText::new(server_display_name(&self.server_url))
                            .small()
                            .strong(),
                    );
                });
            },
            |ui| {
                let blocked = self.reauth_scope.is_some();
                if ui
                    .add_enabled(
                        !blocked,
                        egui::Button::new(Self::icon(Icon::Settings))
                            .min_size(egui::vec2(48.0, 48.0)),
                    )
                    .on_hover_text("Server settings")
                    .on_disabled_hover_text("Recover saved work before changing server")
                    .clicked()
                {
                    self.auth_error = None;
                    self.edit_server = true;
                    self.password.clear();
                    self.field_focus_pending = true;
                }
            },
        );
        ui.add_space(12.0);
        ui.strong("Email");
        let email = ui.add_sized(
            [ui.available_width(), 52.0],
            egui::TextEdit::singleline(&mut self.email)
                .id(egui::Id::new("rf_email"))
                .hint_text("operator@example.com"),
        );
        ui.add_space(8.0);
        ui.strong("Password");
        let available = ui.available_width();
        let password = ui
            .horizontal(|ui| {
                let field_width = (available - 56.0).max(120.0);
                let response = ui.add_sized(
                    [field_width, 52.0],
                    egui::TextEdit::singleline(&mut self.password)
                        .password(!self.reveal_password)
                        .id(egui::Id::new("rf_password")),
                );
                let icon = if self.reveal_password {
                    Icon::EyeOff
                } else {
                    Icon::Eye
                };
                if ui
                    .add(egui::Button::new(Self::icon(icon)).min_size(egui::vec2(48.0, 52.0)))
                    .on_hover_text(if self.reveal_password {
                        "Hide password"
                    } else {
                        "Show password"
                    })
                    .clicked()
                {
                    self.reveal_password = !self.reveal_password;
                }
                response
            })
            .inner;
        if self.field_focus_pending && self.session_gate == SessionGate::SignedOut {
            if self.email.is_empty() {
                email.request_focus();
            } else {
                password.request_focus();
            }
            self.field_focus_pending = false;
        }

        if let Some(error) = self.auth_error.as_deref() {
            ui.add_space(8.0);
            Self::message_band(ui, Self::danger(), Icon::AlertTriangle, error);
        }
        if let Some(notice) = self.reauth_notice.as_deref() {
            ui.add_space(8.0);
            Self::message_band(ui, Self::warning(), Icon::LogIn, notice);
        }
        ui.add_space(14.0);
        let signing_in = self.session_gate == SessionGate::SigningIn;
        let can_submit = !signing_in
            && valid_email(self.email.trim())
            && !self.password.is_empty()
            && !self.server_url.is_empty();
        let label = if signing_in {
            "Signing in..."
        } else {
            "Sign in"
        };
        let clicked = ui
            .add_enabled(
                can_submit,
                egui::Button::new(egui::RichText::new(label).strong())
                    .fill(Self::primary_fill(can_submit))
                    .min_size(egui::vec2(ui.available_width(), 56.0)),
            )
            .clicked();
        let enter = password.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if can_submit && (clicked || enter) {
            self.begin_sign_in(ui.ctx());
        }

        #[cfg(debug_assertions)]
        {
            ui.add_space(12.0);
            if ui
                .add_sized(
                    [ui.available_width(), 48.0],
                    Self::secondary_button("Open workflow preview", ui.available_width(), 48.0),
                )
                .clicked()
            {
                self.open_debug_preview();
            }
        }
    }

    fn message_band(ui: &mut egui::Ui, color: egui::Color32, icon: Icon, message: &str) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(color.gamma_multiply(0.14))
            .stroke(egui::Stroke::new(1.0, color))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_min_width((width - 20.0).max(0.0));
                ui.horizontal_wrapped(|ui| {
                    ui.label(Self::icon(icon).color(color));
                    ui.label(message);
                });
            });
    }

    #[cfg(debug_assertions)]
    fn open_debug_preview(&mut self) {
        let endpoint = ServerEndpoint::parse("http://127.0.0.1:3000")
            .or_else(|_| ServerEndpoint::parse("https://localhost"));
        let Ok(endpoint) = endpoint else {
            return;
        };
        let scope = ExecutionScope {
            tenant_id: 1,
            operator_id: 1,
            device_id: "desktop-preview".into(),
        };
        self.session = Some(RfSession {
            endpoint,
            token: "preview".into(),
            tenant_name: "Workflow preview".into(),
            scope: scope.clone(),
        });
        self.execution_scope = Some(scope);
        self.session_gate = SessionGate::Ready;
        self.workflow.load_debug_claim(Self::debug_claim());
    }

    fn header(&self, ui: &mut egui::Ui) {
        let (label, color) =
            self.heartbeat_header()
                .unwrap_or_else(|| match self.workflow.activity() {
                    Activity::Idle => ("READY", Self::accent()),
                    Activity::Active => ("ACTIVE", Self::accent()),
                    Activity::Persisting => ("SAVING", Self::warning()),
                    Activity::ReadyToDispatch => ("QUEUED", Self::warning()),
                    Activity::InFlight => ("SENDING", Self::warning()),
                    Activity::Ambiguous => ("CHECK", Self::danger()),
                    Activity::ReconcileRequired => ("BLOCKED", Self::danger()),
                });
        egui::containers::Sides::new().height(34.0).show(
            ui,
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(Self::icon(Icon::PackageOpen).color(Self::accent()));
                    ui.heading("Putaway");
                });
            },
            |ui| {
                ui.label(egui::RichText::new(label).strong().color(color));
            },
        );
        if let Some(session) = self.session.as_ref() {
            ui.label(
                egui::RichText::new(&session.tenant_name)
                    .small()
                    .color(egui::Color32::from_rgb(166, 177, 173)),
            );
        }
        ui.separator();
    }

    fn idle(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("WORK TYPE").small().strong());
        let segment_width = (ui.available_width() - 8.0) / 2.0;
        ui.horizontal(|ui| {
            for kind in [PutawayKind::Loose, PutawayKind::LicensePlate] {
                let selected = self.workflow.selected_kind() == kind;
                if ui
                    .add_sized(
                        [segment_width, 52.0],
                        egui::Button::selectable(selected, kind.label()),
                    )
                    .clicked()
                {
                    self.workflow.select_kind(kind);
                }
            }
        });
        ui.add_space(10.0);

        let can_execute = self.can_execute();
        let button = egui::Button::new(egui::RichText::new("Get next task").strong())
            .fill(Self::primary_fill(can_execute))
            .min_size(egui::vec2(ui.available_width(), 58.0));
        if ui.add_enabled(can_execute, button).clicked() {
            let (command_id, key) = Self::command_identity("claim");
            let effect = self.workflow.begin_claim_next(command_id, key);
            self.emit(effect);
        }

        if let Some(error) = self.storage_error.as_deref() {
            ui.add_space(12.0);
            ui.colored_label(Self::danger(), error);
        }

        #[cfg(debug_assertions)]
        {
            ui.add_space(12.0);
            if ui
                .add_sized(
                    [ui.available_width(), 48.0],
                    egui::Button::new("Load preview task")
                        .fill(ui.visuals().faint_bg_color)
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(79, 91, 87))),
                )
                .clicked()
            {
                self.workflow.load_debug_claim(Self::debug_claim());
            }
        }

        if let Some(notice) = self.workflow.notice() {
            ui.add_space(12.0);
            ui.colored_label(Self::warning(), notice);
        }
    }

    fn active_work(&mut self, ui: &mut egui::Ui) {
        let Some(claim) = self.workflow.claim().cloned() else {
            return;
        };

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("TASK {}", claim.task_id))
                    .strong()
                    .color(Self::accent()),
            );
            ui.separator();
            ui.label(format!("Priority {}", claim.priority));
        });

        if let Some(source) = &claim.source {
            Self::location_band(ui, "SOURCE", source);
        }
        Self::work_band(ui, &claim.work);
        Self::location_band(ui, "DESTINATION", &claim.destination);

        if let Some(instructions) = claim.instructions.as_deref() {
            ui.label(
                egui::RichText::new(instructions)
                    .color(Self::warning())
                    .strong(),
            );
        }

        let lease_actions_allowed = if self.workflow.activity() == Activity::Active {
            self.heartbeat_status(ui, claim.task_id)
        } else {
            false
        };
        if let Some(stage) = self.workflow.expected_scan() {
            self.scan_control(ui, claim.task_id, stage, lease_actions_allowed);
        }

        ui.add_space(8.0);
        if self.release_confirmation {
            self.release_confirmation(ui, lease_actions_allowed);
        } else {
            let release_clicked = ui
                .add_enabled(
                    lease_actions_allowed,
                    Self::secondary_button("Release work", ui.available_width(), 48.0),
                )
                .on_disabled_hover_text("Check task connection first")
                .clicked();
            if self.workflow.activity() == Activity::Active
                && action_requested(lease_actions_allowed, release_clicked)
            {
                self.release_confirmation = true;
            }
        }
    }

    fn location_band(ui: &mut egui::Ui, label: &str, location: &Location) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(0.0));
                ui.label(egui::RichText::new(label).small().strong());
                let display_name = location.display_name();
                ui.label(egui::RichText::new(&display_name).size(23.0).strong());
                if let Some(barcode) = location
                    .barcode
                    .as_deref()
                    .filter(|barcode| *barcode != display_name)
                {
                    ui.monospace(barcode);
                }
            });
    }

    fn work_band(ui: &mut egui::Ui, work: &PutawayWork) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(0.0));
                match work {
                    PutawayWork::Loose {
                        item_description,
                        item_id,
                        quantity,
                        uom,
                        lot,
                        serial,
                    } => {
                        ui.label(egui::RichText::new("LOOSE INVENTORY").small().strong());
                        ui.label(
                            egui::RichText::new(
                                item_description
                                    .clone()
                                    .unwrap_or_else(|| format!("Item {item_id}")),
                            )
                            .size(21.0)
                            .strong(),
                        );
                        ui.label(format!("{quantity} {uom}"));
                        if let Some(lot) = lot {
                            ui.monospace(format!("Lot {lot}"));
                        }
                        if let Some(serial) = serial {
                            ui.monospace(format!("Serial {serial}"));
                        }
                    }
                    PutawayWork::LicensePlate {
                        barcode,
                        planned_balance_count,
                    } => {
                        ui.label(egui::RichText::new("LICENSE PLATE").small().strong());
                        ui.monospace(egui::RichText::new(barcode).size(23.0).strong());
                        ui.label(format!("{planned_balance_count} inventory balances"));
                    }
                }
            });
    }

    fn scan_control(
        &mut self,
        ui: &mut egui::Ui,
        task_id: i64,
        stage: ScanStage,
        lease_actions_allowed: bool,
    ) {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(stage.prompt())
                .size(19.0)
                .strong()
                .color(Self::accent()),
        );
        let response = ui
            .add_enabled_ui(lease_actions_allowed, |ui| {
                ui.add_sized(
                    [ui.available_width(), 56.0],
                    egui::TextEdit::singleline(self.workflow.scan_draft_mut())
                        .id(egui::Id::new(("putaway_scan", task_id, stage)))
                        .font(egui::TextStyle::Monospace)
                        .hint_text("SCAN"),
                )
            })
            .inner;
        let focus_key = (task_id, stage);
        if lease_actions_allowed && self.scan_focus != Some(focus_key) {
            response.request_focus();
            self.scan_focus = Some(focus_key);
        } else if !lease_actions_allowed {
            self.scan_focus = None;
        }
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        let scan_ready = !self.workflow.scan_draft_mut().trim().is_empty();
        let can_confirm = scan_ready && lease_actions_allowed;
        let clicked = ui
            .add_enabled(
                can_confirm,
                egui::Button::new(egui::RichText::new("Confirm scan").strong())
                    .fill(Self::primary_fill(can_confirm))
                    .min_size(egui::vec2(ui.available_width(), 54.0)),
            )
            .on_disabled_hover_text(if lease_actions_allowed {
                "A scan is required"
            } else {
                "Check task connection first"
            })
            .clicked();
        if action_requested(lease_actions_allowed, enter || clicked) {
            let (command_id, key) = Self::command_identity("confirm");
            let effect = self.workflow.submit_scan(command_id, key);
            self.emit(effect);
        }
    }

    fn secondary_button(label: &str, width: f32, height: f32) -> egui::Button<'static> {
        egui::Button::new(egui::RichText::new(label.to_owned()))
            .fill(egui::Color32::from_rgb(28, 34, 32))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(79, 91, 87)))
            .min_size(egui::vec2(width, height))
    }

    fn primary_fill(enabled: bool) -> egui::Color32 {
        if enabled {
            egui::Color32::from_rgb(18, 112, 81)
        } else {
            egui::Color32::from_rgb(28, 34, 32)
        }
    }

    fn release_confirmation(&mut self, ui: &mut egui::Ui, lease_actions_allowed: bool) {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(57, 42, 21))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.strong("Return this task to the queue?");
                ui.horizontal(|ui| {
                    if ui
                        .add(Self::secondary_button(
                            "Cancel",
                            ui.available_width() / 2.0 - 4.0,
                            48.0,
                        ))
                        .clicked()
                    {
                        self.release_confirmation = false;
                    }
                    let release_clicked = ui
                        .add_enabled(
                            lease_actions_allowed,
                            egui::Button::new("Return to queue")
                                .fill(if lease_actions_allowed {
                                    egui::Color32::from_rgb(112, 72, 18)
                                } else {
                                    egui::Color32::from_rgb(28, 34, 32)
                                })
                                .min_size(egui::vec2(ui.available_width(), 48.0)),
                        )
                        .on_disabled_hover_text("Check task connection first")
                        .clicked();
                    if action_requested(lease_actions_allowed, release_clicked) {
                        let (command_id, key) = Self::command_identity("release");
                        let effect = self.workflow.begin_release(
                            command_id,
                            key,
                            ReleaseReason::WorkInterrupted,
                            None,
                        );
                        self.emit(effect);
                        self.release_confirmation = false;
                    }
                });
            });
    }

    fn command_state(&mut self, ui: &mut egui::Ui) {
        match self.workflow.activity() {
            Activity::Persisting => Self::state_band(
                ui,
                Self::warning(),
                Icon::Save,
                "Saving scan",
                "Do not scan again",
            ),
            Activity::ReadyToDispatch => Self::state_band(
                ui,
                Self::warning(),
                Icon::Send,
                "Scan saved",
                "Waiting for connection. Do not scan again.",
            ),
            Activity::InFlight => Self::state_band(
                ui,
                Self::warning(),
                Icon::Loader,
                "Sending scan",
                "Waiting for the server. Do not scan again.",
            ),
            Activity::Ambiguous => {
                let message = self
                    .workflow
                    .ambiguous_message()
                    .unwrap_or("The command result is unknown");
                Self::state_band(
                    ui,
                    Self::danger(),
                    Icon::AlertTriangle,
                    "Checking last scan",
                    message,
                );
                if ui
                    .add_sized(
                        [ui.available_width(), 54.0],
                        egui::Button::new(egui::RichText::new("Check again").strong())
                            .fill(egui::Color32::from_rgb(112, 72, 18)),
                    )
                    .clicked()
                {
                    let effect = self.workflow.retry_ambiguous();
                    self.emit(effect);
                }
            }
            Activity::ReconcileRequired => Self::state_band(
                ui,
                Self::danger(),
                Icon::ShieldAlert,
                "Work blocked",
                self.workflow
                    .reconcile_reason()
                    .unwrap_or("Device and server state must be reconciled"),
            ),
            Activity::Idle | Activity::Active => {}
        }
    }

    fn state_band(ui: &mut egui::Ui, color: egui::Color32, icon: Icon, title: &str, detail: &str) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(color.gamma_multiply(0.16))
            .stroke(egui::Stroke::new(1.0, color))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(0.0));
                ui.horizontal(|ui| {
                    ui.label(Self::icon(icon).color(color));
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(title).strong().color(color));
                        ui.label(detail);
                    });
                });
            });
    }

    fn error(&self, ui: &mut egui::Ui) {
        if let Some(error) = self.workflow.error() {
            ui.colored_label(Self::danger(), egui::RichText::new(error).strong());
        }
    }

    #[cfg(debug_assertions)]
    fn debug_claim() -> PutawayClaim {
        PutawayClaim {
            task_id: 1042,
            inventory_owner_id: 12,
            facility_id: 4,
            priority: 80,
            instructions: Some("Keep upright".into()),
            lease_expires_at: (chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339(),
            source: Some(Location {
                location_id: 17,
                name: Some("Receiving 01".into()),
                barcode: Some("RECV-01".into()),
            }),
            destination: Location {
                location_id: 31,
                name: Some("A-01-03".into()),
                barcode: Some("A-01-03".into()),
            },
            work: PutawayWork::Loose {
                item_description: Some("Case-picked item".into()),
                item_id: 88,
                quantity: 4,
                uom: "cases".into(),
                lot: Some("LOT-2407".into()),
                serial: None,
            },
        }
    }
}

fn valid_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !value.chars().any(char::is_whitespace)
        && value.len() <= 254
}

const fn action_requested(actions_allowed: bool, requested: bool) -> bool {
    actions_allowed && requested
}

fn server_display_name(value: &str) -> String {
    ServerEndpoint::parse(value)
        .ok()
        .and_then(|endpoint| url::Url::parse(&endpoint.display()).ok())
        .and_then(|url| {
            let host = url.host_str()?.to_owned();
            Some(match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_else(|| "Server not configured".into())
}

fn session_error_message(status: u16, body: &[u8]) -> String {
    match status {
        401 => "Email or password is incorrect.".into(),
        422 => serde_json::from_slice::<ErrorResponse>(body)
            .ok()
            .and_then(|error| error.violations.first().map(|item| item.field.clone()))
            .map(|field| match field.as_str() {
                "email" => "Enter a valid email address.".to_owned(),
                "password" => "Enter your password.".to_owned(),
                _ => "Check the sign-in details and try again.".to_owned(),
            })
            .unwrap_or_else(|| "Check the sign-in details and try again.".into()),
        500..=599 => "Server unavailable. Try again in a moment.".into(),
        _ => "Sign in failed. Try again.".into(),
    }
}

fn rejected_command_message(error: Option<&ErrorResponse>) -> String {
    match error.map(|error| error.reason) {
        Some(ErrorReason::ValidationFailed | ErrorReason::InvalidRequest) => {
            "The scan was not accepted. Check the task and scan again.".into()
        }
        _ => "The command was not accepted. Check the task and try again.".into(),
    }
}

fn support_message(message: &str, request_id: Option<&str>) -> String {
    match request_id.filter(|request_id| !request_id.is_empty()) {
        Some(request_id) => format!("{message} Request {request_id}."),
        None => message.to_owned(),
    }
}

#[cfg(not(target_os = "android"))]
fn default_server_url() -> String {
    std::env::var("WAREBOXES_API_URL")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:3000".into())
}

#[cfg(target_os = "android")]
fn default_server_url() -> String {
    option_env!("WAREBOXES_RF_API_URL")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_owned()
}

impl eframe::App for RfApp {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.process_network_events(root_ui.ctx());
        self.persist_queued_commands();
        self.dispatch_queued_commands(root_ui.ctx());
        self.maintain_claim_heartbeat(root_ui.ctx());

        #[cfg(target_os = "android")]
        egui::Panel::top("android_status_bar_space")
            .exact_size(28.0)
            .show(root_ui, |_| {});

        if self.storage_error.is_some() || self.command_store.is_none() {
            self.storage_failure_view(root_ui);
            return;
        }
        if self.session.is_none() {
            self.signed_out_view(root_ui);
            return;
        }

        egui::Panel::top("putaway_header")
            .frame(
                egui::Frame::side_top_panel(root_ui.style())
                    .inner_margin(egui::Margin::symmetric(12, 10)),
            )
            .show(root_ui, |ui| self.header(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(root_ui.style()).inner_margin(egui::Margin::same(12)))
            .show(root_ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("putaway_work")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(notice) = self.connectivity_notice.clone() {
                            Self::message_band(ui, Self::warning(), Icon::WifiOff, &notice);
                            if self.workflow.activity() == Activity::Idle
                                && self.expected_claim_request_id.is_none()
                                && ui
                                    .add_sized(
                                        [ui.available_width(), 48.0],
                                        Self::secondary_button(
                                            "Try again",
                                            ui.available_width(),
                                            48.0,
                                        ),
                                    )
                                    .clicked()
                            {
                                self.connectivity_notice = None;
                                self.session_gate = SessionGate::Recovering;
                                self.request_current_claim(ui.ctx());
                            }
                            ui.add_space(8.0);
                        }
                        if self.session_gate == SessionGate::Recovering {
                            Self::state_band(
                                ui,
                                Self::warning(),
                                Icon::Loader,
                                "Checking saved work",
                                "Waiting for the server",
                            );
                        } else if self.workflow.claim().is_some() {
                            self.active_work(ui);
                        } else {
                            match self.workflow.activity() {
                                Activity::Idle => self.idle(ui),
                                Activity::Active => {
                                    self.workflow.require_reconciliation(
                                        "Active work is missing its durable claim".into(),
                                    );
                                }
                                Activity::Persisting
                                | Activity::ReadyToDispatch
                                | Activity::InFlight
                                | Activity::Ambiguous
                                | Activity::ReconcileRequired => {}
                            }
                        }
                        self.command_state(ui);
                        self.error(ui);
                    });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::action_requested;

    #[test]
    fn blocked_lease_rejects_keyboard_button_and_release_requests() {
        assert!(!action_requested(false, true));
        assert!(!action_requested(false, false));
        assert!(action_requested(true, true));
    }
}
