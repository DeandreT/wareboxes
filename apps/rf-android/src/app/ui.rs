use eframe::egui;
use lucide_icons::Icon;

use super::RfApp;

const ICON_FONT: &str = "lucide";

impl RfApp {
    pub(super) fn install_style(creation_context: &eframe::CreationContext<'_>) {
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
        style.spacing.item_spacing = egui::vec2(8.0, 10.0);
        style.spacing.button_padding = egui::vec2(14.0, 11.0);
        style.spacing.interact_size = egui::vec2(48.0, 48.0);
        style.spacing.window_margin = egui::Margin::same(12);
        style.text_styles = [
            (
                egui::TextStyle::Heading,
                egui::FontId::new(23.0, egui::FontFamily::Proportional),
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
        visuals.panel_fill = egui::Color32::from_rgb(10, 14, 13);
        visuals.window_fill = egui::Color32::from_rgb(18, 24, 22);
        visuals.extreme_bg_color = egui::Color32::from_rgb(5, 8, 7);
        visuals.faint_bg_color = egui::Color32::from_rgb(24, 31, 29);
        visuals.selection.bg_fill = egui::Color32::from_rgb(18, 112, 81);
        visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(24, 31, 29);
        visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(24, 31, 29);
        visuals.widgets.inactive.fg_stroke =
            egui::Stroke::new(1.0, egui::Color32::from_rgb(205, 216, 212));
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(8);
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(8);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(8);
        visuals.widgets.open.corner_radius = egui::CornerRadius::same(8);
        style.visuals = visuals;
        style
    }

    pub(super) fn icon(icon: Icon) -> egui::RichText {
        egui::RichText::new(icon.unicode().to_string()).font(egui::FontId::new(
            19.0,
            egui::FontFamily::Name(ICON_FONT.into()),
        ))
    }

    pub(super) fn accent() -> egui::Color32 {
        egui::Color32::from_rgb(45, 190, 139)
    }

    pub(super) fn warning() -> egui::Color32 {
        egui::Color32::from_rgb(241, 179, 70)
    }

    pub(super) fn danger() -> egui::Color32 {
        egui::Color32::from_rgb(245, 104, 93)
    }

    pub(super) fn message_band(ui: &mut egui::Ui, color: egui::Color32, icon: Icon, message: &str) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(color.gamma_multiply(0.14))
            .stroke(egui::Stroke::new(1.0, color))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_min_width((width - 20.0).max(0.0));
                ui.horizontal_wrapped(|ui| {
                    ui.label(Self::icon(icon).color(color));
                    ui.label(message);
                });
            });
    }

    pub(super) fn task_reference(ui: &mut egui::Ui, reference: &str, priority: i64) {
        egui::containers::Sides::new().height(28.0).show(
            ui,
            |ui| {
                ui.label(
                    egui::RichText::new(reference)
                        .small()
                        .strong()
                        .color(Self::accent()),
                );
            },
            |ui| {
                ui.label(
                    egui::RichText::new(format!("Priority {priority}"))
                        .small()
                        .color(egui::Color32::from_rgb(166, 177, 173)),
                );
            },
        );
    }

    pub(super) fn section_label(ui: &mut egui::Ui, label: &str) {
        ui.label(
            egui::RichText::new(label)
                .size(13.0)
                .strong()
                .color(egui::Color32::from_rgb(139, 155, 149)),
        );
    }

    pub(super) fn centered_text_edit<'a>(edit: egui::TextEdit<'a>) -> egui::TextEdit<'a> {
        edit.horizontal_align(egui::Align::Center)
            .vertical_align(egui::Align::Center)
    }

    pub(super) fn centered_hint(
        ui: &egui::Ui,
        response: &egui::Response,
        visible: bool,
        hint: &str,
        text_style: egui::TextStyle,
    ) {
        if visible && ui.is_rect_visible(response.rect) {
            ui.painter().text(
                response.rect.center(),
                egui::Align2::CENTER_CENTER,
                hint,
                text_style.resolve(ui.style()),
                ui.visuals().weak_text_color(),
            );
        }
    }

    pub(super) fn full_width_button(
        ui: &mut egui::Ui,
        enabled: bool,
        button: egui::Button<'_>,
        height: f32,
    ) -> egui::Response {
        let width = ui.available_width();
        ui.add_enabled_ui(enabled, |ui| ui.add_sized([width, height], button))
            .inner
    }

    pub(super) fn scanner_action(
        ui: &mut egui::Ui,
        prompt: &str,
        expected: Option<&str>,
        confirm_label: &str,
        allowed: bool,
        draft: &mut String,
        id: egui::Id,
    ) -> (egui::Response, bool) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(Self::accent().gamma_multiply(0.08))
            .stroke(egui::Stroke::new(1.0, Self::accent()))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(0.0));
                ui.vertical_centered(|ui| {
                    Self::section_label(ui, "NEXT ACTION");
                    ui.label(
                        egui::RichText::new(prompt)
                            .size(23.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                    if let Some(expected) = expected {
                        ui.monospace(
                            egui::RichText::new(expected)
                                .size(17.0)
                                .color(Self::accent()),
                        );
                    }
                });
                let field = ui
                    .add_enabled_ui(allowed, |ui| {
                        ui.add_sized(
                            [ui.available_width(), 60.0],
                            Self::centered_text_edit(
                                egui::TextEdit::singleline(draft)
                                    .id(id)
                                    .font(egui::TextStyle::Monospace),
                            ),
                        )
                    })
                    .inner;
                Self::centered_hint(
                    ui,
                    &field,
                    draft.is_empty(),
                    "SCAN OR TYPE",
                    egui::TextStyle::Monospace,
                );
                let scan_ready = !draft.trim().is_empty();
                let can_confirm = scan_ready && allowed;
                let clicked = Self::full_width_button(
                    ui,
                    can_confirm,
                    egui::Button::new(egui::RichText::new(confirm_label).strong())
                        .fill(Self::primary_fill(can_confirm)),
                    56.0,
                )
                .on_disabled_hover_text(if allowed {
                    "A scan is required"
                } else {
                    "Check task connection first"
                })
                .clicked();
                (field, clicked)
            })
            .inner
    }

    pub(super) fn secondary_button(label: &str, width: f32, height: f32) -> egui::Button<'static> {
        egui::Button::new(egui::RichText::new(label.to_owned()))
            .fill(egui::Color32::from_rgb(28, 34, 32))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(79, 91, 87)))
            .min_size(egui::vec2(width, height))
    }

    pub(super) fn primary_fill(enabled: bool) -> egui::Color32 {
        if enabled {
            egui::Color32::from_rgb(13, 128, 91)
        } else {
            egui::Color32::from_rgb(24, 31, 29)
        }
    }

    pub(super) fn state_band(
        ui: &mut egui::Ui,
        color: egui::Color32,
        icon: Icon,
        title: &str,
        detail: &str,
    ) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(color.gamma_multiply(0.16))
            .stroke(egui::Stroke::new(1.0, color))
            .corner_radius(egui::CornerRadius::same(8))
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
}
