mod api;
mod app;

use app::WareboxesApp;

fn main() -> eframe::Result<()> {
    env_logger::init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Wareboxes WMS",
        options,
        Box::new(|cc| Ok(Box::new(WareboxesApp::new(cc)))),
    )
}
