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
    use eframe::wasm_bindgen::JsCast as _;

    let web_options = eframe::WebOptions::default();
    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window()
            .expect("no window")
            .document()
            .expect("no document");
        let canvas = document
            .get_element_by_id("wareboxes_canvas")
            .expect("missing element id=wareboxes_canvas")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("not a canvas");
        eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| Box::new(WareboxesApp::new(cc))),
            )
            .await
            .expect("failed to start eframe");
    });
}
