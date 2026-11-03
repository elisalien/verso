//! Video outputs to other applications (NDI, Spout, Syphon), fed from the
//! rendered frame, and the screens of the Advanced Output: a window per
//! display screen, an NDI source per NDI screen.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui::{Color32, TextureId};
use eframe::{egui, egui_wgpu, wgpu};
use verso_core::output::{DeviceKind, OutputSetup, Screen};
use verso_io::ndi::NdiSender;
use verso_render::{FrameGrabber, GrabbedFrame, Renderer, ScreenRenderer};

use crate::app::VersoApp;
use crate::theme;

/// Sends frames to one output on its own thread: NDI compresses each frame
/// and texture sharing uploads it, which must not delay rendering. Frames
/// arriving while one is being sent are dropped.
struct Worker {
    frames: SyncSender<Arc<GrabbedFrame>>,
    /// Short state for the menu (receivers, last error).
    status: Arc<Mutex<String>>,
    /// NDI receivers connected; stays 0 for texture sharing.
    connections: Arc<AtomicUsize>,
    name: String,
}

impl Worker {
    /// Runs `send` for each frame; it returns the status to show.
    fn start(
        name: &str,
        mut send: impl FnMut(&GrabbedFrame) -> String + Send + 'static,
    ) -> Result<Self, String> {
        let (frames, incoming) = sync_channel::<Arc<GrabbedFrame>>(1);
        let status = Arc::new(Mutex::new(String::new()));
        let shown = status.clone();
        thread::Builder::new()
            .name(format!("verso-output-{name}"))
            .spawn(move || {
                // Ends when the worker, and so the channel, is dropped.
                for frame in incoming {
                    let text = send(&frame);
                    if let Ok(mut status) = shown.lock() {
                        *status = text;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            frames,
            status,
            connections: Arc::new(AtomicUsize::new(0)),
            name: name.to_owned(),
        })
    }

    fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

fn start_ndi(name: &str) -> Result<Worker, String> {
    let mut sender = NdiSender::new(name).map_err(|e| e.to_string())?;
    let connections = Arc::new(AtomicUsize::new(0));
    let count = connections.clone();
    let mut worker = Worker::start(name, move |frame| {
        sender.send(frame.width, frame.height, &frame.data, 60);
        count.store(sender.connections(), Ordering::Relaxed);
        format!("{} receiver(s) connected", sender.connections())
    })?;
    worker.connections = connections;
    Ok(worker)
}

/// Name of the local texture sharing system of this platform.
pub const SHARE_NAME: Option<&str> = if cfg!(windows) {
    Some("Spout")
} else if cfg!(target_os = "macos") {
    Some("Syphon")
} else {
    None
};

#[cfg(windows)]
fn start_share(name: &str) -> Result<Worker, String> {
    let mut sender = verso_io::spout::SpoutSender::new(name).map_err(|e| e.to_string())?;
    Worker::start(name, move |frame| {
        match sender.send(frame.width, frame.height, &frame.data) {
            Ok(()) => format!("{}×{}", frame.width, frame.height),
            Err(e) => e.to_string(),
        }
    })
}

#[cfg(target_os = "macos")]
fn start_share(name: &str) -> Result<Worker, String> {
    let mut server = verso_io::syphon::SyphonServer::new(name).map_err(|e| e.to_string())?;
    Worker::start(name, move |frame| {
        match server.send(frame.width, frame.height, &frame.data) {
            Ok(()) => format!("{}×{}", frame.width, frame.height),
            Err(e) => e.to_string(),
        }
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
fn start_share(_name: &str) -> Result<Worker, String> {
    Err("texture sharing needs Windows (Spout) or macOS (Syphon)".into())
}

/// NDI source of one Advanced Output screen.
struct ScreenNdi {
    id: u64,
    grabber: Option<FrameGrabber>,
    worker: Worker,
}

pub struct Outputs {
    grabber: Option<FrameGrabber>,
    ndi: Option<Worker>,
    pub ndi_name: String,
    /// Created when the Advanced Output first goes live.
    screens: Option<ScreenRenderer>,
    screen_ndi: Vec<ScreenNdi>,
    /// Screens whose NDI source failed to start: not retried until the
    /// Advanced Output goes live again.
    ndi_failed: Vec<u64>,
    /// Rendered screens, for windows and previews.
    pub(crate) screen_textures: HashMap<u64, TextureId>,
    /// Slice tinted on the screens (Highlight Selection).
    pub highlight: Option<u64>,
    /// Test patterns shown instead of the slices, by screen id.
    pub patterns: Vec<(u64, verso_render::Pattern)>,
    /// Spout (Windows) or Syphon (macOS) sender.
    share: Option<Worker>,
    pub share_name: String,
}

impl Default for Outputs {
    fn default() -> Self {
        Self {
            grabber: None,
            ndi: None,
            ndi_name: "Verso".into(),
            screens: None,
            screen_ndi: Vec::new(),
            ndi_failed: Vec::new(),
            screen_textures: HashMap::new(),
            highlight: None,
            patterns: Vec::new(),
            share: None,
            share_name: "Verso".into(),
        }
    }
}

impl Outputs {
    fn active(&self) -> bool {
        self.ndi.is_some() || self.share.is_some()
    }

    /// Records a copy of the output when an output needs it. Call before submit.
    pub fn capture(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &Renderer,
    ) {
        if !self.active() {
            self.grabber = None;
            return;
        }
        if self
            .grabber
            .as_ref()
            .is_none_or(|g| g.size() != renderer.size())
        {
            self.grabber = Some(FrameGrabber::new(device, renderer.size()));
        }
        if let Some(grabber) = &mut self.grabber {
            grabber.capture(encoder, renderer.output_texture());
        }
    }

    /// Hands finished frames to the outputs. Call after submit.
    pub fn deliver(&mut self, device: &wgpu::Device) {
        self.screen_ndi.retain_mut(|ndi| {
            let Some(grabber) = &mut ndi.grabber else {
                return true;
            };
            grabber.after_submit();
            let Some(frame) = grabber.poll(device) else {
                return true;
            };
            !matches!(
                ndi.worker.frames.try_send(Arc::new(frame)),
                Err(TrySendError::Disconnected(_))
            )
        });
        let Some(grabber) = &mut self.grabber else {
            return;
        };
        grabber.after_submit();
        let Some(frame) = grabber.poll(device) else {
            return;
        };
        // Shared by the outputs without copying the pixels.
        let frame = Arc::new(frame);
        for output in [&mut self.ndi, &mut self.share] {
            if let Some(worker) = output
                && let Err(TrySendError::Disconnected(_)) =
                    worker.frames.try_send(Arc::clone(&frame))
            {
                *output = None;
            }
        }
    }

    /// Renders the screens of `setup` from this frame's images, or stops
    /// them when `setup` is `None`. Returns the screens whose texture was
    /// recreated, and errors to show. Call before submit.
    pub fn render_screens(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        setup: Option<&OutputSetup>,
        renderer: &Renderer,
        comp: &verso_core::Composition,
    ) -> (Vec<u64>, Vec<String>) {
        let Some(setup) = setup else {
            self.screen_ndi.clear();
            self.ndi_failed.clear();
            self.screens = None;
            return (Vec::new(), Vec::new());
        };
        let screens = self
            .screens
            .get_or_insert_with(|| ScreenRenderer::new(device));
        screens.set_highlight(self.highlight);
        screens.set_patterns(std::mem::take(&mut self.patterns));
        let created = screens.render(device, queue, encoder, setup, renderer, comp);

        let mut errors = Vec::new();
        let ndi_screens: Vec<&Screen> = setup
            .screens
            .iter()
            .filter(|s| s.device.kind == DeviceKind::Ndi && screens.texture(s.id).is_some())
            .collect();
        self.screen_ndi
            .retain(|n| ndi_screens.iter().any(|s| s.id == n.id));
        for screen in ndi_screens {
            if self.ndi_failed.contains(&screen.id)
                || self.screen_ndi.iter().any(|n| n.id == screen.id)
            {
                continue;
            }
            let name = if screen.device.name.is_empty() {
                &screen.name
            } else {
                &screen.device.name
            };
            match start_ndi(name) {
                Ok(worker) => self.screen_ndi.push(ScreenNdi {
                    id: screen.id,
                    grabber: None,
                    worker,
                }),
                Err(e) => {
                    self.ndi_failed.push(screen.id);
                    errors.push(format!("Screen \"{}\": {e}", screen.name));
                }
            }
        }
        for ndi in &mut self.screen_ndi {
            let (Some(texture), Some(size)) = (screens.texture(ndi.id), screens.size(ndi.id))
            else {
                continue;
            };
            if ndi.grabber.as_ref().is_none_or(|g| g.size() != size) {
                ndi.grabber = Some(FrameGrabber::new(device, size));
            }
            if let Some(grabber) = &mut ndi.grabber {
                grabber.capture(encoder, texture);
            }
        }
        (created, errors)
    }

    /// Registers the screen textures recreated this frame with egui, and
    /// frees those of screens no longer rendered.
    pub fn register_screens(
        &mut self,
        egui: &mut egui_wgpu::Renderer,
        device: &wgpu::Device,
        created: &[u64],
    ) {
        let screens = self.screens.as_ref();
        self.screen_textures.retain(|id, texture| {
            let alive = screens.is_some_and(|s| s.texture(*id).is_some());
            if !alive {
                egui.free_texture(texture);
            }
            alive
        });
        let Some(screens) = screens else {
            return;
        };
        for &id in created {
            let Some(view) = screens.view(id) else {
                continue;
            };
            match self.screen_textures.get(&id) {
                Some(&texture) => egui.update_egui_texture_from_wgpu_texture(
                    device,
                    view,
                    wgpu::FilterMode::Linear,
                    texture,
                ),
                None => {
                    let texture =
                        egui.register_native_texture(device, view, wgpu::FilterMode::Linear);
                    self.screen_textures.insert(id, texture);
                }
            }
        }
    }

    /// NDI receivers connected to screen `id`, when it publishes a source.
    pub fn screen_receivers(&self, id: u64) -> Option<usize> {
        self.screen_ndi
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.worker.connections.load(Ordering::Relaxed))
    }
}

impl VersoApp {
    pub(crate) fn outputs_menu(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        let mut ndi_on = self.outputs.ndi.is_some();
        ui.horizontal(|ui| {
            if ui.checkbox(&mut ndi_on, "NDI® Output").changed() {
                if ndi_on {
                    match start_ndi(&self.outputs.ndi_name) {
                        Ok(worker) => {
                            self.status = format!("NDI output \"{}\" started", worker.name);
                            self.outputs.ndi = Some(worker);
                        }
                        Err(e) => self.problem(e),
                    }
                } else {
                    self.outputs.ndi = None;
                }
            }
            ui.add_enabled(
                self.outputs.ndi.is_none(),
                egui::TextEdit::singleline(&mut self.outputs.ndi_name).desired_width(110.0),
            )
            .on_hover_text("NDI source name");
        });
        if let Some(ndi) = &self.outputs.ndi {
            status_line(ui, &ndi.status());
        } else if !verso_io::ndi::available() {
            ui.label(
                egui::RichText::new("NDI runtime not found: install NDI Tools")
                    .small()
                    .color(theme::warn()),
            );
        }

        let Some(share) = SHARE_NAME else { return };
        ui.separator();
        let mut on = self.outputs.share.is_some();
        ui.horizontal(|ui| {
            if ui.checkbox(&mut on, format!("{share} Output")).changed() {
                if on {
                    match start_share(&self.outputs.share_name) {
                        Ok(worker) => {
                            self.status = format!("{share} output \"{}\" started", worker.name);
                            self.outputs.share = Some(worker);
                        }
                        Err(e) => self.problem(e),
                    }
                } else {
                    self.outputs.share = None;
                }
            }
            ui.add_enabled(
                self.outputs.share.is_none(),
                egui::TextEdit::singleline(&mut self.outputs.share_name).desired_width(110.0),
            )
            .on_hover_text(format!("{share} sender name"));
        });
        if let Some(worker) = &self.outputs.share {
            status_line(ui, &worker.status());
        }
    }
}

/// A monitor connected to this computer, named as Arena names displays.
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    /// "Display 1", "Display 2", … in system order.
    pub name: String,
    /// Model, when the system knows it.
    pub model: String,
    /// Desktop position and size, in physical pixels.
    pub position: (i32, i32),
    pub size: (u32, u32),
    pub scale: f32,
    pub primary: bool,
}

impl Monitor {
    pub fn label(&self) -> String {
        let (w, h) = self.size;
        let model = if self.model.is_empty() {
            String::new()
        } else {
            format!(" {}", self.model)
        };
        let primary = if self.primary { ", main" } else { "" };
        format!("{}: {w}×{h}{model}{primary}", self.name)
    }
}

/// Monitors connected now. Empty when the system cannot list them (on
/// Wayland, full screen then uses the monitor the window opens on).
pub fn monitors() -> Vec<Monitor> {
    match display_info::DisplayInfo::all() {
        Ok(all) => all
            .into_iter()
            .enumerate()
            .map(|(i, d)| Monitor {
                name: format!("Display {}", i + 1),
                model: d.friendly_name,
                position: (d.x, d.y),
                size: (d.width, d.height),
                scale: if d.scale_factor > 0.0 {
                    d.scale_factor
                } else {
                    1.0
                },
                primary: d.is_primary,
            })
            .collect(),
        Err(e) => {
            log::warn!("could not list monitors: {e}");
            Vec::new()
        }
    }
}

impl VersoApp {
    /// A window per live display screen. A screen assigned to a connected
    /// monitor goes full screen on it; otherwise the window can be moved,
    /// and double-clicked for full screen. Closing a window disables its
    /// screen.
    pub(crate) fn screen_windows(&mut self, ctx: &egui::Context) {
        if !self.advanced.live {
            return;
        }
        let canvas = self.advanced.setup.canvas;
        let monitors = self.advanced.monitors.get_or_insert_with(monitors).clone();
        for screen in &mut self.advanced.setup.screens {
            if screen.device.kind != DeviceKind::Display {
                continue;
            }
            let Some(&texture) = self.outputs.screen_textures.get(&screen.id) else {
                continue;
            };
            let monitor = monitors.iter().find(|m| m.name == screen.device.name);
            let builder =
                egui::ViewportBuilder::default().with_title(format!("Verso – {}", screen.name));
            let builder = match monitor {
                // Opened inside the monitor, then full screen on it.
                Some(m) => builder
                    .with_position([
                        (m.position.0 + 40) as f32 / m.scale,
                        (m.position.1 + 40) as f32 / m.scale,
                    ])
                    .with_inner_size([320.0, 180.0])
                    .with_decorations(false)
                    .with_fullscreen(true),
                None => {
                    let (w, h) = screen.size(canvas);
                    let scale = (960.0 / w as f32).min(1.0);
                    builder.with_inner_size([w as f32 * scale, h as f32 * scale])
                }
            };
            let mut close = false;
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of(("verso-screen", screen.id)),
                builder,
                |ui, _class| {
                    egui::CentralPanel::no_frame()
                        .frame(egui::Frame::NONE.fill(Color32::BLACK))
                        .show(ui, |ui| {
                            let rect = ui.max_rect();
                            let response = ui.allocate_rect(rect, egui::Sense::click());
                            ui.painter().image(
                                texture,
                                rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                Color32::WHITE,
                            );
                            if response.double_clicked() {
                                let fullscreen =
                                    ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
                                ui.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                                    !fullscreen,
                                ));
                            }
                        });
                    if ui.input(|i| i.viewport().close_requested()) {
                        close = true;
                    }
                },
            );
            if close {
                screen.enabled = false;
            }
        }
    }
}

fn status_line(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(theme::text_dim()));
}
