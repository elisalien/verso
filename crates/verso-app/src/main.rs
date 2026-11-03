#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod control_ui;
mod diagnostics;
mod dialogs;
mod effects_ui;
mod grid;
mod history;
mod media;
mod online_ui;
mod outputs;
mod panels;
mod prefs;
mod scripts_ui;
mod theme;
mod widgets;

fn main() -> eframe::Result {
    diagnostics::install_panic_hook();
    diagnostics::log(concat!("Verso ", env!("CARGO_PKG_VERSION"), " starting"));
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Verso")
            .with_inner_size([1400.0, 820.0])
            .with_min_inner_size([900.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Verso",
        options,
        Box::new(|cc| Ok(Box::new(app::VersoApp::new(cc)?))),
    )
}
