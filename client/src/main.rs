//! SyncTube desktop client.
#![windows_subsystem = "windows"]

use client::gui::App;
use client::strings;

fn main() -> eframe::Result {
    let fake_audio = std::env::args().any(|a| a == "--fake-audio");
    client::logging::init(false);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), fake_audio, "starting");

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(strings::APP_TITLE)
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([720.0, 520.0])
            .with_icon(std::sync::Arc::new(client::icons::app_icon(64))),
        ..Default::default()
    };
    let result = eframe::run_native(
        strings::APP_TITLE,
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, fake_audio)))),
    );
    if let Err(e) = &result {
        tracing::error!("ui failed: {e}");
    }
    result
}
