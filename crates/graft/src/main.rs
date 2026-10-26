// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod fonts;
mod installed;
mod keys;
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
    // Graft runs once: a second start hands its folders to the first. After
    // an update the old Graft may still be closing; never hand over to it.
    let _instance = match single::claim(&folders, !after_update) {
        single::Start::First(instance) => instance,
        single::Start::Forwarded => return Ok(()),
    };
    let last_update = update::clean_up();
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

    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Graft")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 560.0])
            // Graft draws its own title bar (see ui::window_frame).
            .with_decorations(false)
            .with_icon(window_icon()),
        // DirectX 12 through wgpu. With OpenGL, some drivers (Intel Arc)
        // stalled for up to two seconds when the window was maximized or
        // restored.
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    configure_gpu(&mut options.wgpu_options);
    let result = eframe::run_native(
        "Graft",
        options,
        Box::new(move |cc| {
            let _ = repaint.set(cc.egui_ctx.clone());
            let app = app::GraftApp::new(cc, folders, forwarded, last_update);
            // After the app picked the theme, for the border color.
            native::style_window(cc);
            Ok(Box::new(app))
        }),
    );
    // Release builds have no console, so say why Graft could not start.
    if let Err(e) = &result {
        native::show_error("Graft could not start", &e.to_string());
    }
    result
}

/// Adjust egui's DirectX 12 setup.
fn configure_gpu(options: &mut eframe::WgpuConfiguration) {
    let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_setup else {
        return;
    };
    // A git GUI needs no dedicated GPU: on laptops with two, stay on the
    // integrated one and spare the battery.
    setup.power_preference = wgpu::PowerPreference::LowPower;
    // The shader compiler that comes with Windows. The newer one is looked
    // up by name, so a dxcompiler.dll in the working folder (a cloned
    // repository, say) or on PATH would be loaded.
    setup
        .instance_descriptor
        .backend_options
        .dx12
        .shader_compiler = wgpu::Dx12Compiler::Fxc;
    // egui asks for at most 8192 px, and a window wider than that (across
    // several monitors) would crash; ask for what the GPU can do.
    let device = Arc::clone(&setup.device_descriptor);
    setup.device_descriptor = Arc::new(move |adapter| {
        let mut descriptor = device(adapter);
        descriptor.required_limits.max_texture_dimension_2d =
            adapter.limits().max_texture_dimension_2d;
        descriptor
    });
}

fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/graft-256.png"))
        .expect("bundled icon is a valid PNG")
}
