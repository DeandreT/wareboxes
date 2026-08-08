#![doc = include_str!("../README.md")]

mod app;
pub mod command_store;
pub mod cycle_count;
pub mod expected_receiving;
pub mod lease;
pub mod picking;
pub mod transport;
pub mod wire;
pub mod workflow;

pub use app::RfApp;

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    let data_path = android_app
        .internal_data_path()
        .map(|path| path.join("wareboxes-rf.sqlite3"));
    let options = eframe::NativeOptions {
        android_app: Some(android_app),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Wareboxes RF",
        options,
        Box::new(move |creation_context| {
            let app = match &data_path {
                Some(path) => RfApp::new_persistent(creation_context, path),
                None => RfApp::new_without_storage(
                    creation_context,
                    "Android internal storage is unavailable",
                ),
            };
            Ok(Box::new(app))
        }),
    );

    if let Err(error) = result {
        log::error!("failed to run Wareboxes RF: {error}");
    }
}
