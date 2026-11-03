//! Video outputs to other applications (NDI, Spout, Syphon), fed from the
//! rendered frame.

use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::{egui, wgpu};
use verso_io::ndi::NdiSender;
use verso_render::{FrameGrabber, GrabbedFrame, Renderer};

use crate::app::VersoApp;
use crate::theme;

/// Sends frames to one output on its own thread: NDI compresses each frame
/// and texture sharing uploads it, which must not delay rendering. Frames
/// arriving while one is being sent are dropped.
struct Worker {
    frames: SyncSender<Arc<GrabbedFrame>>,
    /// Short state for the menu (receivers, last error).
    status: Arc<Mutex<String>>,
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
            name: name.to_owned(),
        })
    }

    fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

fn start_ndi(name: &str) -> Result<Worker, String> {
    let mut sender = NdiSender::new(name).map_err(|e| e.to_string())?;
    Worker::start(name, move |frame| {
        sender.send(frame.width, frame.height, &frame.data, 60);
        format!("{} receiver(s) connected", sender.connections())
    })
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

pub struct Outputs {
    grabber: Option<FrameGrabber>,
    ndi: Option<Worker>,
    pub ndi_name: String,
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
                    .color(theme::WARN),
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

fn status_line(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(theme::TEXT_DIM));
}
