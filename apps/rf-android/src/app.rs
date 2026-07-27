use std::collections::VecDeque;
use std::path::Path;

use eframe::egui;
use lucide_icons::Icon;

use crate::command_store::{CommandStore, ExecutionScope};
use crate::workflow::{
    Activity, Location, PutawayClaim, PutawayKind, PutawayWork, PutawayWorkflow, ReleaseReason,
    ScanStage, Transition, WorkflowEffect,
};

const ICON_FONT: &str = "lucide";

pub struct RfApp {
    workflow: PutawayWorkflow,
    effects: VecDeque<WorkflowEffect>,
    command_store: Option<CommandStore>,
    execution_scope: Option<ExecutionScope>,
    storage_error: Option<String>,
    release_confirmation: bool,
}

impl RfApp {
    pub fn new(creation_context: &eframe::CreationContext<'_>) -> Self {
        let store = CommandStore::open_in_memory();
        Self::initialize(
            creation_context,
            store,
            Some(ExecutionScope {
                tenant_id: 1,
                operator_id: 1,
                device_id: "desktop-preview".into(),
            }),
        )
    }

    pub fn new_persistent(
        creation_context: &eframe::CreationContext<'_>,
        path: impl AsRef<Path>,
    ) -> Self {
        Self::initialize(creation_context, CommandStore::open(path), None)
    }

    pub fn new_without_storage(
        creation_context: &eframe::CreationContext<'_>,
        message: impl Into<String>,
    ) -> Self {
        Self::install_style(creation_context);
        Self {
            workflow: PutawayWorkflow::default(),
            effects: VecDeque::new(),
            command_store: None,
            execution_scope: None,
            storage_error: Some(message.into()),
            release_confirmation: false,
        }
    }

    fn initialize(
        creation_context: &eframe::CreationContext<'_>,
        store: Result<CommandStore, crate::command_store::CommandStoreError>,
        execution_scope: Option<ExecutionScope>,
    ) -> Self {
        Self::install_style(creation_context);
        let (command_store, storage_error) = match store {
            Ok(store) => (Some(store), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            workflow: PutawayWorkflow::default(),
            effects: VecDeque::new(),
            command_store,
            execution_scope,
            storage_error,
            release_confirmation: false,
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

    fn persist_queued_commands(&mut self) {
        let queued = self.effects.len();
        for _ in 0..queued {
            let Some(effect) = self.effects.pop_front() else {
                break;
            };
            let WorkflowEffect::PersistCommand(draft) = effect else {
                self.effects.push_back(effect);
                continue;
            };
            let Some(store) = self.command_store.as_mut() else {
                self.workflow
                    .require_reconciliation("Durable device storage is unavailable".into());
                continue;
            };
            let Some(scope) = self.execution_scope.as_ref() else {
                self.workflow.require_reconciliation(
                    "The command cannot be stored without an authenticated device scope".into(),
                );
                continue;
            };
            let command_id = draft.command_id.clone();
            match store.persist(scope, draft) {
                Ok(record) => {
                    let transition = self
                        .workflow
                        .command_persisted(&command_id, record.record_id);
                    self.emit_transition(transition);
                }
                Err(error) => {
                    self.workflow.require_reconciliation(format!(
                        "The command could not be stored durably: {error}"
                    ));
                }
            }
        }
    }

    fn can_execute(&self) -> bool {
        self.command_store.is_some() && self.execution_scope.is_some()
    }

    fn header(&self, ui: &mut egui::Ui) {
        let (label, color) = match self.workflow.activity() {
            Activity::Idle => ("READY", Self::accent()),
            Activity::Active => ("ACTIVE", Self::accent()),
            Activity::Persisting => ("SAVING", Self::warning()),
            Activity::ReadyToDispatch => ("QUEUED", Self::warning()),
            Activity::InFlight => ("SENDING", Self::warning()),
            Activity::Ambiguous => ("CHECK", Self::danger()),
            Activity::ReconcileRequired => ("BLOCKED", Self::danger()),
        };
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
        ui.separator();
    }

    fn idle(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("WORK TYPE").small().strong());
        ui.horizontal(|ui| {
            for kind in [PutawayKind::Loose, PutawayKind::LicensePlate] {
                let selected = self.workflow.selected_kind() == kind;
                if ui
                    .add_sized(
                        [ui.available_width() / 2.0 - 4.0, 52.0],
                        egui::Button::selectable(selected, kind.label()),
                    )
                    .clicked()
                {
                    self.workflow.select_kind(kind);
                }
            }
        });
        ui.add_space(10.0);

        let button = egui::Button::new(egui::RichText::new("Get next task").strong())
            .fill(egui::Color32::from_rgb(18, 112, 81))
            .min_size(egui::vec2(ui.available_width(), 58.0));
        if ui.add_enabled(self.can_execute(), button).clicked() {
            let (command_id, key) = Self::command_identity("claim");
            let effect = self.workflow.begin_claim_next(command_id, key);
            self.emit(effect);
        }

        if let Some(error) = self.storage_error.as_deref() {
            ui.add_space(12.0);
            ui.colored_label(Self::danger(), error);
        } else if self.execution_scope.is_none() {
            ui.add_space(12.0);
            ui.colored_label(Self::warning(), "Sign in is required before claiming work");
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

        if let Some(stage) = self.workflow.expected_scan() {
            self.scan_control(ui, claim.task_id, stage);
        }

        ui.add_space(8.0);
        if self.release_confirmation {
            self.release_confirmation(ui);
        } else if self.workflow.activity() == Activity::Active
            && ui
                .add(Self::secondary_button(
                    "Release work",
                    ui.available_width(),
                    48.0,
                ))
                .clicked()
        {
            self.release_confirmation = true;
        }
    }

    fn location_band(ui: &mut egui::Ui, label: &str, location: &Location) {
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(label).small().strong());
                ui.label(
                    egui::RichText::new(location.display_name())
                        .size(23.0)
                        .strong(),
                );
                if location.display_name() != location.barcode {
                    ui.monospace(&location.barcode);
                }
            });
    }

    fn work_band(ui: &mut egui::Ui, work: &PutawayWork) {
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| match work {
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
            });
    }

    fn scan_control(&mut self, ui: &mut egui::Ui, task_id: i64, stage: ScanStage) {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(stage.prompt())
                .size(19.0)
                .strong()
                .color(Self::accent()),
        );
        let response = ui.add_sized(
            [ui.available_width(), 56.0],
            egui::TextEdit::singleline(self.workflow.scan_draft_mut())
                .id(egui::Id::new(("putaway_scan", task_id, stage)))
                .font(egui::TextStyle::Monospace)
                .hint_text("SCAN"),
        );
        if !response.has_focus() {
            response.request_focus();
        }
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        let scan_ready = !self.workflow.scan_draft_mut().trim().is_empty();
        if enter
            || ui
                .add_enabled(
                    scan_ready,
                    egui::Button::new(egui::RichText::new("Confirm scan").strong())
                        .fill(egui::Color32::from_rgb(18, 112, 81))
                        .min_size(egui::vec2(ui.available_width(), 54.0)),
                )
                .on_disabled_hover_text("A scan is required")
                .clicked()
        {
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

    fn release_confirmation(&mut self, ui: &mut egui::Ui) {
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
                    if ui
                        .add(
                            egui::Button::new("Return to queue")
                                .fill(egui::Color32::from_rgb(112, 72, 18))
                                .min_size(egui::vec2(ui.available_width(), 48.0)),
                        )
                        .clicked()
                    {
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
                "Saving command",
                "Waiting for durable device storage",
            ),
            Activity::ReadyToDispatch => Self::state_band(
                ui,
                Self::warning(),
                Icon::Send,
                "Command queued",
                "Waiting for the network dispatcher",
            ),
            Activity::InFlight => Self::state_band(
                ui,
                Self::warning(),
                Icon::Loader,
                "Sending command",
                "Awaiting the warehouse service",
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
                    "Check result",
                    message,
                );
                if ui
                    .add_sized(
                        [ui.available_width(), 54.0],
                        egui::Button::new("Retry exact command"),
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
            priority: 80,
            instructions: Some("Keep upright".into()),
            lease_expires_at: "preview".into(),
            source: Some(Location {
                name: Some("Receiving 01".into()),
                barcode: "RECV-01".into(),
            }),
            destination: Location {
                name: Some("A-01-03".into()),
                barcode: "A-01-03".into(),
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

impl eframe::App for RfApp {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.persist_queued_commands();

        #[cfg(target_os = "android")]
        egui::Panel::top("android_status_bar_space")
            .exact_size(28.0)
            .show(root_ui, |_| {});

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
                        match self.workflow.activity() {
                            Activity::Idle => self.idle(ui),
                            Activity::Active if self.workflow.claim().is_some() => {
                                self.active_work(ui);
                            }
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
                        self.command_state(ui);
                        self.error(ui);
                    });
            });
    }
}
