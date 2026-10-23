// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod fonts;
mod project;
mod settings;
mod theme;
mod time;
mod ui;

use tracing_subscriber::EnvFilter;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Graft")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 560.0])
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Graft",
        options,
        Box::new(|cc| {
            // `graft <folder>…` opens those folders as projects.
            let folders = std::env::args_os()
                .skip(1)
                .map(std::path::PathBuf::from)
                .collect();
            Ok(Box::new(app::GraftApp::new(cc, folders)))
        }),
    )
}

fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/graft-256.png"))
        .expect("bundled icon is a valid PNG")
}
