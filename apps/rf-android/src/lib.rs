#![doc = include_str!("../README.md")]

mod app;
pub mod workflow;

pub use app::RfApp;

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    let options = eframe::NativeOptions {
        android_app: Some(android_app),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Wareboxes RF",
        options,
        Box::new(|creation_context| Ok(Box::new(RfApp::new(creation_context)))),
    );

    if let Err(error) = result {
        log::error!("failed to run Wareboxes RF: {error}");
    }
}
