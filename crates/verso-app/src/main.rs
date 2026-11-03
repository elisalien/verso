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
    let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup {
        // Ask for BC textures when the GPU has them: HAP video then goes to
        // the GPU without decoding.
        let base = setup.device_descriptor.clone();
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut descriptor = base(adapter);
            descriptor.required_features |=
                adapter.features() & eframe::wgpu::Features::TEXTURE_COMPRESSION_BC;
            descriptor
        });
    }
    let options = eframe::NativeOptions {
        wgpu_options,
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
