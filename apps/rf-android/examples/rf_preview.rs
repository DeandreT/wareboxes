use wareboxes_rf_android::RfApp;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([480.0, 760.0])
            .with_min_inner_size([360.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Wareboxes RF Preview",
        options,
        Box::new(|creation_context| Ok(Box::new(RfApp::new(creation_context)))),
    )
}
