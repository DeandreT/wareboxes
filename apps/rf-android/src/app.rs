use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;
use lucide_icons::Icon;
use wareboxes_api_contract::v1::{ErrorReason, ErrorResponse};

use crate::command_store::{CommandStore, ExecutionScope};
use crate::cross_dock::{CrossDockScanStage, CrossDockWorkflow};
use crate::cycle_count::{CountScanStage, CycleCountWorkflow};
use crate::expected_receiving::{ExpectedReceivingReducer, ReceivingEffect};
use crate::outbound_load::{OutboundLoadScanStage, OutboundLoadWorkflow};
use crate::picking::{PickScanStage, PickingWorkflow};
use crate::replenishment::{ReplenishmentScanStage, ReplenishmentWorkflow};
use crate::transport::{NetworkEvent, ServerEndpoint};
use crate::workflow::{Activity, MovementWorkflow, ScanStage, Transition, WorkflowEffect};

mod cross_dock_session;
mod cross_dock_ui;
mod cycle_count_ui;
mod heartbeat;
mod movement_ui;
mod navigation;
mod outbound_load_session;
mod outbound_load_ui;
mod picking_ui;
mod receiving;
mod replenishment_session;
mod replenishment_ui;
mod session;
mod ui;

use navigation::WorkMode;
use receiving::ReceivingUiState;

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
    work_mode: WorkMode,
    work_menu_open: bool,
    workflow: MovementWorkflow,
    effects: VecDeque<WorkflowEffect>,
    cycle_count: CycleCountWorkflow,
    cycle_count_effects: VecDeque<WorkflowEffect>,
    picking: PickingWorkflow,
    picking_effects: VecDeque<WorkflowEffect>,
    replenishment: ReplenishmentWorkflow,
    replenishment_effects: VecDeque<WorkflowEffect>,
    cross_dock: CrossDockWorkflow,
    cross_dock_effects: VecDeque<WorkflowEffect>,
    outbound_load: OutboundLoadWorkflow,
    outbound_load_effects: VecDeque<WorkflowEffect>,
    receiving: ExpectedReceivingReducer,
    receiving_effects: VecDeque<ReceivingEffect>,
    receiving_request: Option<session::ReceivingRequest>,
    receiving_command: Option<session::ReceivingCommandRuntime>,
    receiving_ui: ReceivingUiState,
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
    count_scan_focus: Option<(i64, CountScanStage)>,
    pick_scan_focus: Option<(i64, PickScanStage)>,
    replenishment_scan_focus: Option<(i64, ReplenishmentScanStage)>,
    cross_dock_scan_focus: Option<(i64, CrossDockScanStage)>,
    outbound_load_scan_focus: Option<(i64, OutboundLoadScanStage)>,
    outbound_load_barcode_draft: String,
    expected_outbound_load_request_id: Option<String>,
    replenishment_task_id_draft: String,
    cross_dock_task_id_draft: String,
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
            work_mode: WorkMode::Putaway,
            work_menu_open: false,
            workflow: MovementWorkflow::default(),
            effects: VecDeque::new(),
            cycle_count: CycleCountWorkflow::default(),
            cycle_count_effects: VecDeque::new(),
            picking: PickingWorkflow::default(),
            picking_effects: VecDeque::new(),
            replenishment: ReplenishmentWorkflow::default(),
            replenishment_effects: VecDeque::new(),
            cross_dock: CrossDockWorkflow::default(),
            cross_dock_effects: VecDeque::new(),
            outbound_load: OutboundLoadWorkflow::default(),
            outbound_load_effects: VecDeque::new(),
            receiving: ExpectedReceivingReducer::default(),
            receiving_effects: VecDeque::new(),
            receiving_request: None,
            receiving_command: None,
            receiving_ui: ReceivingUiState::default(),
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
            count_scan_focus: None,
            pick_scan_focus: None,
            replenishment_scan_focus: None,
            cross_dock_scan_focus: None,
            outbound_load_scan_focus: None,
            outbound_load_barcode_draft: String::new(),
            expected_outbound_load_request_id: None,
            replenishment_task_id_draft: String::new(),
            cross_dock_task_id_draft: String::new(),
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
        let app = Self {
            work_mode: WorkMode::Putaway,
            work_menu_open: false,
            workflow: MovementWorkflow::default(),
            effects: VecDeque::new(),
            cycle_count: CycleCountWorkflow::default(),
            cycle_count_effects: VecDeque::new(),
            picking: PickingWorkflow::default(),
            picking_effects: VecDeque::new(),
            replenishment: ReplenishmentWorkflow::default(),
            replenishment_effects: VecDeque::new(),
            cross_dock: CrossDockWorkflow::default(),
            cross_dock_effects: VecDeque::new(),
            outbound_load: OutboundLoadWorkflow::default(),
            outbound_load_effects: VecDeque::new(),
            receiving: ExpectedReceivingReducer::default(),
            receiving_effects: VecDeque::new(),
            receiving_request: None,
            receiving_command: None,
            receiving_ui: ReceivingUiState::default(),
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
            count_scan_focus: None,
            pick_scan_focus: None,
            replenishment_scan_focus: None,
            cross_dock_scan_focus: None,
            outbound_load_scan_focus: None,
            outbound_load_barcode_draft: String::new(),
            expected_outbound_load_request_id: None,
            replenishment_task_id_draft: String::new(),
            cross_dock_task_id_draft: String::new(),
            field_focus_pending: true,
            heartbeat: heartbeat::HeartbeatRuntime::new(),
        };
        #[cfg(all(debug_assertions, not(target_os = "android")))]
        let app = {
            let mut app = app;
            app.open_debug_preview_from_environment();
            app
        };
        app
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
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new("WAREBOXES RF")
                                    .size(25.0)
                                    .strong()
                                    .color(Self::accent()),
                            );
                            ui.label(
                                egui::RichText::new("Warehouse execution")
                                    .color(egui::Color32::from_rgb(166, 177, 173)),
                            );
                        });
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
        ui.vertical_centered(|ui| {
            ui.heading("Connect this device");
            ui.add(
                egui::Label::new("Enter the secure server address provided by your administrator.")
                    .halign(egui::Align::Center),
            );
        });
        ui.add_space(20.0);
        ui.strong("Server address");
        let response = ui.add_sized(
            [ui.available_width(), 52.0],
            Self::centered_text_edit(
                egui::TextEdit::singleline(&mut self.server_url).id(egui::Id::new("rf_server_url")),
            ),
        );
        Self::centered_hint(
            ui,
            &response,
            self.server_url.is_empty(),
            "https://wms.example.com",
            egui::TextStyle::Body,
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
        let connect = Self::full_width_button(
            ui,
            can_connect,
            egui::Button::new(egui::RichText::new("Connect").strong())
                .fill(Self::primary_fill(can_connect)),
            56.0,
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
        ui.vertical_centered(|ui| {
            ui.heading(if self.reauth_scope.is_some() {
                "Sign in to recover work"
            } else {
                "Sign in"
            });
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
            Self::centered_text_edit(
                egui::TextEdit::singleline(&mut self.email).id(egui::Id::new("rf_email")),
            ),
        );
        Self::centered_hint(
            ui,
            &email,
            self.email.is_empty(),
            "operator@example.com",
            egui::TextStyle::Body,
        );
        ui.add_space(8.0);
        ui.strong("Password");
        let available = ui.available_width();
        let password = ui
            .horizontal(|ui| {
                let field_width = (available - 56.0).max(120.0);
                let response = ui.add_sized(
                    [field_width, 52.0],
                    Self::centered_text_edit(
                        egui::TextEdit::singleline(&mut self.password)
                            .password(!self.reveal_password)
                            .id(egui::Id::new("rf_password")),
                    ),
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
        let clicked = Self::full_width_button(
            ui,
            can_submit,
            egui::Button::new(egui::RichText::new(label).strong())
                .fill(Self::primary_fill(can_submit)),
            56.0,
        )
        .clicked();
        let enter = password.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if can_submit && (clicked || enter) {
            self.begin_sign_in(ui.ctx());
        }

        #[cfg(debug_assertions)]
        {
            if Self::show_preview_controls() {
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
    }

    #[cfg(debug_assertions)]
    fn show_preview_controls() -> bool {
        std::env::var("WAREBOXES_RF_SHOW_PREVIEW_TOOLS").is_ok_and(|value| value == "1")
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

const fn can_retry_connectivity_check(
    movement: Activity,
    cycle_count: Activity,
    picking: Activity,
    replenishment: Activity,
    cross_dock: Activity,
    outbound_load: Activity,
    request_pending: bool,
) -> bool {
    matches!(movement, Activity::Idle)
        && matches!(cycle_count, Activity::Idle)
        && matches!(picking, Activity::Idle)
        && matches!(replenishment, Activity::Idle)
        && matches!(cross_dock, Activity::Idle)
        && matches!(outbound_load, Activity::Idle)
        && !request_pending
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
        self.process_receiving_effects(root_ui.ctx());
        if self.receiving_command.is_some() {
            self.work_mode = WorkMode::Receive;
        }
        self.effects.extend(self.cycle_count_effects.drain(..));
        self.effects.extend(self.picking_effects.drain(..));
        self.effects.extend(self.replenishment_effects.drain(..));
        self.effects.extend(self.cross_dock_effects.drain(..));
        self.effects.extend(self.outbound_load_effects.drain(..));
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
        if !self.work_switch_allowed() {
            self.work_menu_open = false;
        }

        egui::Panel::top("rf_work_header")
            .exact_size(80.0)
            .frame(
                egui::Frame::side_top_panel(root_ui.style())
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .stroke(egui::Stroke::new(0.0, egui::Color32::TRANSPARENT)),
            )
            .show(root_ui, |ui| self.work_header(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(root_ui.style()).inner_margin(egui::Margin::same(12)))
            .show(root_ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("rf_work", self.work_mode))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.work_menu_open {
                            self.work_launcher(ui);
                            return;
                        }
                        if matches!(
                            self.work_mode,
                            WorkMode::Putaway
                                | WorkMode::Relocate
                                | WorkMode::Count
                                | WorkMode::Pick
                                | WorkMode::Replenish
                                | WorkMode::CrossDock
                                | WorkMode::OutboundLoad
                        ) && let Some(notice) = self.connectivity_notice.clone()
                        {
                            Self::message_band(ui, Self::warning(), Icon::WifiOff, &notice);
                            if can_retry_connectivity_check(
                                self.workflow.activity(),
                                self.cycle_count.activity(),
                                self.picking.activity(),
                                self.replenishment.activity(),
                                self.cross_dock.activity(),
                                self.outbound_load.activity(),
                                self.expected_claim_request_id.is_some(),
                            ) && ui
                                .add_sized(
                                    [ui.available_width(), 48.0],
                                    Self::secondary_button("Try again", ui.available_width(), 48.0),
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
                        } else {
                            match self.work_mode {
                                WorkMode::Receive => self.receiving_view(ui),
                                WorkMode::Count => self.count_view(ui),
                                WorkMode::Pick => self.pick_view(ui),
                                WorkMode::Replenish => self.replenishment_view(ui),
                                WorkMode::CrossDock => self.cross_dock_view(ui),
                                WorkMode::OutboundLoad => self.outbound_load_view(ui),
                                WorkMode::Putaway | WorkMode::Relocate => {
                                    if self.workflow.claim().is_some() {
                                        self.active_movement(ui);
                                    } else {
                                        match self.workflow.activity() {
                                            Activity::Idle => self.movement_idle(ui),
                                            Activity::Active => {
                                                self.workflow.require_reconciliation(
                                                    "Active work is missing its durable claim"
                                                        .into(),
                                                );
                                            }
                                            Activity::Persisting
                                            | Activity::ReadyToDispatch
                                            | Activity::InFlight
                                            | Activity::Ambiguous
                                            | Activity::ReconcileRequired => {}
                                        }
                                    }
                                    self.movement_command_state(ui);
                                    self.movement_error(ui);
                                }
                            }
                        }
                    });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::session::next_claim_operation_after_conflict;
    use super::{action_requested, can_retry_connectivity_check};
    use crate::workflow::{Activity, ClaimOperation};

    #[test]
    fn blocked_lease_rejects_keyboard_button_and_release_requests() {
        assert!(!action_requested(false, true));
        assert!(!action_requested(false, false));
        assert!(action_requested(true, true));
    }

    #[test]
    fn connectivity_retry_cannot_replace_any_active_workflow() {
        assert!(can_retry_connectivity_check(
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
            false,
        ));
        assert!(!can_retry_connectivity_check(
            Activity::Idle,
            Activity::Idle,
            Activity::Active,
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
            false,
        ));
        assert!(!can_retry_connectivity_check(
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
            Activity::Active,
            Activity::Idle,
            Activity::Idle,
            false,
        ));
        assert!(!can_retry_connectivity_check(
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
            true,
        ));
    }

    #[test]
    fn current_claim_recovery_reaches_non_movement_work() {
        assert_eq!(
            next_claim_operation_after_conflict(ClaimOperation::Putaway),
            Some(ClaimOperation::InventoryRelocation)
        );
        assert_eq!(
            next_claim_operation_after_conflict(ClaimOperation::InventoryRelocation),
            Some(ClaimOperation::CycleCount)
        );
    }
}
