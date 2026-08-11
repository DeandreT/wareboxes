use eframe::egui;
use lucide_icons::Icon;

use crate::expected_receiving::ReceivingActivity;
use crate::workflow::{Activity, MovementOperation};

use super::RfApp;
use super::session::ReceivingCommandPhase;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum WorkMode {
    Receive,
    Putaway,
    Pick,
    Relocate,
    Replenish,
    OutboundLoad,
    Count,
}

impl WorkMode {
    const ALL: [Self; 7] = [
        Self::Receive,
        Self::Putaway,
        Self::Pick,
        Self::Relocate,
        Self::Replenish,
        Self::OutboundLoad,
        Self::Count,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Receive => "Receive",
            Self::Putaway => "Putaway",
            Self::Pick => "Pick",
            Self::Relocate => "Relocate",
            Self::Replenish => "Replenish",
            Self::OutboundLoad => "Load",
            Self::Count => "Count",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::Receive => "Unload and receive stock",
            Self::Putaway => "Move inbound stock to storage",
            Self::Pick => "Pick released order demand",
            Self::Relocate => "Move stock between locations",
            Self::Replenish => "Refill forward pick locations",
            Self::OutboundLoad => "Load and unload cartons",
            Self::Count => "Perform directed counts",
        }
    }

    const fn icon(self) -> Icon {
        match self {
            Self::Receive => Icon::PackagePlus,
            Self::Putaway => Icon::PackageOpen,
            Self::Pick => Icon::ScanBarcode,
            Self::Relocate => Icon::Move,
            Self::Replenish => Icon::RefreshCw,
            Self::OutboundLoad => Icon::Truck,
            Self::Count => Icon::ClipboardCheck,
        }
    }
}

impl From<MovementOperation> for WorkMode {
    fn from(operation: MovementOperation) -> Self {
        match operation {
            MovementOperation::Putaway => Self::Putaway,
            MovementOperation::InventoryRelocation => Self::Relocate,
        }
    }
}

impl RfApp {
    pub(super) fn work_header(&mut self, ui: &mut egui::Ui) {
        ui.set_min_height(60.0);
        ui.set_max_height(60.0);
        let (label, color) = self.work_status();
        let switching_allowed = self.work_switch_allowed();
        let action_width = 142.0;
        let title_width = (ui.available_width() - action_width - 8.0).max(110.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(title_width, 60.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(Self::icon(self.work_mode.icon()).color(Self::accent()));
                            ui.heading(self.work_mode.label());
                        });
                        if let Some(session) = self.session.as_ref() {
                            ui.label(
                                egui::RichText::new(&session.tenant_name)
                                    .small()
                                    .color(egui::Color32::from_rgb(166, 177, 173)),
                            );
                        }
                    });
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(action_width, 60.0),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.horizontal(|ui| {
                        egui::Frame::new()
                            .fill(color.gamma_multiply(0.14))
                            .stroke(egui::Stroke::new(1.0, color))
                            .corner_radius(egui::CornerRadius::same(10))
                            .inner_margin(egui::Margin::symmetric(9, 5))
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new(label).small().strong().color(color));
                            });
                        let menu_label = if self.work_menu_open { "Close" } else { "Work" };
                        if ui
                            .add_enabled(
                                switching_allowed,
                                egui::Button::new(egui::RichText::new(menu_label).strong())
                                    .selected(self.work_menu_open)
                                    .min_size(egui::vec2(70.0, 44.0)),
                            )
                            .on_disabled_hover_text(
                                "Finish or recover current work before switching",
                            )
                            .clicked()
                        {
                            self.work_menu_open = !self.work_menu_open;
                        }
                    });
                },
            );
        });
    }

    pub(super) fn work_switch_allowed(&self) -> bool {
        work_mode_switch_allowed(
            self.workflow.activity(),
            self.receiving.activity(),
            self.cycle_count.activity(),
            self.picking.activity(),
            self.replenishment.activity(),
            self.outbound_load.activity(),
        )
    }

    pub(super) fn work_launcher(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        ui.vertical_centered(|ui| {
            ui.heading("Choose work");
            ui.label("Select a workflow. Work switching locks when work becomes active.");
        });
        ui.add_space(8.0);

        for modes in WorkMode::ALL.chunks(2) {
            let width = (ui.available_width() - 8.0) / 2.0;
            ui.horizontal(|ui| {
                for mode in modes {
                    let selected = self.work_mode == *mode;
                    let fill = if selected {
                        Self::accent().gamma_multiply(0.22)
                    } else {
                        ui.visuals().faint_bg_color
                    };
                    let response = ui.add(
                        egui::Button::new(egui::RichText::new(mode.label()).size(18.0).strong())
                            .selected(selected)
                            .fill(fill)
                            .stroke(egui::Stroke::new(
                                1.0,
                                if selected {
                                    Self::accent()
                                } else {
                                    egui::Color32::from_rgb(65, 78, 73)
                                },
                            ))
                            .min_size(egui::vec2(width, 62.0)),
                    );
                    if response.clicked() {
                        self.select_work_mode(*mode);
                    }
                }
            });
            ui.horizontal(|ui| {
                for mode in modes {
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 30.0),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(mode.description())
                                    .small()
                                    .color(egui::Color32::from_rgb(166, 177, 173)),
                            );
                        },
                    );
                }
            });
            ui.add_space(4.0);
        }
    }

    fn select_work_mode(&mut self, mode: WorkMode) {
        if !self.work_switch_allowed() {
            self.work_menu_open = false;
            return;
        }
        self.work_mode = mode;
        match mode {
            WorkMode::Putaway => self.workflow.select_operation(MovementOperation::Putaway),
            WorkMode::Relocate => self
                .workflow
                .select_operation(MovementOperation::InventoryRelocation),
            WorkMode::Receive
            | WorkMode::Pick
            | WorkMode::Replenish
            | WorkMode::OutboundLoad
            | WorkMode::Count => {}
        }
        self.receiving_ui.clear_focus();
        self.scan_focus = None;
        self.count_scan_focus = None;
        self.pick_scan_focus = None;
        self.replenishment_scan_focus = None;
        self.outbound_load_scan_focus = None;
        self.work_menu_open = false;
    }

    fn work_status(&self) -> (&'static str, egui::Color32) {
        match self.work_mode {
            WorkMode::Putaway | WorkMode::Relocate => self
                .heartbeat_header()
                .unwrap_or_else(|| activity_status(self.workflow.activity())),
            WorkMode::Count => self
                .heartbeat_header()
                .unwrap_or_else(|| activity_status(self.cycle_count.activity())),
            WorkMode::Pick => self
                .heartbeat_header()
                .unwrap_or_else(|| activity_status(self.picking.activity())),
            WorkMode::Replenish => self
                .heartbeat_header()
                .unwrap_or_else(|| activity_status(self.replenishment.activity())),
            WorkMode::OutboundLoad => activity_status(self.outbound_load.activity()),
            WorkMode::Receive => match self.receiving.activity() {
                ReceivingActivity::AwaitingLoad | ReceivingActivity::LoadComplete => {
                    ("READY", Self::accent())
                }
                ReceivingActivity::Active => ("ACTIVE", Self::accent()),
                ReceivingActivity::ConfirmationPending => self.receiving_command.as_ref().map_or(
                    ("WORKING", Self::warning()),
                    |command| match command.phase() {
                        ReceivingCommandPhase::Ready | ReceivingCommandPhase::InFlight => {
                            ("WORKING", Self::warning())
                        }
                        ReceivingCommandPhase::Ambiguous => ("CHECK", Self::danger()),
                        ReceivingCommandPhase::ReconcileRequired => ("BLOCKED", Self::danger()),
                    },
                ),
                ReceivingActivity::ResolvingLoad | ReceivingActivity::Refreshing => {
                    ("WORKING", Self::warning())
                }
                ReceivingActivity::LoadResolutionFailed | ReceivingActivity::RefreshFailed => {
                    ("RETRY", Self::warning())
                }
                ReceivingActivity::ReconcileRequired => ("BLOCKED", Self::danger()),
            },
        }
    }
}

pub(super) fn work_mode_switch_allowed(
    putaway: Activity,
    receiving: ReceivingActivity,
    count: Activity,
    picking: Activity,
    replenishment: Activity,
    outbound_load: Activity,
) -> bool {
    putaway == Activity::Idle
        && count == Activity::Idle
        && picking == Activity::Idle
        && replenishment == Activity::Idle
        && outbound_load == Activity::Idle
        && matches!(
            receiving,
            ReceivingActivity::AwaitingLoad | ReceivingActivity::LoadComplete
        )
}

fn activity_status(activity: Activity) -> (&'static str, egui::Color32) {
    match activity {
        Activity::Idle => ("READY", RfApp::accent()),
        Activity::Active => ("ACTIVE", RfApp::accent()),
        Activity::Persisting => ("SAVING", RfApp::warning()),
        Activity::ReadyToDispatch => ("QUEUED", RfApp::warning()),
        Activity::InFlight => ("SENDING", RfApp::warning()),
        Activity::Ambiguous => ("CHECK", RfApp::danger()),
        Activity::ReconcileRequired => ("BLOCKED", RfApp::danger()),
    }
}

#[cfg(test)]
mod tests {
    use super::work_mode_switch_allowed;
    use crate::expected_receiving::ReceivingActivity;
    use crate::workflow::Activity;

    #[test]
    fn work_mode_changes_only_without_owned_work() {
        assert!(work_mode_switch_allowed(
            Activity::Idle,
            ReceivingActivity::AwaitingLoad,
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
        ));
        assert!(work_mode_switch_allowed(
            Activity::Idle,
            ReceivingActivity::LoadComplete,
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
            Activity::Idle,
        ));
        for blocked in 0..6 {
            let mut activities = [Activity::Idle; 5];
            let receiving = if blocked == 1 {
                ReceivingActivity::Active
            } else {
                ReceivingActivity::AwaitingLoad
            };
            if blocked != 1 {
                let index = if blocked == 0 { 0 } else { blocked - 1 };
                activities[index] = Activity::Active;
            }
            assert!(!work_mode_switch_allowed(
                activities[0],
                receiving,
                activities[1],
                activities[2],
                activities[3],
                activities[4],
            ));
        }
    }

    #[test]
    fn receiving_command_states_lock_work_switching() {
        for receiving in [
            ReceivingActivity::ResolvingLoad,
            ReceivingActivity::Active,
            ReceivingActivity::ConfirmationPending,
            ReceivingActivity::Refreshing,
            ReceivingActivity::LoadResolutionFailed,
            ReceivingActivity::RefreshFailed,
            ReceivingActivity::ReconcileRequired,
        ] {
            assert!(!work_mode_switch_allowed(
                Activity::Idle,
                receiving,
                Activity::Idle,
                Activity::Idle,
                Activity::Idle,
                Activity::Idle,
            ));
        }
    }
}
