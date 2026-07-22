mod api;
mod app;

use app::WareboxesApp;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    env_logger::init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Wareboxes WMS",
        options,
        Box::new(|cc| Box::new(WareboxesApp::new(cc))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    let web_options = eframe::WebOptions::default();
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        let Some(canvas) = document.get_element_by_id("wareboxes_canvas") else {
            return;
        };

        let started = eframe::WebRunner::new()
            .start(
                "wareboxes_canvas",
                web_options,
                Box::new(|cc| Box::new(WareboxesApp::new(cc))),
            )
            .await;

        if started.is_ok() {
            let _ = canvas.set_attribute("data-wareboxes-ready", "true");
        } else if let Some(status) = document.get_element_by_id("loader_status") {
            status.set_text_content(Some("The Wareboxes client could not start."));
        }
    });
}
