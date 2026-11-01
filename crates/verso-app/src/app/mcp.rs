//! MCP server in the app: tool calls arrive from the `verso-mcp` threads and
//! run here, at the start of a frame, through the same commands as the UI.
//! Captures read GPU textures back after the frame renders. See `docs/MCP.md`.
//!
//! A child of `app`, so it reaches the app's private state without widening
//! its visibility.

use std::collections::VecDeque;
use std::io::Cursor;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::wgpu;
use serde_json::{Value, json};
use verso_core::control::{Action, Param};
use verso_core::{
    Clip, Command, Composition, Effect, EffectEdit, EffectTarget, ParamValue, Source,
};
use verso_mcp::summary::{self, SourceRequest, index_arg, optional_index};
use verso_mcp::{McpServer, ToolCall, ToolOutput};
use verso_render::{ParamKind, Renderer, ShaderKind};

use super::{PropsTab, VersoApp};
use crate::hover_preview::{HoverTarget, preview_composition};

/// Width of captures when the call gives none.
const CAPTURE_WIDTH: u32 = 640;
/// Frames rendered before an offscreen capture, so MilkDrop has drawn.
const OFFSCREEN_FRAMES: u32 = 60;
/// How long a simulated hover lasts when the call gives no duration.
const HOVER_SECONDS: f64 = 10.0;

/// Pointer position simulated by the `hover` tool.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hover {
    Header(HoverTarget),
    Slot(usize, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capture {
    /// The whole window, as egui painted it.
    Ui,
    Output,
    Hover,
    /// A source not placed in the composition.
    Source,
    Layer(usize),
    Column(usize),
    Clip(usize, usize),
}

impl Capture {
    fn offscreen(self) -> bool {
        matches!(
            self,
            Capture::Layer(_) | Capture::Column(_) | Capture::Clip(..) | Capture::Source
        )
    }

    /// Caption, with indices from 1 as in the tool arguments.
    fn label(self) -> String {
        match self {
            Capture::Ui => "Verso window".into(),
            Capture::Source => "Source".into(),
            Capture::Output => "Output".into(),
            Capture::Hover => "Hover preview".into(),
            Capture::Layer(l) => format!("Layer {}", l + 1),
            Capture::Column(c) => format!("Column {}", c + 1),
            Capture::Clip(l, c) => format!("Clip at layer {}, column {}", l + 1, c + 1),
        }
    }
}

impl Hover {
    fn label(self) -> String {
        match self {
            Hover::Header(HoverTarget::Layer(l)) => format!("layer {} header", l + 1),
            Hover::Header(HoverTarget::Column(c)) => format!("column {} header", c + 1),
            Hover::Header(HoverTarget::Clip(l, c)) => {
                format!("slot at layer {}, column {}", l + 1, c + 1)
            }
            Hover::Header(HoverTarget::Source) => "browser source".into(),
            Hover::Slot(l, c) => format!("slot at layer {}, column {}", l + 1, c + 1),
        }
    }
}

struct PendingCapture {
    call: ToolCall,
    target: Capture,
    /// For [`Capture::Source`].
    source: Option<Source>,
    frames_left: u32,
    width: u32,
}

/// Renderer drawing one offscreen capture target at one size.
struct Offscreen {
    target: Capture,
    source: Option<Source>,
    size: (u32, u32),
    renderer: Renderer,
}

#[derive(Default)]
pub struct McpState {
    server: Option<McpServer>,
    hover: Option<(Hover, Instant)>,
    captures: Vec<PendingCapture>,
    offscreen: Option<Offscreen>,
    /// Frame rate smoothed over about a second.
    fps: f32,
    ffmpeg: bool,
    ctx: Option<egui::Context>,
    /// Input events of a `ui_input` gesture, one batch per frame.
    input: VecDeque<Vec<egui::Event>>,
    /// The `ui_input` call, answered a few frames after its last events.
    input_call: Option<(ToolCall, u32)>,
    /// `ui_widgets` calls waiting for a frame with the accessibility tree.
    widget_calls: Vec<ToolCall>,
}

/// Frames the UI gets to react to injected input before `ui_input` answers.
const INPUT_SETTLE_FRAMES: u32 = 2;
/// Pointer moves of a drag, one per frame.
const DRAG_STEPS: usize = 8;

/// Port asked for on the command line or in `VERSO_MCP_PORT`; `None` keeps
/// the server off.
pub(super) fn requested_port() -> Option<u16> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    verso_mcp::requested_port(&args, std::env::var("VERSO_MCP_PORT").ok().as_deref())
}

impl McpState {
    /// Starts the server when asked on the command line or in the
    /// environment. Returns the state and a message for the status bar.
    pub fn start(ctx: &egui::Context) -> (Self, Option<String>) {
        let Some(port) = requested_port() else {
            return (Self::default(), None);
        };
        let mut state = Self {
            ffmpeg: verso_media::ffmpeg_available(),
            ctx: Some(ctx.clone()),
            ..Self::default()
        };
        let ctx = ctx.clone();
        let message = match McpServer::bind(port, move || ctx.request_repaint()) {
            Ok(server) => {
                let message = format!("MCP server on http://127.0.0.1:{}/mcp", server.port());
                state.server = Some(server);
                message
            }
            Err(e) => format!("MCP server could not listen on port {port}: {e}"),
        };
        // The status bar writes it to the log as well.
        (state, Some(message))
    }
}

/// Reads an RGBA8 texture back, waiting for the GPU.
fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Result<image::RgbaImage, String> {
    let (width, height) = (texture.width(), texture.height());
    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mcp capture"),
        size: u64::from(row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (sender, mapped) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(5)),
        })
        .map_err(|e| format!("GPU readback: {e}"))?;
    mapped
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "GPU readback timed out".to_owned())?
        .map_err(|e| format!("GPU readback: {e}"))?;
    let data = {
        let view = buffer
            .get_mapped_range(..)
            .map_err(|e| format!("GPU readback: {e}"))?;
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for line in view.chunks(row as usize) {
            data.extend_from_slice(&line[..(width * 4) as usize]);
        }
        data
    };
    buffer.unmap();
    image::RgbaImage::from_raw(width, height, data).ok_or_else(|| "bad readback size".into())
}

/// PNG of `image` scaled to `width`, opaque.
fn png(mut image: image::RgbaImage, width: u32) -> Result<Vec<u8>, String> {
    if width < image.width() {
        let height = (image.height() as f32 * width as f32 / image.width() as f32).round() as u32;
        image = image::imageops::resize(
            &image,
            width,
            height.max(1),
            image::imageops::FilterType::Triangle,
        );
    }
    for pixel in image.pixels_mut() {
        pixel.0[3] = 255;
    }
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn param_value(value: ParamValue) -> Value {
    match value {
        ParamValue::Float(v) => json!(v),
        ParamValue::Bool(b) => json!(b),
        ParamValue::Color(c) => json!(c),
        ParamValue::Point(p) => json!(p),
    }
}

fn param_kind(kind: &ParamKind) -> Value {
    match *kind {
        ParamKind::Float { min, max, default } => {
            json!({"type": "float", "min": min, "max": max, "default": default})
        }
        ParamKind::Color { default } => json!({"type": "color", "default": default}),
        ParamKind::Bool { default } => json!({"type": "bool", "default": default}),
        ParamKind::Point { min, max, default } => {
            json!({"type": "point", "min": min, "max": max, "default": default})
        }
    }
}

fn contains(haystack: &str, filter: &str) -> bool {
    filter.is_empty() || haystack.to_lowercase().contains(filter)
}

/// Effect target named by `layer`, `column` and `group` arguments.
fn target_arg(args: &Value) -> Result<EffectTarget, String> {
    Ok(
        match (
            optional_index(args, "layer")?,
            optional_index(args, "column")?,
            optional_index(args, "group")?,
        ) {
            (Some(layer), Some(column), None) => EffectTarget::Clip { layer, column },
            (Some(layer), None, None) => EffectTarget::Layer(layer),
            (None, None, Some(group)) => EffectTarget::Group(group),
            (None, None, None) => EffectTarget::Composition,
            _ => {
                return Err(
                    "give layer and column (clip), layer, group, or nothing (composition)".into(),
                );
            }
        },
    )
}

fn modifiers(m: verso_mcp::gesture::Modifiers) -> egui::Modifiers {
    egui::Modifiers {
        alt: m.alt,
        ctrl: m.ctrl,
        shift: m.shift,
        mac_cmd: false,
        command: m.ctrl,
    }
}

/// egui events of a gesture, one batch per frame, so the UI sees presses,
/// moves and releases as separate steps the way a real mouse makes them.
fn gesture_events(
    gesture: &verso_mcp::gesture::Gesture,
) -> Result<VecDeque<Vec<egui::Event>>, String> {
    use egui::{Event, PointerButton, pos2};
    use verso_mcp::gesture::{Button, Gesture};
    let button = |b: Button| match b {
        Button::Primary => PointerButton::Primary,
        Button::Secondary => PointerButton::Secondary,
        Button::Middle => PointerButton::Middle,
    };
    let press = |at: egui::Pos2, button, pressed, m| Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: m,
    };
    let mut steps = VecDeque::new();
    match gesture {
        Gesture::Move { at } => steps.push_back(vec![Event::PointerMoved(pos2(at[0], at[1]))]),
        Gesture::Click {
            at,
            button: b,
            count,
            modifiers: m,
        } => {
            let (at, m) = (pos2(at[0], at[1]), modifiers(*m));
            steps.push_back(vec![Event::PointerMoved(at)]);
            for _ in 0..*count {
                steps.push_back(vec![press(at, button(*b), true, m)]);
                steps.push_back(vec![press(at, button(*b), false, m)]);
            }
        }
        Gesture::Drag {
            from,
            to,
            modifiers: m,
        } => {
            let (from, to, m) = (pos2(from[0], from[1]), pos2(to[0], to[1]), modifiers(*m));
            steps.push_back(vec![Event::PointerMoved(from)]);
            steps.push_back(vec![press(from, PointerButton::Primary, true, m)]);
            for i in 1..=DRAG_STEPS {
                let t = i as f32 / DRAG_STEPS as f32;
                steps.push_back(vec![Event::PointerMoved(from.lerp(to, t))]);
            }
            steps.push_back(vec![press(to, PointerButton::Primary, false, m)]);
        }
        Gesture::Scroll { at, delta } => {
            steps.push_back(vec![Event::PointerMoved(pos2(at[0], at[1]))]);
            steps.push_back(vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(delta[0], delta[1]),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        Gesture::Key { name, modifiers: m } => {
            let key = egui::Key::from_name(name).ok_or(format!("unknown key {name}"))?;
            let key_event = |pressed| Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers: modifiers(*m),
            };
            steps.push_back(vec![key_event(true)]);
            steps.push_back(vec![key_event(false)]);
        }
        Gesture::Text(text) => steps.push_back(vec![Event::Text(text.clone())]),
        Gesture::Leave => steps.push_back(vec![Event::PointerGone]),
    }
    Ok(steps)
}

impl VersoApp {
    /// Runs the tool calls that arrived and keeps a simulated hover alive.
    /// Called once per frame, before commands apply and the frame renders.
    pub(super) fn poll_mcp(&mut self) {
        if self.frame_dt > 0.0 {
            let fps = (1.0 / self.frame_dt) as f32;
            self.mcp.fps += (fps - self.mcp.fps) * (self.frame_dt as f32).min(1.0);
        }
        // The UI resets the hover from the real pointer every frame.
        if let Some((hover, until)) = self.mcp.hover {
            if Instant::now() >= until {
                self.mcp.hover = None;
                self.hover_target = None;
                self.hovered_clip = None;
            } else {
                match hover {
                    Hover::Header(target) => self.hover_target = Some(target),
                    Hover::Slot(l, c) => self.hover_clip(l, c),
                }
            }
        }
        if self.mcp.input.is_empty()
            && let Some((call, frames)) = self.mcp.input_call.take()
        {
            if frames == 0 {
                let pointer = self
                    .mcp
                    .ctx
                    .as_ref()
                    .and_then(|ctx| ctx.input(|i| i.pointer.latest_pos()));
                call.reply(ToolOutput::text(match pointer {
                    Some(p) => format!("Done; pointer at ({:.0}, {:.0})", p.x, p.y),
                    None => "Done; pointer outside the window".into(),
                }));
            } else {
                self.mcp.input_call = Some((call, frames - 1));
            }
        }
        if !(self.mcp.input.is_empty()
            && self.mcp.input_call.is_none()
            && self.mcp.widget_calls.is_empty()
            && self.mcp.captures.is_empty())
        {
            self.paint_while_hidden();
        }
        let calls = self
            .mcp
            .server
            .as_ref()
            .map(McpServer::poll)
            .unwrap_or_default();
        for call in calls {
            self.run_tool(call);
        }
    }

    /// Keeps the UI pass running while the window is minimized or hidden,
    /// so screenshots, injected input and the widget list still work.
    fn paint_while_hidden(&self) {
        if let Some(ctx) = &self.mcp.ctx {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint();
        }
    }

    /// Feeds the next batch of a `ui_input` gesture to egui, as if the user
    /// moved the mouse or typed. Called by `raw_input_hook`.
    pub(super) fn mcp_raw_input(&mut self, raw_input: &mut egui::RawInput) {
        let Some(events) = self.mcp.input.pop_front() else {
            return;
        };
        // egui follows modifier keys through these events.
        let held = events.iter().find_map(|event| match event {
            egui::Event::Key { modifiers, .. } | egui::Event::PointerButton { modifiers, .. }
                if !modifiers.is_none() =>
            {
                Some(*modifiers)
            }
            _ => None,
        });
        if let Some(modifiers) = held {
            raw_input
                .events
                .push(egui::Event::ModifiersChanged(modifiers));
        }
        raw_input.events.extend(events);
        if held.is_some() && self.mcp.input.is_empty() {
            raw_input
                .events
                .push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        }
        if let Some(ctx) = &self.mcp.ctx {
            ctx.request_repaint();
        }
    }

    /// Answers `ui_widgets` from the accessibility tree egui built during
    /// this pass. Called at the end of the UI pass.
    pub(super) fn mcp_after_ui(&mut self, ctx: &egui::Context) {
        if self.mcp.widget_calls.is_empty() {
            return;
        }
        // The tree exists only for passes that began with AccessKit enabled.
        let Some(widgets) = ctx.viewport(|v| {
            let state = v.this_pass.accesskit_state.as_ref()?;
            let mut widgets: Vec<Value> = state
                .nodes
                .values()
                .filter_map(|node| {
                    // Text runs repeat the label of their parent.
                    if format!("{:?}", node.role()) == "TextRun" {
                        return None;
                    }
                    let bounds = node.bounds()?;
                    let label = node.label().or(node.value()).map(str::to_owned);
                    let toggled = node.toggled().map(|t| format!("{t:?}"));
                    let number = node.numeric_value();
                    if label.as_deref().is_none_or(str::is_empty)
                        && toggled.is_none()
                        && number.is_none()
                    {
                        return None;
                    }
                    let (x0, y0, x1, y1) = (bounds.x0, bounds.y0, bounds.x1, bounds.y1);
                    Some(json!({
                        "role": format!("{:?}", node.role()),
                        "label": label,
                        "toggled": toggled,
                        "value": number,
                        "disabled": node.is_disabled(),
                        "rect": [x0.round(), y0.round(), x1.round(), y1.round()],
                        "center": [((x0 + x1) / 2.0).round(), ((y0 + y1) / 2.0).round()],
                    }))
                })
                .collect();
            // Reading order: top to bottom, then left to right.
            widgets.sort_by(|a, b| {
                let key = |w: &Value| (w["rect"][1].as_f64(), w["rect"][0].as_f64());
                key(a)
                    .partial_cmp(&key(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            Some(widgets)
        }) else {
            return;
        };
        ctx.disable_accesskit();
        let slots: Vec<Value> = self
            .cell_rects
            .iter()
            .map(|(rect, (l, c))| {
                let clip = self.comp.layers.get(*l).and_then(|layer| layer.clips.get(*c));
                json!({
                    "layer": l + 1,
                    "column": c + 1,
                    "clip": clip.and_then(Option::as_ref).map(|clip| &clip.name),
                    "rect": [rect.min.x.round(), rect.min.y.round(), rect.max.x.round(), rect.max.y.round()],
                    "center": [rect.center().x.round(), rect.center().y.round()],
                })
            })
            .collect();
        let screen = ctx.content_rect();
        for call in std::mem::take(&mut self.mcp.widget_calls) {
            let filter = call.args["filter"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase();
            let limit = call.args["limit"].as_u64().unwrap_or(200) as usize;
            let matching: Vec<&Value> = widgets
                .iter()
                .filter(|w| contains(w["label"].as_str().unwrap_or_default(), &filter))
                .collect();
            call.reply(ToolOutput::json(&json!({
                "window_points": [screen.width().round(), screen.height().round()],
                "pixels_per_point": ctx.pixels_per_point(),
                "total": matching.len(),
                "widgets": matching.into_iter().take(limit).collect::<Vec<_>>(),
                "clip_slots": if filter.is_empty() { slots.clone() } else { Vec::new() },
            })));
        }
    }

    /// Captures the whole window as egui paints it next frame.
    fn screenshot_ui(&self, call: ToolCall, width: u32) {
        let Some(ctx) = &self.mcp.ctx else {
            call.reply(ToolOutput::error("no UI context"));
            return;
        };
        let pixels_per_point = ctx.pixels_per_point();
        ctx.request_screenshot(move |image| {
            // Encoding takes a while: not on the render thread.
            std::thread::spawn(move || {
                let [w, h] = image.size;
                let bytes: Vec<u8> = image
                    .pixels
                    .iter()
                    .flat_map(|p| p.to_srgba_unmultiplied())
                    .collect();
                let output = image::RgbaImage::from_raw(w as u32, h as u32, bytes)
                    .ok_or_else(|| "bad screenshot size".to_owned())
                    .and_then(|image| png(image, width))
                    .map_or_else(ToolOutput::error, |png| {
                        ToolOutput::png(
                            png,
                            format!(
                                "Verso window, {w}x{h} pixels = {:.0}x{:.0} UI points \
                                 (pixels_per_point {pixels_per_point}); ui_input takes points",
                                w as f32 / pixels_per_point,
                                h as f32 / pixels_per_point
                            ),
                        )
                    });
                call.reply(output);
            });
        });
        ctx.request_repaint();
    }

    fn run_tool(&mut self, call: ToolCall) {
        self.paint_while_hidden();
        let args = call.args.clone();
        let output = match call.name.as_str() {
            "capture" => match self.queue_capture(&args) {
                Ok((Capture::Ui, _, _, width)) => {
                    self.screenshot_ui(call, width);
                    return;
                }
                Ok((target, source, frames, width)) => {
                    self.mcp.captures.push(PendingCapture {
                        call,
                        target,
                        source,
                        frames_left: frames,
                        width,
                    });
                    return;
                }
                Err(e) => Err(e),
            },
            "ui_input" => match verso_mcp::gesture::parse(&args).and_then(|g| gesture_events(&g)) {
                Ok(_) if self.mcp.input_call.is_some() => {
                    Err("the previous ui_input gesture is still running".into())
                }
                Ok(events) => {
                    self.mcp.input = events;
                    self.mcp.input_call = Some((call, INPUT_SETTLE_FRAMES));
                    return;
                }
                Err(e) => Err(e),
            },
            "ui_widgets" => {
                if let Some(ctx) = &self.mcp.ctx {
                    ctx.enable_accesskit();
                }
                self.mcp.widget_calls.push(call);
                return;
            }
            "status" => Ok(self.mcp_status()),
            "get_composition" => Ok(if args["full"].as_bool() == Some(true) {
                ToolOutput::json(&serde_json::to_value(&self.comp).unwrap_or_default())
            } else {
                let mut summary = summary::composition(&self.comp);
                summary["selected"] = json!({
                    "layer": self.selected_layer + 1,
                    "clip": self.selected_clip.map(|(l, c)| [l + 1, c + 1]),
                });
                ToolOutput::json(&summary)
            }),
            "get_layer" => self.mcp_layer(&args),
            "get_clip" => self.mcp_clip(&args),
            "list_sources" => Ok(self.mcp_sources(&args)),
            "list_effects" => Ok(self.mcp_effects(&args)),
            "get_log" => Ok(self.mcp_log(&args)),
            "osc" => self.mcp_osc(&args),
            "set_clip" => self.mcp_set_clip(&args),
            "set_milkdrop" => self.mcp_set_milkdrop(&args),
            "add_effect" => self.mcp_add_effect(&args),
            "apply_commands" => serde_json::from_value::<Vec<Command>>(args["commands"].clone())
                .map_err(|e| format!("commands: {e}"))
                .and_then(|commands| {
                    let count = commands.len();
                    self.apply_now(commands, format!("{count} command(s) applied"))
                }),
            "select" => self.mcp_select(&args),
            "hover" => self.mcp_hover(&args),
            "undo" => {
                self.undo();
                Ok(ToolOutput::text(self.status.clone()))
            }
            "redo" => {
                self.redo();
                Ok(ToolOutput::text(self.status.clone()))
            }
            "save" => {
                let path = args["path"]
                    .as_str()
                    .map(PathBuf::from)
                    .or(self.path.clone());
                match path {
                    Some(path) => {
                        self.save_to(&path);
                        Ok(ToolOutput::text(self.status.clone()))
                    }
                    None => Err("the composition has no file yet: give a path".into()),
                }
            }
            "open" => {
                let path = PathBuf::from(args["path"].as_str().unwrap_or_default());
                self.open_file(path);
                Ok(ToolOutput::text(self.status.clone()))
            }
            "new_composition" => {
                self.replace_composition(Composition::new("Untitled", 3, 8), None);
                Ok(ToolOutput::text("New composition"))
            }
            other => Err(format!(
                "tool {other} is not handled by this version of Verso"
            )),
        };
        call.reply(output.unwrap_or_else(ToolOutput::error));
    }

    /// Queues `commands` and applies them now, as the end of a frame would.
    /// An error reported by the composition makes the call fail.
    fn apply_now(&mut self, commands: Vec<Command>, done: String) -> Result<ToolOutput, String> {
        let before = self.status.clone();
        self.status.clear();
        for command in commands {
            self.send(command);
        }
        self.apply_pending();
        if self.status.is_empty() {
            self.status = before;
            Ok(ToolOutput::text(done))
        } else {
            Err(self.status.clone())
        }
    }

    fn mcp_status(&self) -> ToolOutput {
        ToolOutput::json(&json!({
            "pid": std::process::id(),
            "version": env!("CARGO_PKG_VERSION"),
            "fps": self.mcp.fps.round(),
            "frame_ms": self.frame_dt * 1000.0,
            "composition": self.comp.name,
            "file": self.path,
            "unsaved_changes": self.dirty,
            "size": [self.comp.width, self.comp.height],
            "layers": self.comp.layers.len(),
            "columns": self.comp.columns.len(),
            "undo_steps": self.undo.len(),
            "mcp_port": self.mcp.server.as_ref().map(McpServer::port),
            "log_file": crate::logging::path(),
            "ffmpeg": self.mcp.ffmpeg,
            "effects": self.renderer.library().sorted().len(),
            "milkdrop_families": self.renderer.milkdrop().library().len(),
            "osc_port": self.controls.osc.as_ref().map(|o| o.port()),
            "output_window": self.output_open,
            "status": self.status,
        }))
    }

    /// Effects of a chain with their definitions, values and OSC addresses.
    fn effect_details(&self, target: EffectTarget, chain: &[Effect]) -> Value {
        chain
            .iter()
            .enumerate()
            .map(|(index, effect)| {
                let def = self.renderer.library().get(&effect.id);
                let params: Vec<Value> = def
                    .map(|def| {
                        def.params
                            .iter()
                            .map(|p| {
                                json!({
                                    "name": p.name,
                                    "kind": param_kind(&p.kind),
                                    "value": param_value(def.value(effect, p)),
                                    "osc": summary::effect_address(target, index, &p.name),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                json!({
                    "index": index + 1,
                    "id": effect.id,
                    "name": effect.name,
                    "bypass": effect.bypass,
                    "missing": def.is_none(),
                    "bypass_osc": Action::Set(Param::EffectBypass { target, index }).address(),
                    "params": params,
                })
            })
            .collect()
    }

    fn mcp_layer(&self, args: &Value) -> Result<ToolOutput, String> {
        let l = index_arg(args, "layer")?;
        let layer = self
            .comp
            .layers
            .get(l)
            .ok_or(format!("no layer {}", l + 1))?;
        let mut value = summary::composition(&self.comp)["layers"][l].take();
        value["effects"] = self.effect_details(EffectTarget::Layer(l), &layer.effects);
        value["osc"] = json!({
            "opacity": Action::Set(Param::LayerOpacity(l)).address(),
            "bypass": Action::Set(Param::LayerBypass(l)).address(),
            "solo": Action::Set(Param::LayerSolo(l)).address(),
            "transition": Action::Set(Param::LayerTransition(l)).address(),
            "eject": Action::EjectLayer(l).address(),
        });
        Ok(ToolOutput::json(&value))
    }

    fn mcp_clip(&self, args: &Value) -> Result<ToolOutput, String> {
        let (l, c) = (index_arg(args, "layer")?, index_arg(args, "column")?);
        let clip = self
            .comp
            .layers
            .get(l)
            .and_then(|layer| layer.clips.get(c))
            .ok_or(format!("no slot at layer {}, column {}", l + 1, c + 1))?
            .as_ref()
            .ok_or("the slot is empty")?;
        let mut source = serde_json::to_value(&clip.source).unwrap_or_default();
        if let Source::Milkdrop { family, .. } = &clip.source
            && let Some(info) = self.renderer.milkdrop().library().info(family)
        {
            source = json!({"source": source, "family_name": info.name, "variants": info.variants});
        }
        let transport = self.media.transport(l, c).map(|t| {
            json!({"position": t.position, "duration": t.duration, "frame": t.frame, "ended": t.ended})
        });
        Ok(ToolOutput::json(&json!({
            "layer": l + 1,
            "column": c + 1,
            "name": clip.name,
            "playing": self.comp.layers[l].active == Some(c),
            "launches": self.comp.layers[l].launch_count(c),
            "source": source,
            "media_failed": self.media.failed(&clip.source),
            "transport": transport,
            "effects": self.effect_details(EffectTarget::Clip { layer: l, column: c }, &clip.effects),
            "launch_osc": Action::LaunchClip { layer: l, column: c }.address(),
        })))
    }

    fn mcp_sources(&self, args: &Value) -> ToolOutput {
        let filter = args["filter"].as_str().unwrap_or_default().to_lowercase();
        let limit = args["limit"].as_u64().unwrap_or(50) as usize;
        let generators: Vec<&str> = verso_core::Generator::ALL
            .iter()
            .map(|g| g.name())
            .filter(|n| contains(n, &filter))
            .collect();
        let shaders: Vec<_> = self
            .renderer
            .library()
            .sorted_kind(ShaderKind::Generator)
            .into_iter()
            .filter(|d| contains(&d.name, &filter) || contains(&d.id, &filter))
            .collect();
        let families: Vec<_> = self
            .renderer
            .milkdrop()
            .library()
            .sorted()
            .into_iter()
            .filter(|f| contains(&f.name, &filter) || contains(&f.id, &filter))
            .collect();
        ToolOutput::json(&json!({
            "generators": generators,
            "solid": "{\"solid\": [r, g, b, a]}",
            "shaders": {
                "total": shaders.len(),
                "items": shaders.iter().take(limit).map(|d| json!({
                    "id": d.id, "name": d.name, "category": d.category, "credit": d.credit,
                })).collect::<Vec<_>>(),
            },
            "milkdrop": {
                "total": families.len(),
                "items": families.iter().take(limit).map(|f| json!({
                    "id": f.id, "name": f.name, "category": f.category,
                    "variants": f.variants,
                })).collect::<Vec<_>>(),
            },
        }))
    }

    fn mcp_effects(&self, args: &Value) -> ToolOutput {
        let filter = args["filter"].as_str().unwrap_or_default().to_lowercase();
        let effects: Vec<Value> = self
            .renderer
            .library()
            .sorted_kind(ShaderKind::Filter)
            .into_iter()
            .filter(|d| contains(&d.name, &filter) || contains(&d.id, &filter))
            .map(|d| {
                json!({
                    "id": d.id,
                    "name": d.name,
                    "category": d.category,
                    "params": d.params.iter()
                        .map(|p| json!({"name": p.name, "kind": param_kind(&p.kind)}))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        ToolOutput::json(&json!(effects))
    }

    fn mcp_log(&self, args: &Value) -> ToolOutput {
        let lines = args["lines"].as_u64().unwrap_or(50) as usize;
        let now = Instant::now();
        let messages: Vec<String> = self
            .log
            .iter()
            .rev()
            .take(lines)
            .rev()
            .map(|(when, text)| format!("{:>5}s ago  {text}", now.duration_since(*when).as_secs()))
            .collect();
        let file = crate::logging::path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| {
                let all: Vec<&str> = text.lines().collect();
                all[all.len().saturating_sub(lines)..].join("\n")
            })
            .unwrap_or_default();
        ToolOutput::text(format!(
            "Status messages (newest last):\n{}\n\nLog file {}:\n{file}",
            messages.join("\n"),
            crate::logging::path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(none)".into())
        ))
    }

    fn mcp_osc(&mut self, args: &Value) -> Result<ToolOutput, String> {
        let address = args["address"].as_str().ok_or("address is required")?;
        let value = args["value"].as_f64().map(|v| v as f32);
        let (action, unit) =
            Action::parse_osc(address, value).ok_or(format!("unknown OSC address {address}"))?;
        // As an OSC message from the network would.
        self.controls.last_message = format!(
            "MCP {address} {}",
            value.map(|v| format!("{v:.3}")).unwrap_or_default()
        );
        let range = match &action {
            Action::Set(param) => self.param_range(&self.comp, param).map(|(r, _)| r),
            _ => None,
        };
        let Some(command) = action.command(unit, range) else {
            return Ok(ToolOutput::text(match action {
                Action::Set(_) => format!("{address}: no such parameter here"),
                _ => format!("{address}: a value of 0.5 or less does not press a trigger"),
            }));
        };
        self.apply_now(vec![command], String::new())?;
        let now = match &action {
            Action::Set(param) => self
                .param_range(&self.comp, param)
                .map(|(_, v)| format!("{} = {v}", action.address()))
                .unwrap_or_else(|| action.address()),
            _ => format!("{} done", action.address()),
        };
        Ok(ToolOutput::text(now))
    }

    /// Source and default clip name for a `source` argument (see `set_clip`).
    fn resolve_source(&self, value: &Value) -> Result<(Source, String), String> {
        Ok(match summary::parse_source(value)? {
            SourceRequest::File(path) => {
                let template = crate::grid::SourceTemplate::from_path(&path).ok_or(format!(
                    "{} is not a supported image or video",
                    path.display()
                ))?;
                (template.source, template.name)
            }
            SourceRequest::Shader(id) => {
                let def = self
                    .renderer
                    .library()
                    .get(&id)
                    .ok_or(format!("no shader {id}; see list_sources"))?;
                if def.kind != ShaderKind::Generator {
                    return Err(format!("{id} is an effect, not a source: use add_effect"));
                }
                let name = def.name.clone();
                (Source::Shader(Effect::new(&id, &name)), name)
            }
            SourceRequest::Ready(source) => {
                let name = match &source {
                    Source::Solid { .. } => "Solid Color".to_owned(),
                    Source::Generator { kind, .. } => kind.name().to_owned(),
                    Source::Milkdrop {
                        family, variant, ..
                    } => {
                        let info = self
                            .renderer
                            .milkdrop()
                            .library()
                            .info(family)
                            .ok_or(format!("no MilkDrop family {family}; see list_sources"))?;
                        if *variant >= info.variants.len() {
                            return Err(format!("{family} has {} variant(s)", info.variants.len()));
                        }
                        info.name.clone()
                    }
                    other => other
                        .path()
                        .and_then(|p| p.file_stem())
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Clip".into()),
                };
                (source, name)
            }
        })
    }

    fn mcp_set_clip(&mut self, args: &Value) -> Result<ToolOutput, String> {
        let (layer, column) = (index_arg(args, "layer")?, index_arg(args, "column")?);
        let clip = if args["source"].is_null() {
            None
        } else {
            let (source, name) = self.resolve_source(&args["source"])?;
            let name = args["name"].as_str().map(str::to_owned).unwrap_or(name);
            Some(Clip::new(name, source))
        };
        let done = match &clip {
            Some(clip) => format!(
                "{} in layer {}, column {}",
                clip.name,
                layer + 1,
                column + 1
            ),
            None => format!("Emptied layer {}, column {}", layer + 1, column + 1),
        };
        self.apply_now(
            vec![Command::SetClip {
                layer,
                column,
                clip,
            }],
            done,
        )
    }

    fn mcp_set_milkdrop(&mut self, args: &Value) -> Result<ToolOutput, String> {
        let (layer, column) = (index_arg(args, "layer")?, index_arg(args, "column")?);
        let mut clip = self
            .comp
            .layers
            .get(layer)
            .and_then(|l| l.clips.get(column).cloned().flatten())
            .ok_or("no clip in that slot")?;
        let Source::Milkdrop {
            family,
            variant,
            blend,
            input,
            input_mix,
        } = &mut clip.source
        else {
            return Err("that clip is not a MilkDrop preset".into());
        };
        if let Some(v) = args["variant"].as_u64() {
            let count = self
                .renderer
                .milkdrop()
                .library()
                .info(family)
                .map_or(usize::MAX, |i| i.variants.len());
            let v = (v as usize).checked_sub(1).ok_or("variant counts from 1")?;
            if v >= count {
                return Err(format!("{family} has {count} variant(s)"));
            }
            *variant = v;
        }
        if let Some(b) = args["blend"].as_f64() {
            *blend = b as f32;
        }
        if let Some(i) = args["input_layer"].as_u64() {
            *input = (i as usize).checked_sub(1);
        }
        if let Some(m) = args["input_mix"].as_f64() {
            *input_mix = (m as f32).clamp(0.0, 1.0);
        }
        let done = format!("{}", summary::source(&clip.source));
        self.apply_now(
            vec![Command::SetClip {
                layer,
                column,
                clip: Some(clip),
            }],
            done,
        )
    }

    fn mcp_add_effect(&mut self, args: &Value) -> Result<ToolOutput, String> {
        let id = args["effect"].as_str().ok_or("effect is required")?;
        let target = target_arg(args)?;
        let def = self
            .renderer
            .library()
            .get(id)
            .ok_or(format!("no effect {id}; see list_effects"))?;
        if def.kind != ShaderKind::Filter {
            return Err(format!("{id} is a source: use set_clip with shader"));
        }
        let effect = Effect::new(id, &def.name);
        let done = format!("Added {} to {target:?}", def.name);
        self.apply_now(
            vec![Command::EditEffects {
                target,
                edit: EffectEdit::Add(effect),
            }],
            done,
        )
    }

    fn mcp_select(&mut self, args: &Value) -> Result<ToolOutput, String> {
        let layer = index_arg(args, "layer")?;
        if layer >= self.comp.layers.len() {
            return Err(format!("no layer {}", layer + 1));
        }
        self.selected_layer = layer;
        match optional_index(args, "column")? {
            Some(column) if column < self.comp.columns.len() => {
                self.selected_clip = Some((layer, column));
                self.props_tab = PropsTab::Clip;
            }
            Some(column) => return Err(format!("no column {}", column + 1)),
            None => self.props_tab = PropsTab::Layer,
        }
        Ok(ToolOutput::text("Selected"))
    }

    fn mcp_hover(&mut self, args: &Value) -> Result<ToolOutput, String> {
        let layer = optional_index(args, "layer")?;
        let column = optional_index(args, "column")?;
        let hover = match (layer, column) {
            (Some(l), Some(c)) => Hover::Slot(l, c),
            (Some(l), None) => Hover::Header(HoverTarget::Layer(l)),
            (None, Some(c)) => Hover::Header(HoverTarget::Column(c)),
            (None, None) => {
                self.mcp.hover = None;
                self.hover_target = None;
                self.hovered_clip = None;
                return Ok(ToolOutput::text("Hover stopped"));
            }
        };
        if layer.is_some_and(|l| l >= self.comp.layers.len())
            || column.is_some_and(|c| c >= self.comp.columns.len())
        {
            return Err("no such layer or column".into());
        }
        let seconds = args["seconds"].as_f64().unwrap_or(HOVER_SECONDS).max(0.0);
        self.mcp.hover = Some((hover, Instant::now() + Duration::from_secs_f64(seconds)));
        Ok(ToolOutput::text(format!(
            "Hovering the {} for {seconds} s",
            hover.label()
        )))
    }

    fn queue_capture(&self, args: &Value) -> Result<(Capture, Option<Source>, u32, u32), String> {
        let layer = || index_arg(args, "layer");
        let column = || index_arg(args, "column");
        let mut source = None;
        let target = match args["target"].as_str().unwrap_or_default() {
            "ui" => Capture::Ui,
            "output" => Capture::Output,
            "source" => {
                let (resolved, _) = self.resolve_source(&args["source"])?;
                if !resolved.is_procedural() {
                    return Err(
                        "files are not previewed here: put the file in a slot with set_clip, \
                         then capture target=clip"
                            .into(),
                    );
                }
                source = Some(resolved);
                Capture::Source
            }
            "hover" => {
                if self.hover_target.is_none() {
                    return Err(
                        "nothing is hovered: call hover with a layer or a column first".into(),
                    );
                }
                Capture::Hover
            }
            "layer" => Capture::Layer(layer()?),
            "column" => Capture::Column(column()?),
            "clip" => Capture::Clip(layer()?, column()?),
            other => return Err(format!("unknown capture target {other:?}")),
        };
        if let Capture::Layer(l) | Capture::Clip(l, _) = target
            && l >= self.comp.layers.len()
        {
            return Err(format!("no layer {}", l + 1));
        }
        if let Capture::Column(c) | Capture::Clip(_, c) = target
            && c >= self.comp.columns.len()
        {
            return Err(format!("no column {}", c + 1));
        }
        let default_frames = if matches!(target, Capture::Output | Capture::Ui) {
            1
        } else {
            OFFSCREEN_FRAMES
        };
        let frames = args["frames"]
            .as_u64()
            .map_or(default_frames, |f| f.clamp(1, 600) as u32);
        let width = args["width"]
            .as_u64()
            .map_or(CAPTURE_WIDTH, |w| w.clamp(16, 3840) as u32);
        Ok((target, source, frames, width))
    }

    /// Composition and media an offscreen capture draws.
    fn offscreen_frame(&self, target: Capture, source: Option<&Source>) -> Composition {
        if let (Capture::Source, Some(source)) = (target, source) {
            let mut comp = Composition::new("Source preview", 1, 1);
            (comp.width, comp.height) = (self.comp.width, self.comp.height);
            comp.layers[0].opacity = 1.0;
            let clip = Some(Clip::new("Source", source.clone()));
            let _ = comp.apply(Command::SetClip {
                layer: 0,
                column: 0,
                clip,
            });
            let _ = comp.apply(Command::TriggerClip {
                layer: 0,
                column: 0,
            });
            return comp;
        }
        let rendered = self.modulated();
        match target {
            Capture::Layer(l) => preview_composition(&rendered, HoverTarget::Layer(l)),
            Capture::Column(c) => preview_composition(&rendered, HoverTarget::Column(c)),
            Capture::Clip(l, c) => {
                let mut comp = rendered;
                let _ = comp.apply(Command::TriggerClip {
                    layer: l,
                    column: c,
                });
                preview_composition(&comp, HoverTarget::Layer(l))
            }
            Capture::Output | Capture::Hover | Capture::Ui | Capture::Source => rendered,
        }
    }

    /// Advances pending captures by one frame and answers the finished ones.
    /// Called after the frame has rendered.
    pub(super) fn finish_mcp_captures(&mut self) {
        if self.mcp.captures.is_empty() {
            self.mcp.offscreen = None;
            return;
        }
        let render_state = self.render_state.clone();
        let (device, queue) = (&render_state.device, &render_state.queue);
        // One offscreen target renders at a time; the others wait their turn.
        let current = self
            .mcp
            .captures
            .iter()
            .find(|c| c.target.offscreen())
            .map(|c| (c.target, c.width, c.source.clone()));
        if let Some((target, width, source)) = &current {
            let (target, width) = (*target, *width);
            let comp_size = (self.comp.width.max(1), self.comp.height.max(1));
            let height = (width as f32 * comp_size.1 as f32 / comp_size.0 as f32).round() as u32;
            let size = (width, height.max(1));
            if self
                .mcp
                .offscreen
                .as_ref()
                .is_none_or(|o| o.target != target || o.size != size || &o.source != source)
            {
                // The libraries come from the main renderer: no disk reads.
                let mut renderer = Renderer::new(device, size);
                *renderer.library_mut() = self.renderer.library().clone();
                *renderer.milkdrop_mut().library_mut() = self.renderer.milkdrop().library().clone();
                self.mcp.offscreen = Some(Offscreen {
                    target,
                    source: source.clone(),
                    size,
                    renderer,
                });
            }
            let mut comp = self.offscreen_frame(target, source.as_ref());
            (comp.width, comp.height) = size;
            let views = match target {
                Capture::Column(c) | Capture::Clip(_, c) => self.media.column_views(&self.comp, c),
                Capture::Source => vec![None],
                _ => self.media.layer_views(&self.comp),
            };
            let time = self.start.elapsed().as_secs_f32();
            let mut encoder = device.create_command_encoder(&Default::default());
            let offscreen = self.mcp.offscreen.as_mut().expect("just made");
            offscreen
                .renderer
                .render(device, queue, &mut encoder, &comp, &views, time);
            drop(views);
            queue.submit([encoder.finish()]);
        }

        let mut waiting = Vec::new();
        for mut capture in std::mem::take(&mut self.mcp.captures) {
            let rendering = !capture.target.offscreen()
                || current.as_ref().is_some_and(|(target, width, source)| {
                    (*target, *width, source) == (capture.target, capture.width, &capture.source)
                });
            if rendering {
                capture.frames_left = capture.frames_left.saturating_sub(1);
            }
            if capture.frames_left > 0 || !rendering {
                waiting.push(capture);
                continue;
            }
            let texture = match capture.target {
                Capture::Output => Some(self.renderer.output_texture()),
                Capture::Hover => self
                    .hover_target
                    .and_then(|t| self.hover_preview.renderer(t))
                    .map(Renderer::output_texture),
                _ => self
                    .mcp
                    .offscreen
                    .as_ref()
                    .map(|o| o.renderer.output_texture()),
            };
            let result = texture
                .ok_or_else(|| "nothing rendered for that target yet".to_owned())
                .and_then(|t| read_texture(device, queue, t))
                .and_then(|image| {
                    let (w, h) = image.dimensions();
                    png(image, capture.width).map(|bytes| (bytes, w, h))
                });
            // A black image is easier to read with the reason next to it.
            let layers = &self.comp.layers;
            let empty = match capture.target {
                Capture::Layer(l) => layers.get(l).is_none_or(|l| l.active.is_none()),
                Capture::Column(c) => layers
                    .iter()
                    .all(|l| l.clips.get(c).is_none_or(Option::is_none)),
                Capture::Clip(l, c) => layers
                    .get(l)
                    .and_then(|l| l.clips.get(c))
                    .is_none_or(Option::is_none),
                Capture::Output | Capture::Hover | Capture::Ui | Capture::Source => false,
            };
            let output = match result {
                Ok((bytes, w, h)) => ToolOutput::png(
                    bytes,
                    format!(
                        "{}, rendered at {w}x{h}{}",
                        capture.target.label(),
                        if empty {
                            " (nothing to play there)"
                        } else {
                            ""
                        }
                    ),
                ),
                Err(e) => ToolOutput::error(e),
            };
            capture.call.reply(output);
        }
        self.mcp.captures = waiting;
    }
}
