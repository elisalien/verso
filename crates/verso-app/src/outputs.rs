//! Video outputs to other applications (NDI), fed from the rendered frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::thread;

use eframe::{egui, wgpu};
use verso_io::ndi::NdiSender;
use verso_render::{FrameGrabber, GrabbedFrame, Renderer};

use crate::app::VersoApp;
use crate::theme;

/// Sends frames on its own thread: NDI converts and compresses each frame,
/// which must not delay rendering. Frames arriving while one is being sent
/// are dropped.
struct NdiWorker {
    frames: SyncSender<GrabbedFrame>,
    connections: Arc<AtomicUsize>,
    name: String,
}

impl NdiWorker {
    fn start(name: &str) -> verso_io::Result<Self> {
        let mut sender = NdiSender::new(name)?;
        let (frames, incoming) = sync_channel::<GrabbedFrame>(1);
        let connections = Arc::new(AtomicUsize::new(0));
        let count = connections.clone();
        thread::Builder::new()
            .name("verso-ndi".into())
            .spawn(move || {
                // Ends when the worker, and so the channel, is dropped.
                for frame in incoming {
                    sender.send(frame.width, frame.height, &frame.data, 60);
                    count.store(sender.connections(), Ordering::Relaxed);
                }
            })
            .map_err(|e| verso_io::Error::Ndi(e.to_string()))?;
        Ok(Self {
            frames,
            connections,
            name: name.to_owned(),
        })
    }
}

pub struct Outputs {
    grabber: Option<FrameGrabber>,
    ndi: Option<NdiWorker>,
    pub ndi_name: String,
}

impl Default for Outputs {
    fn default() -> Self {
        Self {
            grabber: None,
            ndi: None,
            ndi_name: "Verso".into(),
        }
    }
}

impl Outputs {
    fn active(&self) -> bool {
        self.ndi.is_some()
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
        if let Some(ndi) = &self.ndi
            && let Err(TrySendError::Disconnected(_)) = ndi.frames.try_send(frame)
        {
            self.ndi = None;
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
                    match NdiWorker::start(&self.outputs.ndi_name) {
                        Ok(worker) => {
                            self.status = format!("NDI output \"{}\" started", worker.name);
                            self.outputs.ndi = Some(worker);
                        }
                        Err(e) => self.status = e.to_string(),
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
            let receivers = ndi.connections.load(Ordering::Relaxed);
            ui.label(
                egui::RichText::new(format!("{receivers} receiver(s) connected"))
                    .small()
                    .color(theme::text_dim()),
            );
        } else if !verso_io::ndi::available() {
            ui.label(
                egui::RichText::new("NDI runtime not found: install NDI Tools")
                    .small()
                    .color(theme::warn()),
            );
        }
    }
}
