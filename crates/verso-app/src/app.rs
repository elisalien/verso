use std::path::PathBuf;
use std::time::Instant;

use eframe::egui::{self, Color32, RichText, Sense, Stroke, TextureId, Vec2};
use eframe::{egui_wgpu, wgpu};
use verso_core::{BlendMode, Clip, Command, Composition, Generator, Source};
use verso_render::Renderer;

const CELL: Vec2 = Vec2::new(96.0, 54.0);
const LAYER_CONTROLS_WIDTH: f32 = 230.0;

pub struct VersoApp {
    comp: Composition,
    renderer: Renderer,
    render_state: egui_wgpu::RenderState,
    texture: TextureId,
    /// Output size the egui texture was registered with.
    texture_size: (u32, u32),
    start: Instant,
    path: Option<PathBuf>,
    selected: Option<(usize, usize)>,
    output_open: bool,
    status: String,
}

fn to_color32(c: [f32; 4]) -> Color32 {
    Color32::from_rgb(
        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
    )
}

fn clip_color(clip: &Clip) -> [f32; 4] {
    match clip.source {
        Source::Solid { color } | Source::Generator { color, .. } => color,
    }
}

impl VersoApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let render_state = cc
            .wgpu_render_state
            .clone()
            .ok_or("Verso needs the wgpu renderer")?;
        let comp = Composition::demo();
        let renderer = Renderer::new(&render_state.device, (comp.width, comp.height));
        let texture = render_state.renderer.write().register_native_texture(
            &render_state.device,
            renderer.output_view(),
            wgpu::FilterMode::Linear,
        );
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        Ok(Self {
            texture_size: renderer.size(),
            comp,
            renderer,
            render_state,
            texture,
            start: Instant::now(),
            path: None,
            selected: None,
            output_open: false,
            status: String::new(),
        })
    }

    fn apply(&mut self, command: Command) {
        if let Err(e) = self.comp.apply(command) {
            self.status = e.to_string();
        }
    }

    fn render(&mut self) {
        let device = &self.render_state.device;
        let queue = &self.render_state.queue;
        let mut encoder = device.create_command_encoder(&Default::default());
        let time = self.start.elapsed().as_secs_f32();
        self.renderer
            .render(device, queue, &mut encoder, &self.comp, time);
        queue.submit([encoder.finish()]);

        // The output texture is recreated when the composition size changes.
        if self.renderer.size() != self.texture_size {
            self.texture_size = self.renderer.size();
            self.render_state
                .renderer
                .write()
                .update_egui_texture_from_wgpu_texture(
                    device,
                    self.renderer.output_view(),
                    wgpu::FilterMode::Linear,
                    self.texture,
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
        let mut commands = Vec::new();
        ctx.input(|i| {
            for (column, key) in keys.iter().enumerate() {
                if i.key_pressed(*key) && column < self.comp.columns.len() {
                    commands.push(Command::TriggerColumn(column));
                }
            }
            if i.key_pressed(egui::Key::Escape) {
                commands.push(Command::ClearAll);
            }
            if i.key_pressed(egui::Key::F2) {
                self.output_open = !self.output_open;
            }
        });
        for command in commands {
            self.apply(command);
        }
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
            Ok(comp) => {
                self.comp = comp;
                self.selected = None;
                self.status = format!("Opened {}", path.display());
                self.path = Some(path);
            }
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New").clicked() {
                    self.comp = Composition::new("Untitled", 3, 8);
                    self.path = None;
                    self.selected = None;
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
            });
            ui.menu_button("Composition", |ui| {
                if ui.button("Add Layer").clicked() {
                    self.apply(Command::AddLayer);
                }
                if ui.button("Add Column").clicked() {
                    self.apply(Command::AddColumn);
                }
                if ui.button("Clear All (Esc)").clicked() {
                    self.apply(Command::ClearAll);
                }
            });
            ui.menu_button("Output", |ui| {
                ui.checkbox(&mut self.output_open, "Output Window (F2)");
            });
            ui.separator();
            ui.label(RichText::new(&self.comp.name).strong());
            ui.label(format!("{}×{}", self.comp.width, self.comp.height));
        });
    }

    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Preview");
        let (w, h) = self.renderer.size();
        let width = ui.available_width();
        let size = Vec2::new(width, width * h as f32 / w as f32);
        ui.add(egui::Image::new((self.texture, size)));

        ui.add_space(8.0);
        ui.label("Master");
        let mut opacity = self.comp.master.opacity;
        if ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0)).changed() {
            self.apply(Command::SetMasterOpacity(opacity));
        }
        ui.add_space(8.0);
        ui.label(format!("BPM {:.1}", self.comp.bpm));
        ui.label(format!(
            "{:.0} fps",
            1.0 / ui.input(|i| i.stable_dt).max(1e-3)
        ));
    }

    fn clip_inspector(&mut self, ui: &mut egui::Ui) {
        let Some((layer, column)) = self.selected else {
            ui.label("Select a clip to edit it. Right-click a cell to change its source.");
            return;
        };
        let Some(Some(mut clip)) = self
            .comp
            .layers
            .get(layer)
            .and_then(|l| l.clips.get(column))
            .cloned()
        else {
            ui.label("Empty cell.");
            return;
        };
        let before = clip.clone();
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} / {}",
                self.comp.layers[layer].name, self.comp.columns[column].name
            ));
            ui.text_edit_singleline(&mut clip.name);
            match &mut clip.source {
                Source::Solid { color } => {
                    ui.label("Color");
                    ui.color_edit_button_rgba_unmultiplied(color);
                }
                Source::Generator { kind, color, speed } => {
                    egui::ComboBox::from_id_salt("generator")
                        .selected_text(kind.name())
                        .show_ui(ui, |ui| {
                            for g in Generator::ALL {
                                ui.selectable_value(kind, g, g.name());
                            }
                        });
                    ui.label("Color");
                    ui.color_edit_button_rgba_unmultiplied(color);
                    ui.label("Speed");
                    ui.add(egui::Slider::new(speed, 0.0..=4.0));
                }
            }
        });
        if clip != before {
            self.apply(Command::SetClip {
                layer,
                column,
                clip: Some(clip),
            });
        }
    }

    fn cell_menu(&mut self, ui: &mut egui::Ui, layer: usize, column: usize) {
        let color = self.comp.layers[layer].clips[column]
            .as_ref()
            .map(clip_color)
            .unwrap_or([1.0; 4]);
        for g in Generator::ALL {
            if ui.button(g.name()).clicked() {
                self.apply(Command::SetClip {
                    layer,
                    column,
                    clip: Some(Clip {
                        name: g.name().to_owned(),
                        source: Source::Generator {
                            kind: g,
                            color,
                            speed: 1.0,
                        },
                    }),
                });
                self.selected = Some((layer, column));
            }
        }
        if ui.button("Solid Color").clicked() {
            self.apply(Command::SetClip {
                layer,
                column,
                clip: Some(Clip {
                    name: "Solid".to_owned(),
                    source: Source::Solid { color },
                }),
            });
            self.selected = Some((layer, column));
        }
        ui.separator();
        if ui.button("Clear").clicked() {
            self.apply(Command::SetClip {
                layer,
                column,
                clip: None,
            });
        }
    }

    fn layer_controls(&mut self, ui: &mut egui::Ui, index: usize) {
        let layer = &self.comp.layers[index];
        let (mut opacity, mut blend, mut bypass) = (layer.opacity, layer.blend, layer.bypass);
        let name = layer.name.clone();
        ui.allocate_ui(Vec2::new(LAYER_CONTROLS_WIDTH, CELL.y), |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.toggle_value(&mut bypass, "B").on_hover_text("Bypass");
                    if ui.small_button("✖").on_hover_text("Eject").clicked() {
                        self.apply(Command::EjectLayer(index));
                    }
                    ui.label(RichText::new(name).strong());
                });
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt(("blend", index))
                        .width(84.0)
                        .selected_text(blend.name())
                        .show_ui(ui, |ui| {
                            for b in BlendMode::ALL {
                                ui.selectable_value(&mut blend, b, b.name());
                            }
                        });
                    ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0).show_value(false));
                });
            });
        });
        let before = &self.comp.layers[index];
        let (old_opacity, old_blend, old_bypass) = (before.opacity, before.blend, before.bypass);
        if opacity != old_opacity {
            self.apply(Command::SetLayerOpacity {
                layer: index,
                opacity,
            });
        }
        if blend != old_blend {
            self.apply(Command::SetLayerBlend {
                layer: index,
                blend,
            });
        }
        if bypass != old_bypass {
            self.apply(Command::SetLayerBypass {
                layer: index,
                bypass,
            });
        }
    }

    fn cell(&mut self, ui: &mut egui::Ui, layer: usize, column: usize) {
        let clip = self.comp.layers[layer].clips[column].clone();
        let active = self.comp.layers[layer].active == Some(column);
        let selected = self.selected == Some((layer, column));
        let (rect, response) = ui.allocate_exact_size(CELL, Sense::click());

        let painter = ui.painter_at(rect);
        let fill = match &clip {
            Some(clip) => {
                to_color32(clip_color(clip)).gamma_multiply(if active { 0.9 } else { 0.35 })
            }
            None => ui.visuals().extreme_bg_color,
        };
        painter.rect_filled(rect, 4.0, fill);
        let stroke = if active {
            Stroke::new(2.0, Color32::WHITE)
        } else if selected {
            Stroke::new(2.0, ui.visuals().selection.stroke.color)
        } else {
            Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
        };
        painter.rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);
        if let Some(clip) = &clip {
            painter.text(
                rect.left_bottom() + Vec2::new(6.0, -4.0),
                egui::Align2::LEFT_BOTTOM,
                &clip.name,
                egui::FontId::proportional(12.0),
                Color32::WHITE,
            );
        }

        if response.clicked() {
            self.selected = Some((layer, column));
            self.apply(Command::TriggerClip { layer, column });
        }
        response.context_menu(|ui| self.cell_menu(ui, layer, column));
    }

    fn grid(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::both().show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(4.0);
            // Column headers: trigger every layer at once.
            ui.horizontal(|ui| {
                ui.add_space(LAYER_CONTROLS_WIDTH + 4.0);
                for column in 0..self.comp.columns.len() {
                    let label = format!("▶ {}", self.comp.columns[column].name);
                    if ui
                        .add_sized([CELL.x, 22.0], egui::Button::new(label))
                        .clicked()
                    {
                        self.apply(Command::TriggerColumn(column));
                    }
                }
            });
            // Top layer is drawn first, as it renders on top.
            for layer in (0..self.comp.layers.len()).rev() {
                ui.horizontal(|ui| {
                    self.layer_controls(ui, layer);
                    for column in 0..self.comp.columns.len() {
                        self.cell(ui, layer, column);
                    }
                });
            }
        });
    }

    fn output_window(&mut self, ctx: &egui::Context) {
        if !self.output_open {
            return;
        }
        let texture = self.texture;
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
                        let response = ui.allocate_rect(rect, Sense::click());
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
        self.render();
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        "1–9 trigger column · Esc clear · F2 output · right-click cell to edit",
                    );
                });
            });
        });
        egui::Panel::bottom("inspector")
            .exact_size(56.0)
            .show(ui, |ui| self.clip_inspector(ui));
        egui::Panel::right("preview")
            .default_size(360.0)
            .resizable(true)
            .show(ui, |ui| self.preview_panel(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| self.grid(ui));

        let ctx = ui.ctx().clone();
        self.output_window(&ctx);
    }
}
