use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use eframe::egui::{self, Color32, TextureId};
use eframe::{egui_wgpu, wgpu};
use verso_core::{Command, Composition};
use verso_render::{Renderer, Thumbnails};

use crate::media::Media;
use crate::theme;

/// Size of the live preview rendered for each clip cell.
const CELL_THUMB: (u32, u32) = (192, 108);
/// Size of the clip preview monitor.
const MONITOR: (u32, u32) = (640, 360);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropsTab {
    Composition,
    Group,
    Layer,
    Clip,
}

/// Folders scanned for effects at startup: `effects` next to the executable
/// and in the working directory.
fn effect_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("effects")];
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("effects")))
    {
        dirs.push(exe_dir);
    }
    dirs
}

pub struct VersoApp {
    pub(crate) comp: Composition,
    pub(crate) renderer: Renderer,
    cell_thumbs: Thumbnails<(usize, usize)>,
    clip_monitor: Thumbnails<()>,
    render_state: egui_wgpu::RenderState,
    pub(crate) output_texture: TextureId,
    /// Output size the egui texture was registered with.
    output_texture_size: (u32, u32),
    pub(crate) cell_textures: HashMap<(usize, usize), TextureId>,
    /// Screen rectangle of every clip cell drawn this frame, for file drops.
    pub(crate) cell_rects: Vec<(egui::Rect, (usize, usize))>,
    pub(crate) monitor_texture: Option<TextureId>,
    pub(crate) media: Media,
    /// Layers whose clip was launched again since the last media update.
    restarted: Vec<usize>,
    pub(crate) files: crate::panels::FileBrowser,
    start: Instant,
    last_frame: Instant,
    path: Option<PathBuf>,
    pub(crate) selected_clip: Option<(usize, usize)>,
    pub(crate) selected_layer: usize,
    pub(crate) props_tab: PropsTab,
    pub(crate) output_open: bool,
    pub(crate) status: String,
    /// Commands collected while drawing, applied at the end of the frame.
    pending: Vec<Command>,
}

impl VersoApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let render_state = cc
            .wgpu_render_state
            .clone()
            .ok_or("Verso needs the wgpu renderer")?;
        let device = &render_state.device;
        let comp = Composition::demo();
        let mut renderer = Renderer::new(device, (comp.width, comp.height));
        let effect_errors: Vec<String> = effect_dirs()
            .iter()
            .flat_map(|dir| renderer.library_mut().load_dir(dir))
            .collect();
        let output_texture = render_state.renderer.write().register_native_texture(
            device,
            renderer.output_view(),
            wgpu::FilterMode::Linear,
        );
        theme::apply(&cc.egui_ctx);
        Ok(Self {
            output_texture_size: renderer.size(),
            cell_thumbs: Thumbnails::new(device, CELL_THUMB),
            clip_monitor: Thumbnails::new(device, MONITOR),
            comp,
            renderer,
            render_state,
            output_texture,
            cell_textures: HashMap::new(),
            cell_rects: Vec::new(),
            monitor_texture: None,
            media: Media::new(),
            restarted: Vec::new(),
            files: Default::default(),
            start: Instant::now(),
            last_frame: Instant::now(),
            path: None,
            selected_clip: None,
            selected_layer: 0,
            props_tab: PropsTab::Composition,
            output_open: false,
            status: effect_errors.first().cloned().unwrap_or_default(),
            pending: Vec::new(),
        })
    }

    /// Queues a change; it applies at the end of the frame.
    pub(crate) fn send(&mut self, command: Command) {
        self.pending.push(command);
    }

    fn apply_pending(&mut self) {
        for command in std::mem::take(&mut self.pending) {
            // Launching a clip again restarts it from the beginning.
            match &command {
                Command::TriggerClip { layer, column } => {
                    if self.comp.layers.get(*layer).and_then(|l| l.active) == Some(*column) {
                        self.restarted.push(*layer);
                    }
                }
                Command::TriggerColumn(column) => {
                    for (index, layer) in self.comp.layers.iter().enumerate() {
                        if layer.active == Some(*column) {
                            self.restarted.push(index);
                        }
                    }
                }
                _ => {}
            }
            if let Err(e) = self.comp.apply(command) {
                self.status = e.to_string();
            }
        }
        // Keep selections valid after layers or columns change.
        self.selected_layer = self
            .selected_layer
            .min(self.comp.layers.len().saturating_sub(1));
        if let Some((l, c)) = self.selected_clip
            && self
                .comp
                .layers
                .get(l)
                .and_then(|l| l.clips.get(c))
                .is_none()
        {
            self.selected_clip = None;
        }
    }

    fn render(&mut self) {
        let device = &self.render_state.device;
        let queue = &self.render_state.queue;
        let mut encoder = device.create_command_encoder(&Default::default());
        let time = self.start.elapsed().as_secs_f32();
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f64();
        self.last_frame = now;

        let errors = self.media.update(
            device,
            queue,
            &mut self.render_state.renderer.write(),
            &self.comp,
            &std::mem::take(&mut self.restarted),
            dt,
        );
        if let Some(error) = errors.last() {
            self.status = error.clone();
        }
        let views = self.media.layer_views(&self.comp);
        self.renderer
            .render(device, queue, &mut encoder, &self.comp, &views, time);
        drop(views);

        let mut new_cells = Vec::new();
        for (l, layer) in self.comp.layers.iter().enumerate() {
            for (c, clip) in layer.clips.iter().enumerate() {
                if let Some(clip) = clip
                    && self.cell_thumbs.render(
                        device,
                        queue,
                        &mut encoder,
                        (l, c),
                        &clip.source,
                        time,
                    )
                {
                    new_cells.push((l, c));
                }
            }
        }
        let selected_source = self
            .selected_clip
            .and_then(|(l, c)| self.comp.layers.get(l)?.clips.get(c)?.as_ref())
            .map(|clip| clip.source.clone());
        let new_monitor = selected_source.is_some_and(|source| {
            self.clip_monitor
                .render(device, queue, &mut encoder, (), &source, time)
        });
        queue.submit([encoder.finish()]);

        let mut egui_renderer = self.render_state.renderer.write();
        for key in new_cells {
            let view = self.cell_thumbs.view(&key).expect("just rendered");
            let id = egui_renderer.register_native_texture(device, view, wgpu::FilterMode::Linear);
            self.cell_textures.insert(key, id);
        }
        if new_monitor {
            let view = self.clip_monitor.view(&()).expect("just rendered");
            self.monitor_texture =
                Some(egui_renderer.register_native_texture(device, view, wgpu::FilterMode::Linear));
        }
        // The output texture is recreated when the composition size changes.
        if self.renderer.size() != self.output_texture_size {
            self.output_texture_size = self.renderer.size();
            egui_renderer.update_egui_texture_from_wgpu_texture(
                device,
                self.renderer.output_view(),
                wgpu::FilterMode::Linear,
                self.output_texture,
            );
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let keys = [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ];
        let columns = self.comp.columns.len();
        let (mut commands, mut toggle_output) = (Vec::new(), false);
        ctx.input(|i| {
            for (column, key) in keys.iter().enumerate() {
                if i.key_pressed(*key) && column < columns {
                    commands.push(Command::TriggerColumn(column));
                }
            }
            if i.key_pressed(egui::Key::Escape) {
                commands.push(Command::ClearAll);
            }
            toggle_output = i.key_pressed(egui::Key::F2);
        });
        self.pending.extend(commands);
        self.output_open ^= toggle_output;
    }

    fn save(&mut self, save_as: bool) {
        let path = match (&self.path, save_as) {
            (Some(path), false) => Some(path.clone()),
            _ => rfd::FileDialog::new()
                .add_filter("Verso composition", &["verso"])
                .set_file_name(format!("{}.verso", self.comp.name))
                .save_file(),
        };
        let Some(path) = path else { return };
        let result = self
            .comp
            .to_ron()
            .map_err(|e| e.to_string())
            .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
        self.status = match result {
            Ok(()) => format!("Saved {}", path.display()),
            Err(e) => format!("Save failed: {e}"),
        };
        self.path = Some(path);
    }

    fn open(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Verso composition", &["verso"])
            .pick_file()
        else {
            return;
        };
        let result = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| Composition::from_ron(&text).map_err(|e| e.to_string()));
        match result {
            Ok(comp) => self.replace_composition(comp, Some(path)),
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }

    fn replace_composition(&mut self, comp: Composition, path: Option<PathBuf>) {
        self.status = match &path {
            Some(path) => format!("Opened {}", path.display()),
            None => String::new(),
        };
        self.comp = comp;
        self.path = path;
        self.selected_clip = None;
        self.selected_layer = 0;
        self.pending.clear();
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.label(egui::RichText::new("VERSO").strong().color(theme::ACCENT));
            ui.add_space(6.0);
            ui.menu_button("Composition", |ui| {
                if ui.button("New").clicked() {
                    self.replace_composition(Composition::new("Untitled", 3, 8), None);
                }
                if ui.button("Open…").clicked() {
                    self.open();
                }
                if ui.button("Save").clicked() {
                    self.save(false);
                }
                if ui.button("Save As…").clicked() {
                    self.save(true);
                }
                ui.separator();
                if ui.button("Clear All Layers (Esc)").clicked() {
                    self.send(Command::ClearAll);
                }
            });
            ui.menu_button("Layer", |ui| {
                if ui.button("New Layer").clicked() {
                    self.send(Command::AddLayer);
                }
                ui.separator();
                let layer = self.selected_layer;
                if ui.button("Group Selected Layer").clicked() {
                    self.send(Command::AddGroup {
                        layers: vec![layer],
                    });
                    self.props_tab = PropsTab::Group;
                }
                let grouped = self
                    .comp
                    .layers
                    .get(layer)
                    .is_some_and(|l| l.group.is_some());
                if ui
                    .add_enabled(
                        grouped,
                        egui::Button::new("Remove Selected Layer from Group"),
                    )
                    .clicked()
                {
                    self.send(Command::SetLayerGroup { layer, group: None });
                }
            });
            ui.menu_button("Column", |ui| {
                if ui.button("New Column").clicked() {
                    self.send(Command::AddColumn);
                }
            });
            ui.menu_button("Output", |ui| {
                ui.checkbox(&mut self.output_open, "Output Window (F2)");
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let fps = 1.0 / ui.input(|i| i.stable_dt).max(1e-3);
                ui.label(
                    egui::RichText::new(format!("{fps:.0} FPS"))
                        .color(theme::TEXT_DIM)
                        .monospace(),
                );
                ui.separator();
                let mut bpm = self.comp.bpm;
                if ui
                    .add(
                        egui::DragValue::new(&mut bpm)
                            .range(20.0..=500.0)
                            .speed(0.1)
                            .suffix(" BPM"),
                    )
                    .changed()
                {
                    self.send(Command::SetBpm(bpm));
                }
                ui.separator();
                ui.label(egui::RichText::new(&self.comp.name).color(theme::TEXT_DIM));
            });
        });
    }

    fn output_window(&mut self, ctx: &egui::Context) {
        if !self.output_open {
            return;
        }
        let texture = self.output_texture;
        let mut close = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("verso-output"),
            egui::ViewportBuilder::default()
                .with_title("Verso Output")
                .with_inner_size([960.0, 540.0]),
            |ui, _class| {
                egui::CentralPanel::no_frame()
                    .frame(egui::Frame::NONE.fill(Color32::BLACK))
                    .show(ui, |ui| {
                        let rect = ui.max_rect();
                        let response = ui.allocate_rect(rect, egui::Sense::click());
                        ui.painter().image(
                            texture,
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        if response.double_clicked() {
                            let fullscreen = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
                            ui.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                        }
                    });
                if ui.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
            },
        );
        if close {
            self.output_open = false;
        }
    }
}

impl eframe::App for VersoApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_keys(ctx);
        self.apply_pending();
        self.render();
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let bar = egui::Frame::NONE
            .fill(theme::BG_HEADER)
            .inner_margin(egui::Margin::symmetric(6, 3));
        egui::Panel::top("menu")
            .frame(bar)
            .show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").frame(bar).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(
                            "1–9 column · Esc clear · F2 output · drag sources onto slots · right-click slot",
                        )
                        .color(theme::TEXT_DIM),
                    );
                });
            });
        });
        egui::Panel::bottom("lower")
            .resizable(true)
            .default_size(300.0)
            .min_size(180.0)
            .frame(egui::Frame::NONE.fill(theme::BG_DEEP))
            .show(ui, |ui| {
                egui::Panel::left("properties")
                    .resizable(true)
                    .default_size(420.0)
                    .show(ui, |ui| self.properties(ui));
                egui::Panel::right("browser")
                    .resizable(true)
                    .default_size(230.0)
                    .show(ui, |ui| self.browser(ui));
                egui::CentralPanel::default_margins().show(ui, |ui| self.monitors(ui));
            });
        egui::Panel::bottom("crossfader")
            .frame(bar)
            .show(ui, |ui| self.crossfader(ui));
        egui::CentralPanel::default_margins()
            .frame(egui::Frame::NONE.fill(theme::BG_DEEP).inner_margin(6))
            .show(ui, |ui| self.grid(ui));

        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);
        self.output_window(&ctx);
    }
}
