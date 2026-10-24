// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod fonts;
mod launch;
mod native;
mod project;
mod settings;
mod single;
mod theme;
mod time;
mod ui;
mod update;

use std::sync::{Arc, OnceLock};

use tracing_subscriber::EnvFilter;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // `graft <folder>…` opens those folders as projects.
    let mut folders: Vec<std::path::PathBuf> =
        std::env::args_os().skip(1).map(Into::into).collect();
    let after_update = folders
        .first()
        .is_some_and(|a| a.as_os_str() == update::AFTER_UPDATE);
    if after_update {
        // Started by an update: let the old Graft save and close first.
        if let Some(pid) = folders.get(1).and_then(|p| p.to_str()?.parse().ok()) {
            native::wait_for_exit(pid);
        }
        folders.clear();
    }
    update::clean_up();
    // Graft runs once: a second start hands its folders to the first. After
    // an update the old Graft may still be closing; never hand over to it.
    let _instance = match single::claim(&folders, !after_update) {
        single::Start::First(instance) => instance,
        single::Start::Forwarded => return Ok(()),
    };
    // Listen before the window exists: creating it can take seconds, and a
    // second start only waits about three.
    let (forward_tx, forwarded) = crossbeam_channel::unbounded();
    let repaint = Arc::new(OnceLock::<egui::Context>::new());
    single::listen({
        let repaint = Arc::clone(&repaint);
        move |folders| {
            let _ = forward_tx.send(folders);
            if let Some(ctx) = repaint.get() {
                ctx.request_repaint();
            }
        }
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Graft")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 560.0])
            // Graft draws its own title bar (see ui::window_frame).
            .with_decorations(false)
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Graft",
        options,
        Box::new(move |cc| {
            let _ = repaint.set(cc.egui_ctx.clone());
            let app = app::GraftApp::new(cc, folders, forwarded);
            // After the app picked the theme, for the border color.
            native::style_window(cc);
            Ok(Box::new(app))
        }),
    )
}

fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/graft-256.png"))
        .expect("bundled icon is a valid PNG")
}
