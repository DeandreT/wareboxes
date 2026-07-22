use super::*;

impl WareboxesApp {
    pub(super) fn active_workspace(&self) -> &PanelWorkspace {
        self.workspaces
            .iter()
            .find(|workspace| workspace.id == self.active_workspace_id)
            .unwrap_or(&self.workspaces[0])
    }

    pub(super) fn active_workspace_mut(&mut self) -> &mut PanelWorkspace {
        let active_id = self.active_workspace_id;
        let index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == active_id)
            .unwrap_or(0);
        &mut self.workspaces[index]
    }

    pub(super) fn panel(&mut self, screen: Screen) -> &mut PanelState {
        self.active_workspace_mut()
            .panels
            .entry(screen)
            .or_default()
    }

    pub(super) fn set_panel_open(&mut self, screen: Screen, open: bool) {
        let was_open = self
            .active_workspace()
            .panels
            .get(&screen)
            .is_some_and(|panel| panel.open);
        self.panel(screen).open = open;
        if open && !was_open {
            self.fetch(screen);
        }
    }

    pub(super) fn create_workspace(&mut self) {
        let id = self.next_workspace_id;
        self.next_workspace_id += 1;
        self.workspaces.push(PanelWorkspace::new(
            id,
            format!("Workspace {}", self.workspaces.len() + 1),
        ));
        self.active_workspace_id = id;
    }

    pub(super) fn duplicate_active_workspace(&mut self) {
        let mut duplicate = self.active_workspace().clone();
        let id = self.next_workspace_id;
        self.next_workspace_id += 1;
        duplicate.id = id;
        duplicate.name = format!("{} copy", duplicate.name);
        for panel in duplicate.panels.values_mut() {
            panel.detached = false;
        }
        self.workspaces.push(duplicate);
        self.active_workspace_id = id;
    }

    pub(super) fn open_workspace_editor(&mut self) {
        self.workspace_name_draft = self.active_workspace().name.clone();
        self.workspace_editor_open = true;
    }

    pub(super) fn arrange_active_workspace(&mut self, layout: WorkspaceLayout) {
        let workspace = self.active_workspace_mut();
        workspace.layout = layout;
        workspace.layout_generation = workspace.layout_generation.saturating_add(1);
        for panel in workspace.panels.values_mut() {
            panel.detached = false;
        }
    }

    fn delete_workspace(&mut self, id: u64) {
        if self.workspaces.len() <= 1 {
            return;
        }
        let Some(index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == id)
        else {
            return;
        };
        self.workspaces.remove(index);
        if self.active_workspace_id == id {
            let next_index = index.min(self.workspaces.len() - 1);
            self.active_workspace_id = self.workspaces[next_index].id;
        }
    }

    pub(super) fn workspace_editor(&mut self, ctx: &egui::Context) {
        if !self.workspace_editor_open {
            return;
        }

        let mut open = self.workspace_editor_open;
        let mut save = false;
        let mut request_delete = false;
        egui::Window::new("Workspace settings")
            .id(egui::Id::new("workspace_editor"))
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.strong("Name");
                let response = ui.add_sized(
                    [ui.available_width(), 34.0],
                    egui::TextEdit::singleline(&mut self.workspace_name_draft).char_limit(32),
                );
                let submitted =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() || submitted {
                        save = true;
                    }
                    if self.workspaces.len() > 1
                        && ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Delete workspace")
                                        .color(Self::danger_text_color(ui)),
                                )
                                .stroke(egui::Stroke::new(1.0_f32, Self::danger_text_color(ui))),
                            )
                            .clicked()
                    {
                        request_delete = true;
                    }
                });
            });

        if save {
            let name = self.workspace_name_draft.trim();
            if !name.is_empty() {
                self.active_workspace_mut().name = name.to_owned();
                open = false;
            }
        }
        if request_delete {
            self.pending_workspace_delete = Some(self.active_workspace_id);
            open = false;
        }
        self.workspace_editor_open = open;
    }

    pub(super) fn workspace_delete_confirmation(&mut self, ctx: &egui::Context) {
        let Some(workspace_id) = self.pending_workspace_delete else {
            return;
        };
        let name = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| "workspace".to_owned());
        let mut cancel = false;
        let mut confirm = false;
        egui::Window::new("Delete workspace")
            .id(egui::Id::new("workspace_delete_confirmation"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("Delete {name}?"));
                ui.weak("Its panel arrangement will be removed.");
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Delete").color(Self::danger_text_color(ui)),
                            )
                            .stroke(egui::Stroke::new(1.0_f32, Self::danger_text_color(ui))),
                        )
                        .clicked()
                    {
                        confirm = true;
                    }
                });
            });
        if confirm {
            self.delete_workspace(workspace_id);
            self.pending_workspace_delete = None;
        } else if cancel {
            self.pending_workspace_delete = None;
        }
    }
}
