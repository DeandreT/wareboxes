use super::*;

impl WareboxesApp {
    const ICON_FONT: &'static str = "lucide";

    pub(super) fn install_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();
        let fallback_fonts = fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        fonts.font_data.insert(
            Self::ICON_FONT.to_owned(),
            egui::FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES),
        );
        let icon_family = fonts
            .families
            .entry(egui::FontFamily::Name(Self::ICON_FONT.into()))
            .or_default();
        icon_family.push(Self::ICON_FONT.to_owned());
        icon_family.extend(fallback_fonts);
        ctx.set_fonts(fonts);
    }

    pub(super) fn icon(icon: Icon) -> egui::RichText {
        egui::RichText::new(icon.unicode().to_string()).font(egui::FontId::new(
            15.0,
            egui::FontFamily::Name(Self::ICON_FONT.into()),
        ))
    }

    pub(super) fn icon_button(ui: &mut egui::Ui, icon: Icon, tooltip: &str) -> egui::Response {
        ui.add_sized([26.0, 26.0], egui::Button::new(Self::icon(icon)))
            .on_hover_text(tooltip)
    }

    pub(super) fn theme_style(light_mode: bool) -> egui::Style {
        use egui::{Color32, FontFamily, FontId, Rounding, Stroke, TextStyle};

        let mut style = egui::Style::default();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.interact_size = egui::vec2(26.0, 26.0);
        style.spacing.window_margin = egui::Margin::symmetric(8.0, 6.0);
        style.spacing.menu_margin = egui::Margin::same(6.0);
        style.animation_time = 0.16;
        style.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(17.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (
                TextStyle::Monospace,
                FontId::new(12.0, FontFamily::Monospace),
            ),
            (
                TextStyle::Button,
                FontId::new(13.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(11.0, FontFamily::Proportional),
            ),
        ]
        .into();

        let mut visuals = if light_mode {
            egui::Visuals::light()
        } else {
            egui::Visuals::dark()
        };
        let rounding = Rounding::same(4.0);

        if light_mode {
            let text = Color32::from_rgb(24, 34, 31);
            let border = Color32::from_rgb(207, 216, 212);
            let surface = Color32::from_rgb(255, 255, 255);
            let control = Color32::from_rgb(241, 245, 243);
            let accent = Color32::from_rgb(8, 122, 99);

            visuals.panel_fill = Color32::from_rgb(245, 247, 246);
            visuals.window_fill = surface;
            visuals.extreme_bg_color = surface;
            visuals.faint_bg_color = Color32::from_rgb(235, 240, 238);
            visuals.code_bg_color = control;
            visuals.window_stroke = Stroke::new(1.0_f32, border);
            visuals.hyperlink_color = accent;
            visuals.warn_fg_color = Color32::from_rgb(160, 101, 12);
            visuals.error_fg_color = Color32::from_rgb(180, 54, 48);
            visuals.selection.bg_fill = accent;
            visuals.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
            visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, text);
            visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, border);
            visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text);
            visuals.widgets.inactive.bg_fill = surface;
            visuals.widgets.inactive.weak_bg_fill = control;
            visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, border);
            visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, text);
            visuals.widgets.hovered.bg_fill = Color32::from_rgb(224, 239, 234);
            visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(224, 239, 234);
            visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, accent);
            visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, text);
            visuals.widgets.active.bg_fill = accent;
            visuals.widgets.active.weak_bg_fill = accent;
            visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, accent);
            visuals.widgets.open = visuals.widgets.active;
            visuals.text_cursor = Stroke::new(2.0_f32, accent);
            visuals.widgets.noninteractive.weak_bg_fill = surface;
            visuals.override_text_color = None;
        } else {
            let text = Color32::from_rgb(229, 235, 232);
            let border = Color32::from_rgb(53, 62, 59);
            let surface = Color32::from_rgb(25, 30, 29);
            let control = Color32::from_rgb(31, 38, 36);
            let accent = Color32::from_rgb(49, 181, 143);

            visuals.panel_fill = Color32::from_rgb(17, 21, 20);
            visuals.window_fill = surface;
            visuals.extreme_bg_color = Color32::from_rgb(12, 16, 15);
            visuals.faint_bg_color = Color32::from_rgb(29, 35, 33);
            visuals.code_bg_color = Color32::from_rgb(37, 44, 42);
            visuals.window_stroke = Stroke::new(1.0_f32, border);
            visuals.hyperlink_color = accent;
            visuals.warn_fg_color = Color32::from_rgb(232, 178, 74);
            visuals.error_fg_color = Color32::from_rgb(255, 112, 101);
            visuals.selection.bg_fill = Color32::from_rgb(30, 112, 91);
            visuals.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
            visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, text);
            visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, border);
            visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text);
            visuals.widgets.inactive.bg_fill = Color32::from_rgb(19, 24, 23);
            visuals.widgets.inactive.weak_bg_fill = control;
            visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, border);
            visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
            visuals.widgets.hovered.bg_fill = Color32::from_rgb(42, 67, 59);
            visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(42, 67, 59);
            visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, accent);
            visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
            visuals.widgets.active.bg_fill = Color32::from_rgb(30, 80, 68);
            visuals.widgets.active.weak_bg_fill = Color32::from_rgb(30, 80, 68);
            visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, accent);
            visuals.widgets.open = visuals.widgets.active;
            visuals.text_cursor = Stroke::new(2.0_f32, accent);
            visuals.widgets.noninteractive.weak_bg_fill = surface;
        }

        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.rounding = rounding;
        }
        visuals.window_rounding = rounding;
        visuals.menu_rounding = rounding;
        visuals.striped = true;
        style.visuals = visuals;
        style
    }
}
