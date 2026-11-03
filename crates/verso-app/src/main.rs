#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod advanced_output_ui;
mod ao_editor;
mod app;
mod control_ui;
mod cues_ui;
mod dialogs;
mod dock;
mod effects_ui;
mod grid;
mod groups_ui;
mod history;
mod hover_preview;
mod logging;
mod media;
mod milkdrop_settings;
mod milkdrop_ui;
mod online_ui;
mod outputs;
mod panels;
mod prefs;
mod scripts_ui;
mod source_browser;
mod theme;
mod title_bar;
mod widgets;
mod workspace;

fn main() -> eframe::Result {
    logging::init();
    let window = title_bar::WindowSettings::load();
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
            .with_min_inner_size([900.0, 500.0])
            .with_decorations(!window.custom_title_bar),
        ..Default::default()
    };
    eframe::run_native(
        "Verso",
        options,
        Box::new(|cc| Ok(Box::new(app::VersoApp::new(cc)?))),
    )
}
