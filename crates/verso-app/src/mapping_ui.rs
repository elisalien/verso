//! Output › Mapping: screens (projectors), their slices, and the editor.
//!
//! The setup belongs to the venue, not to a composition: it is kept in
//! `mapping.versomap` in Verso's user folder (reloaded at startup) and can be
//! saved to or opened from any `.versomap` file.

use std::path::PathBuf;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use eframe::{egui_wgpu, wgpu};
use verso_core::mapping::{self, EdgeBlend, MappingSetup, Mask, Screen, Slice, Warp};
use verso_render::mapping::ScreenRenderer;

use crate::app::VersoApp;
use crate::dialogs::FileRequest;
use crate::theme;

/// Seconds between saves of the setup to the user folder after a change.
const SAVE_DELAY: f64 = 1.0;

fn setup_path() -> Option<PathBuf> {
    crate::online_ui::user_dir().map(|d| d.join("mapping.versomap"))
}

/// GPU side of a screen: its renderer and egui texture.
struct ScreenGpu {
    renderer: ScreenRenderer,
    texture: Option<(egui::TextureId, (u32, u32))>,
}

/// What a drag on the editor canvases moves.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Handle {
    /// Corner of the input rectangle (0 top left … 3 bottom left) or its
    /// middle (4) to move it.
    Input(usize),
    /// Warp grid point.
    Warp(usize),
    /// Mask point.
    Mask(usize),
}

#[derive(Default)]
pub struct MappingState {
    pub setup: MappingSetup,
    /// Screen windows are open.
    pub live: bool,
    pub editor_open: bool,
    screen: usize,
    slice: usize,
    /// Clicks on the output canvas add mask points.
    editing_mask: bool,
    dragging: Option<Handle>,
    gpu: Vec<ScreenGpu>,
    /// When the setup changed, to save it shortly after.
    changed: Option<f64>,
}

impl MappingState {
    pub fn load() -> Self {
        let setup = setup_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| MappingSetup::from_ron(&text).ok())
            .unwrap_or_default();
        Self {
            setup,
            ..Default::default()
        }
    }

    fn touch(&mut self, now: f64) {
        self.changed = Some(now);
    }

    /// Saves the setup to the user folder once edits pause.
    pub fn autosave(&mut self, now: f64) {
        if self.changed.is_some_and(|at| now - at > SAVE_DELAY) {
            self.changed = None;
            self.save_default();
        }
    }

    pub fn save_default(&self) {
        let (Some(path), Ok(text)) = (setup_path(), self.setup.to_ron()) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = crate::app::write_atomic(&path, text.as_bytes());
    }

    /// Draws every open screen from the composition output. Call after the
    /// composition is rendered, before the encoder is submitted.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
    ) {
        let shown = self.live || self.editor_open;
        if !shown {
            self.gpu.clear();
            return;
        }
        self.gpu.truncate(self.setup.screens.len());
        for (index, screen) in self.setup.screens.iter().enumerate() {
            if self.gpu.len() <= index {
                self.gpu.push(ScreenGpu {
                    renderer: ScreenRenderer::new(device, queue, screen.size),
                    texture: None,
                });
            }
            if screen.enabled || (self.editor_open && index == self.screen) {
                self.gpu[index]
                    .renderer
                    .render(device, queue, encoder, screen, source);
            }
        }
    }

    /// Registers or refreshes the egui textures of the screens.
    pub fn register(&mut self, device: &wgpu::Device, egui: &mut egui_wgpu::Renderer) {
        for gpu in &mut self.gpu {
            let size = gpu.renderer.size();
            match &mut gpu.texture {
                Some((id, registered)) if *registered == size => {}
                Some((id, registered)) => {
                    egui.update_egui_texture_from_wgpu_texture(
                        device,
                        gpu.renderer.view(),
                        wgpu::FilterMode::Linear,
                        *id,
                    );
                    *registered = size;
                    let _ = id;
                }
                None => {
                    let id = egui.register_native_texture(
                        device,
                        gpu.renderer.view(),
                        wgpu::FilterMode::Linear,
                    );
                    gpu.texture = Some((id, size));
                }
            }
        }
    }

    fn texture(&self, screen: usize) -> Option<egui::TextureId> {
        self.gpu.get(screen)?.texture.map(|(id, _)| id)
    }
}

/// Fits a canvas of `aspect` (width / height) in the space left in `ui`.
fn canvas(ui: &mut egui::Ui, aspect: f32, max_height: f32) -> (Rect, egui::Response) {
    let width = ui.available_width().min(max_height * aspect);
    let size = Vec2::new(width, width / aspect.max(0.01));
    ui.allocate_exact_size(size, Sense::click_and_drag())
}

fn to_screen(rect: Rect, p: [f32; 2]) -> Pos2 {
    rect.min + Vec2::new(p[0] * rect.width(), p[1] * rect.height())
}

fn from_screen(rect: Rect, p: Pos2) -> [f32; 2] {
    [
        (p.x - rect.min.x) / rect.width().max(1.0),
        (p.y - rect.min.y) / rect.height().max(1.0),
    ]
}

fn nearest(points: &[Pos2], at: Pos2, radius: f32) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .map(|(i, p)| (i, p.distance(at)))
        .filter(|(_, d)| *d <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

fn handle(painter: &egui::Painter, at: Pos2, color: Color32) {
    painter.circle_filled(at, 4.5, color);
    painter.circle_stroke(at, 4.5, Stroke::new(1.0, Color32::BLACK));
}

impl VersoApp {
    pub(crate) fn mapping_menu(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        let screens = self.mapping.setup.screens.len();
        ui.add_enabled(screens > 0, egui::Checkbox::new(&mut self.mapping.live, "Mapping Outputs"))
            .on_hover_text("Open a window per screen of the mapping setup; move each to its projector and double-click for fullscreen")
            .on_disabled_hover_text("Add a screen in the Mapping editor first");
        if ui.button("Mapping Editor…").clicked() {
            self.mapping.editor_open = true;
            if self.mapping.setup.screens.is_empty() {
                let size = (self.comp.width, self.comp.height);
                self.mapping
                    .setup
                    .screens
                    .push(Screen::new("Screen 1", size));
                self.mapping.touch(self.start.elapsed().as_secs_f64());
            }
        }
        ui.horizontal(|ui| {
            if ui.button("Open Setup…").clicked() {
                self.file_request(FileRequest::OpenMapping);
            }
            if ui.button("Save Setup As…").clicked() {
                self.file_request(FileRequest::SaveMapping);
            }
        });
    }

    pub(crate) fn mapping_file(&mut self, request: FileRequest, path: PathBuf) {
        match request {
            FileRequest::OpenMapping => {
                match std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|t| MappingSetup::from_ron(&t).map_err(|e| e.to_string()))
                {
                    Ok(setup) => {
                        self.mapping.setup = setup;
                        self.mapping.screen = 0;
                        self.mapping.slice = 0;
                        self.mapping.save_default();
                        self.status = format!("Opened mapping {}", path.display());
                    }
                    Err(e) => self.problem(format!("Mapping: {e}")),
                }
            }
            FileRequest::SaveMapping => {
                let result = self
                    .mapping
                    .setup
                    .to_ron()
                    .map_err(|e| e.to_string())
                    .and_then(|t| {
                        crate::app::write_atomic(&path, t.as_bytes()).map_err(|e| e.to_string())
                    });
                match result {
                    Ok(()) => self.status = format!("Saved mapping {}", path.display()),
                    Err(e) => self.problem(format!("Mapping: {e}")),
                }
            }
            _ => {}
        }
    }

    /// One window per enabled screen, showing its rendered image.
    pub(crate) fn mapping_windows(&mut self, ctx: &egui::Context) {
        if !self.mapping.live {
            return;
        }
        let now = self.start.elapsed().as_secs_f64();
        for index in 0..self.mapping.setup.screens.len() {
            let screen = &self.mapping.setup.screens[index];
            if !screen.enabled {
                continue;
            }
            let Some(texture) = self.mapping.texture(index) else {
                continue;
            };
            let mut builder = egui::ViewportBuilder::default()
                .with_title(format!("Verso — {}", screen.name))
                .with_inner_size([screen.size.0 as f32 / 2.0, screen.size.1 as f32 / 2.0]);
            if let Some([x, y, w, h]) = screen.window {
                builder = builder.with_position([x, y]).with_inner_size([w, h]);
            }
            if screen.fullscreen {
                builder = builder.with_fullscreen(true);
            }
            let mut window = None;
            let mut fullscreen = screen.fullscreen;
            let mut close = false;
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of(("verso-screen", index)),
                builder,
                |ui, _class| {
                    egui::CentralPanel::no_frame()
                        .frame(egui::Frame::NONE.fill(Color32::BLACK))
                        .show(ui, |ui| {
                            let rect = ui.max_rect();
                            let response = ui.allocate_rect(rect, Sense::click());
                            ui.painter().image(
                                texture,
                                rect,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                            if response.double_clicked() {
                                fullscreen = !fullscreen;
                                ui.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fullscreen));
                            }
                        });
                    window = ui.input(|i| {
                        let outer = i.viewport().outer_rect?;
                        let inner = i.viewport().inner_rect?;
                        Some([outer.min.x, outer.min.y, inner.width(), inner.height()])
                    });
                    close = ui.input(|i| i.viewport().close_requested());
                },
            );
            let screen = &mut self.mapping.setup.screens[index];
            let moved = window.is_some() && window != screen.window && !fullscreen;
            if moved {
                screen.window = window;
            }
            let toggled = fullscreen != screen.fullscreen;
            screen.fullscreen = fullscreen;
            if close {
                screen.enabled = false;
            }
            if moved || toggled || close {
                self.mapping.touch(now);
            }
        }
    }

    pub(crate) fn mapping_editor(&mut self, ctx: &egui::Context) {
        if !self.mapping.editor_open {
            return;
        }
        let mut open = true;
        let before = self.mapping.setup.clone();
        egui::Window::new("Mapping")
            .open(&mut open)
            .default_size([980.0, 560.0])
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(220.0);
                        self.mapping_lists(ui);
                    });
                    ui.separator();
                    ui.vertical(|ui| self.mapping_canvases(ui));
                });
            });
        self.mapping.editor_open = open;
        if self.mapping.setup != before {
            self.mapping.touch(self.start.elapsed().as_secs_f64());
        }
    }

    fn mapping_lists(&mut self, ui: &mut egui::Ui) {
        let m = &mut self.mapping;
        crate::widgets::header(ui, "Screens");
        let mut remove = None;
        for (i, screen) in m.setup.screens.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut screen.enabled, "")
                    .on_hover_text("Output this screen");
                if ui.selectable_label(m.screen == i, &screen.name).clicked() {
                    m.screen = i;
                    m.slice = 0;
                }
                if ui
                    .small_button("×")
                    .on_hover_text("Remove screen")
                    .clicked()
                {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            m.setup.screens.remove(i);
            m.screen = m.screen.min(m.setup.screens.len().saturating_sub(1));
        }
        if ui.button("+ Add Screen").clicked() {
            let n = m.setup.screens.len() + 1;
            m.setup
                .screens
                .push(Screen::new(format!("Screen {n}"), (1920, 1080)));
            m.screen = n - 1;
            m.slice = 0;
        }
        let Some(screen) = m.setup.screens.get_mut(m.screen) else {
            return;
        };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut screen.name);
        });
        ui.horizontal(|ui| {
            ui.label("Size");
            ui.add(egui::DragValue::new(&mut screen.size.0).range(16..=8192));
            ui.label("×");
            ui.add(egui::DragValue::new(&mut screen.size.1).range(16..=8192));
        });

        ui.add_space(6.0);
        crate::widgets::header(ui, "Slices");
        let mut remove = None;
        for (i, slice) in screen.slices.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut slice.enabled, "");
                if ui.selectable_label(m.slice == i, &slice.name).clicked() {
                    m.slice = i;
                }
                if ui.small_button("×").on_hover_text("Remove slice").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            screen.slices.remove(i);
            m.slice = m.slice.min(screen.slices.len().saturating_sub(1));
        }
        if ui.button("+ Add Slice").clicked() {
            let n = screen.slices.len() + 1;
            screen.slices.push(Slice::full(format!("Slice {n}")));
            m.slice = n - 1;
        }
        let Some(slice) = screen.slices.get_mut(m.slice) else {
            return;
        };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut slice.name);
        });
        ui.horizontal(|ui| {
            ui.label("Opacity");
            ui.add(egui::Slider::new(&mut slice.opacity, 0.0..=1.0));
        });
        ui.horizontal(|ui| {
            ui.label("Grid");
            let (mut columns, mut rows) = (slice.warp.columns, slice.warp.rows);
            let c = ui.add(egui::DragValue::new(&mut columns).range(2..=mapping::MAX_GRID));
            ui.label("×");
            let r = ui.add(egui::DragValue::new(&mut rows).range(2..=mapping::MAX_GRID));
            if c.changed() || r.changed() {
                slice.warp = slice.warp.resized(columns, rows);
            }
        })
        .response
        .on_hover_text("2×2 is a corner pin; more points bend the image smoothly");
        ui.horizontal(|ui| {
            if ui.button("Reset Warp").clicked() {
                slice.warp = Warp::grid(mapping::Rect::FULL, slice.warp.columns, slice.warp.rows);
            }
            if ui.button("Full Input").clicked() {
                slice.input = mapping::Rect::FULL;
            }
        });

        ui.add_space(6.0);
        crate::widgets::header(ui, "Edge Blend");
        let b = &mut slice.blend;
        for (label, value) in [
            ("Left", &mut b.left),
            ("Right", &mut b.right),
            ("Top", &mut b.top),
            ("Bottom", &mut b.bottom),
        ] {
            ui.horizontal(|ui| {
                ui.add_sized([44.0, 16.0], egui::Label::new(label));
                ui.add(
                    egui::Slider::new(value, 0.0..=0.5)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
            });
        }
        ui.horizontal(|ui| {
            ui.add_sized([44.0, 16.0], egui::Label::new("Gamma"));
            ui.add(egui::Slider::new(&mut b.gamma, 0.5..=3.0));
        });
        if ui.small_button("Reset Blend").clicked() {
            *b = EdgeBlend::default();
        }

        ui.add_space(6.0);
        crate::widgets::header(ui, "Mask");
        ui.horizontal(|ui| {
            ui.toggle_value(&mut m.editing_mask, "Edit Points")
                .on_hover_text("Click the output to add points; right-click a point to remove it");
            let mut invert = slice.mask.as_ref().is_some_and(|k| k.invert);
            if ui.checkbox(&mut invert, "Invert").changed() {
                slice
                    .mask
                    .get_or_insert_with(|| Mask {
                        points: Vec::new(),
                        invert: false,
                    })
                    .invert = invert;
            }
            if ui.small_button("Clear").clicked() {
                slice.mask = None;
            }
        });
    }

    fn mapping_canvases(&mut self, ui: &mut egui::Ui) {
        let source = self.output_texture;
        let comp_aspect = self.comp.width as f32 / self.comp.height.max(1) as f32;
        let screen_texture = self.mapping.texture(self.mapping.screen);
        let m = &mut self.mapping;
        let Some(screen) = m.setup.screens.get_mut(m.screen) else {
            ui.label("Add a screen to start.");
            return;
        };
        let screen_aspect = screen.size.0 as f32 / screen.size.1.max(1) as f32;
        let selected = m.slice;
        let dragging = &mut m.dragging;
        let editing_mask = m.editing_mask;

        ui.label(
            egui::RichText::new("Input: drag the corners or the middle of the selected slice")
                .small()
                .color(theme::TEXT_DIM),
        );
        let (rect, response) = canvas(ui, comp_aspect, 220.0);
        let painter = ui.painter_at(rect);
        painter.image(
            source,
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        for (i, slice) in screen.slices.iter().enumerate() {
            let r = slice.input;
            let area = Rect::from_min_max(
                to_screen(rect, r.at(0.0, 0.0)),
                to_screen(rect, r.at(1.0, 1.0)),
            );
            let color = if i == selected {
                theme::ACCENT
            } else {
                theme::TEXT_DIM
            };
            painter.rect_stroke(area, 0.0, Stroke::new(1.5, color), egui::StrokeKind::Inside);
        }
        if let Some(slice) = screen.slices.get_mut(selected) {
            let r = slice.input;
            let points: Vec<Pos2> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.5, 0.5)]
                .iter()
                .map(|&(u, v)| to_screen(rect, r.at(u, v)))
                .collect();
            for p in &points {
                handle(&painter, *p, theme::ACCENT);
            }
            // Where the button went down: the pointer has already moved
            // when the drag is recognised.
            if response.drag_started()
                && let Some(pos) = ui.input(|i| i.pointer.press_origin())
            {
                *dragging = nearest(&points, pos, 12.0).map(Handle::Input);
            }
            if let (Some(Handle::Input(corner)), Some(pos)) =
                (*dragging, response.interact_pointer_pos())
                && response.dragged()
            {
                let [x, y] = from_screen(rect, pos);
                let r = &mut slice.input;
                let (right, bottom) = (r.x + r.w, r.y + r.h);
                match corner {
                    0 => (r.x, r.y) = (x.min(right - 0.01), y.min(bottom - 0.01)),
                    1 => (r.y, r.w) = (y.min(bottom - 0.01), (x - r.x).max(0.01)),
                    2 => (r.w, r.h) = ((x - r.x).max(0.01), (y - r.y).max(0.01)),
                    3 => (r.x, r.h) = (x.min(right - 0.01), (y - r.y).max(0.01)),
                    _ => (r.x, r.y) = (x - r.w / 2.0, y - r.h / 2.0),
                }
                if corner == 0 {
                    (r.w, r.h) = (right - r.x, bottom - r.y);
                } else if corner == 1 {
                    r.h = bottom - r.y;
                } else if corner == 3 {
                    r.w = right - r.x;
                }
            }
        }
        if response.drag_stopped() {
            *dragging = None;
        }

        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(if editing_mask {
                "Output: click to add mask points, right-click a point to remove it"
            } else {
                "Output: drag the grid points of the selected slice"
            })
            .small()
            .color(theme::TEXT_DIM),
        );
        let (rect, response) = canvas(ui, screen_aspect, 260.0);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::BLACK);
        if let Some(texture) = screen_texture {
            painter.image(
                texture,
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        let Some(slice) = screen.slices.get_mut(selected) else {
            return;
        };
        // Grid lines and points.
        let warp = &slice.warp;
        let grid: Vec<Pos2> = warp.points.iter().map(|p| to_screen(rect, *p)).collect();
        let line = Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.7));
        for r in 0..warp.rows {
            for c in 0..warp.columns {
                let p = grid[r * warp.columns + c];
                if c + 1 < warp.columns {
                    painter.line_segment([p, grid[r * warp.columns + c + 1]], line);
                }
                if r + 1 < warp.rows {
                    painter.line_segment([p, grid[(r + 1) * warp.columns + c]], line);
                }
            }
        }
        let mask_points: Vec<Pos2> = slice
            .mask
            .as_ref()
            .map(|k| k.points.iter().map(|p| to_screen(rect, *p)).collect())
            .unwrap_or_default();
        if mask_points.len() >= 2 {
            let mut closed = mask_points.clone();
            closed.push(mask_points[0]);
            painter.add(egui::Shape::line(closed, Stroke::new(1.5, theme::SOLO)));
        }
        if editing_mask {
            for p in &mask_points {
                handle(&painter, *p, theme::SOLO);
            }
        } else {
            for p in &grid {
                handle(&painter, *p, theme::ACCENT);
            }
        }
        let pointer = response.interact_pointer_pos();
        if response.drag_started()
            && let Some(pos) = ui.input(|i| i.pointer.press_origin())
        {
            *dragging = if editing_mask {
                nearest(&mask_points, pos, 10.0).map(Handle::Mask)
            } else {
                nearest(&grid, pos, 12.0).map(Handle::Warp)
            };
        }
        if response.dragged()
            && let Some(pos) = pointer
        {
            let p = from_screen(rect, pos);
            match *dragging {
                Some(Handle::Warp(i)) => slice.warp.points[i] = p,
                Some(Handle::Mask(i)) => {
                    if let Some(point) = slice.mask.as_mut().and_then(|k| k.points.get_mut(i)) {
                        *point = p;
                    }
                }
                _ => {}
            }
        }
        if response.drag_stopped() {
            *dragging = None;
        }
        if editing_mask {
            if response.clicked()
                && let Some(pos) = pointer
                && nearest(&mask_points, pos, 10.0).is_none()
            {
                slice
                    .mask
                    .get_or_insert_with(|| Mask {
                        points: Vec::new(),
                        invert: false,
                    })
                    .points
                    .push(from_screen(rect, pos));
            }
            if response.secondary_clicked()
                && let Some(pos) = response
                    .interact_pointer_pos()
                    .or(ui.input(|i| i.pointer.latest_pos()))
                && let Some(i) = nearest(&mask_points, pos, 10.0)
                && let Some(mask) = slice.mask.as_mut()
            {
                mask.points.remove(i);
            }
        }
    }
}
