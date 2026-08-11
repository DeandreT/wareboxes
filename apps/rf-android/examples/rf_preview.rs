use wareboxes_rf_android::RfApp;

fn main() -> eframe::Result<()> {
    let width = preview_dimension("WAREBOXES_RF_PREVIEW_WIDTH", 480.0, 360.0);
    let height = preview_dimension("WAREBOXES_RF_PREVIEW_HEIGHT", 760.0, 640.0);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([width, height])
            .with_min_inner_size([360.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Wareboxes RF Preview",
        options,
        Box::new(|creation_context| Ok(Box::new(RfApp::new(creation_context)))),
    )
}

fn preview_dimension(name: &str, default: f32, minimum: f32) -> f32 {
    let value = std::env::var(name).ok();
    parse_preview_dimension(value.as_deref(), default, minimum)
}

fn parse_preview_dimension(value: Option<&str>, default: f32, minimum: f32) -> f32 {
    value
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value >= minimum)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::parse_preview_dimension;

    #[test]
    fn preview_dimension_accepts_supported_handheld_sizes() {
        assert_eq!(parse_preview_dimension(Some("360"), 480.0, 360.0), 360.0);
        assert_eq!(parse_preview_dimension(Some("480.5"), 480.0, 360.0), 480.5);
    }

    #[test]
    fn preview_dimension_rejects_small_or_invalid_values() {
        for value in [None, Some("359"), Some("NaN"), Some("invalid")] {
            assert_eq!(parse_preview_dimension(value, 480.0, 360.0), 480.0);
        }
    }
}
