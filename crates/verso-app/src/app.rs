use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use eframe::egui::{self, Color32, TextureId};
use eframe::{egui_wgpu, wgpu};
use verso_core::output::InputSource;
use verso_core::{Command, CommandKind, Composition, Source};
use verso_render::{Renderer, Thumbnails};

use crate::dialogs::{DialogKind, Dialogs};
use crate::media::Media;
use crate::theme;

mod mcp;

/// Snapshots kept for undo.
const UNDO_DEPTH: usize = 50;
/// Status messages kept in the log.
const LOG_LINES: usize = 200;
/// Unsaved changes are copied to a `.verso.bak` file this often.
const AUTOSAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(120);

/// What to do once the user has answered the "unsaved changes" question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum After {
    New,
    Open,
    Quit,
}

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
    dirs.extend(crate::online_ui::shaders_dir());
    dirs
}

pub struct VersoApp {
    pub(crate) comp: Composition,
    pub(crate) renderer: Renderer,
    cell_thumbs: Thumbnails<(usize, usize)>,
    pub(crate) browser_thumbs: crate::source_browser::BrowserThumbs,
    clip_monitor: Thumbnails<()>,
    pub(crate) render_state: egui_wgpu::RenderState,
    pub(crate) output_texture: TextureId,
    /// Output size the egui texture was registered with.
    output_texture_size: (u32, u32),
    pub(crate) cell_textures: HashMap<(usize, usize), TextureId>,
    /// Screen rectangle of every clip cell drawn this frame, for file drops.
    pub(crate) cell_rects: Vec<(egui::Rect, (usize, usize))>,
    /// Clip under the pointer in the grid, shown in the Preview monitor.
    pub(crate) hovered_clip: Option<(usize, usize)>,
    /// Layer or column header under the pointer, shown in the Preview monitor.
    pub(crate) hover_target: Option<crate::hover_preview::HoverTarget>,
    hover_preview: crate::hover_preview::HoverPreview,
    /// Browser source under the pointer (name, source), with `HoverTarget::Source`.
    pub(crate) hover_source: Option<(String, Source)>,
    pub(crate) monitor_texture: Option<TextureId>,
    pub(crate) media: Media,
    /// Layers whose clip was launched again since the last media update.
    restarted: Vec<usize>,
    pub(crate) files: crate::panels::FileBrowser,
    start: Instant,
    last_frame: Instant,
    /// Seconds since the previous frame.
    frame_dt: f64,
    path: Option<PathBuf>,
    /// Last autosave, or last save / open, after which a backup is pointless.
    last_autosave: Instant,
    pub(crate) selected_clip: Option<(usize, usize)>,
    pub(crate) selected_layer: usize,
    /// Layers picked with Ctrl/Shift-click, for grouping.
    pub(crate) selected_layers: std::collections::BTreeSet<usize>,
    /// Group picked by clicking its header.
    pub(crate) selected_group: Option<usize>,
    pub(crate) props_tab: PropsTab,
    /// Selection kind already brought forward in the workspace.
    pub(crate) last_props_tab: PropsTab,
    pub(crate) workspace: crate::workspace::Workspace,
    pub(crate) colors_open: bool,
    pub(crate) output_open: bool,
    pub(crate) status: String,
    pub(crate) controls: crate::control_ui::Controls,
    pub(crate) scripts: crate::scripts_ui::ScriptsState,
    pub(crate) outputs: crate::outputs::Outputs,
    pub(crate) advanced: crate::advanced_output_ui::AdvancedOutput,
    pub(crate) online: crate::online_ui::OnlineState,
    pub(crate) milk: crate::milkdrop_ui::MilkState,
    /// MCP server, when started with `--mcp`.
    mcp: mcp::McpState,
    /// Commands collected while drawing, applied at the end of the frame.
    pending: Vec<Command>,
    /// Whether the composition changed since it was saved or opened.
    pub(crate) dirty: bool,
    undo: Vec<Composition>,
    redo: Vec<Composition>,
    /// Question shown when an action would lose unsaved changes.
    confirm: Option<After>,
    /// Action to run once the save in progress succeeds.
    after_save: Option<After>,
    /// Set when the user chose to quit: the next close request goes through.
    quitting: bool,
    /// File dialogs running on their own thread.
    pub(crate) dialogs: Dialogs,
    /// Status messages with the time they appeared, newest last.
    pub(crate) log: VecDeque<(Instant, String)>,
    last_status: String,
    pub(crate) log_open: bool,
    title: String,
    /// Resolution being typed or dragged, applied when the edit ends.
    pub(crate) edit_size: Option<(u32, u32)>,
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
        let milk_errors: Vec<String> = crate::milkdrop_ui::milkdrop_dirs()
            .iter()
            .flat_map(|dir| renderer.milkdrop_mut().library_mut().load_dir(dir))
            .collect();
        let output_texture = render_state.renderer.write().register_native_texture(
            device,
            renderer.output_view(),
            wgpu::FilterMode::Linear,
        );
        theme::load(&cc.egui_ctx);
        let ffmpeg = verso_media::ffmpeg_available();
        let (mcp, mcp_message) = mcp::McpState::start(&cc.egui_ctx);
        let mut app = Self {
            output_texture_size: renderer.size(),
            cell_thumbs: Thumbnails::new(device, CELL_THUMB),
            browser_thumbs: crate::source_browser::BrowserThumbs::new(device),
            clip_monitor: Thumbnails::new(device, MONITOR),
            comp,
            renderer,
            render_state,
            output_texture,
            cell_textures: HashMap::new(),
            cell_rects: Vec::new(),
            hovered_clip: None,
            hover_target: None,
            hover_preview: Default::default(),
            hover_source: None,
            monitor_texture: None,
            media: Media::new(),
            restarted: Vec::new(),
            files: Default::default(),
            start: Instant::now(),
            last_frame: Instant::now(),
            frame_dt: 0.0,
            path: None,
            last_autosave: Instant::now(),
            selected_clip: None,
            selected_layer: 0,
            selected_layers: Default::default(),
            selected_group: None,
            props_tab: PropsTab::Composition,
            last_props_tab: PropsTab::Composition,
            workspace: crate::workspace::Workspace::load(),
            colors_open: false,
            output_open: false,
            status: effect_errors
                .first()
                .or(milk_errors.first())
                .cloned()
                .unwrap_or_default(),
            pending: Vec::new(),
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            confirm: None,
            after_save: None,
            quitting: false,
            dialogs: Dialogs::default(),
            log: VecDeque::new(),
            last_status: String::new(),
            log_open: false,
            title: String::new(),
            edit_size: None,
            controls: Default::default(),
            scripts: Default::default(),
            outputs: Default::default(),
            advanced: Default::default(),
            online: Default::default(),
            milk: Default::default(),
            mcp,
        };
        if let Some(message) = mcp_message {
            app.status = message;
        }
        if !ffmpeg {
            app.status = "FFmpeg not found: videos will not play. Install FFmpeg (ffmpeg and ffprobe on PATH) or set VERSO_FFMPEG and VERSO_FFPROBE.".into();
        }
        Ok(app)
    }

    /// Clip shown in the Preview monitor: the one under the pointer, else the
    /// selected one.
    /// Composition the undo stack and the dirty flag compare against.
    fn remember_for_undo(&mut self) {
        self.undo.push(self.comp.clone());
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Restores `snapshot`, keeping what plays now so the show does not jump.
    fn restore(&mut self, mut snapshot: Composition) -> Composition {
        for (restored, current) in snapshot.layers.iter_mut().zip(&self.comp.layers) {
            if let Some(column) = current.active
                && restored.clips.get(column).is_some_and(Option::is_some)
            {
                restored.active = Some(column);
            }
            restored.launches = current.launches.clone();
        }
        let previous = std::mem::replace(&mut self.comp, snapshot);
        self.dirty = true;
        self.pending.clear();
        previous
    }

    pub(crate) fn undo(&mut self) {
        match self.undo.pop() {
            Some(snapshot) => {
                let previous = self.restore(snapshot);
                self.redo.push(previous);
                self.status = "Undo".into();
            }
            None => self.status = "Nothing to undo.".into(),
        }
    }

    pub(crate) fn redo(&mut self) {
        match self.redo.pop() {
            Some(snapshot) => {
                let previous = self.restore(snapshot);
                self.undo.push(previous);
                self.status = "Redo".into();
            }
            None => self.status = "Nothing to redo.".into(),
        }
    }

    /// Draws the hovered layer or column for the Preview monitor.
    fn render_hover_preview(
        &mut self,
        encoder: &mut eframe::wgpu::CommandEncoder,
        rendered: &Composition,
        time: f32,
    ) {
        use crate::hover_preview::HoverTarget;
        let Some(target) = self.hover_target else {
            self.hover_preview.idle();
            return;
        };
        let source_comp;
        let (comp, views) = match target {
            HoverTarget::Layer(_) => (rendered, self.media.layer_views(&self.comp)),
            HoverTarget::Column(column) | HoverTarget::Clip(_, column) => {
                (rendered, self.media.column_views(&self.comp, column))
            }
            HoverTarget::Source => {
                let Some((name, source)) = &self.hover_source else {
                    return;
                };
                let mut comp = crate::hover_preview::source_composition(source, name);
                (comp.width, comp.height) = (self.comp.width, self.comp.height);
                source_comp = comp;
                (&source_comp, Vec::new())
            }
        };
        self.hover_preview.render(
            &self.render_state,
            encoder,
            comp,
            target,
            &views,
            time,
            |renderer| {
                for dir in effect_dirs() {
                    renderer.library_mut().load_dir(&dir);
                }
                for dir in crate::milkdrop_ui::milkdrop_dirs() {
                    renderer.milkdrop_mut().library_mut().load_dir(&dir);
                }
            },
        );
    }

    /// Texture and caption of the hover preview, when a header is hovered.
    pub(crate) fn hover_preview(&self) -> Option<(eframe::egui::TextureId, String)> {
        use crate::hover_preview::HoverTarget;
        let target = self.hover_target?;
        let texture = self.hover_preview.texture(target)?;
        let caption = match target {
            HoverTarget::Layer(l) => {
                let layer = self.comp.layers.get(l)?;
                if layer.active_clip().is_some() {
                    layer.name.clone()
                } else {
                    format!("{} · nothing playing", layer.name)
                }
            }
            HoverTarget::Column(c) => self.comp.columns.get(c)?.name.clone(),
            HoverTarget::Clip(l, c) => self.comp.layers.get(l)?.clips.get(c)?.as_ref()?.name.clone(),
            HoverTarget::Source => self.hover_source.as_ref()?.0.clone(),
        };
        Some((texture, caption))
    }

    /// The pointer is over clip slot (`layer`, `column`). MilkDrop clips
    /// preview live; other sources use the clip monitor.
    pub(crate) fn hover_clip(&mut self, layer: usize, column: usize) {
        self.hovered_clip = Some((layer, column));
        let milkdrop = self
            .comp
            .layers
            .get(layer)
            .and_then(|l| l.clips.get(column))
            .and_then(Option::as_ref)
            .is_some_and(|clip| matches!(clip.source, Source::Milkdrop { .. }));
        if milkdrop {
            self.hover_target = Some(crate::hover_preview::HoverTarget::Clip(layer, column));
        }
    }

    pub(crate) fn preview_slot(&self) -> Option<(usize, usize)> {
        self.hovered_clip
            .filter(|&(l, c)| {
                self.comp
                    .layers
                    .get(l)
                    .and_then(|layer| layer.clips.get(c))
                    .is_some_and(Option::is_some)
            })
            .or(self.selected_clip)
    }

    /// Queues a change; it applies at the end of the frame.
    pub(crate) fn send(&mut self, command: Command) {
        self.pending.push(command);
    }

    /// Text of the window title bar: composition name and unsaved marker.
    fn window_title(&self) -> String {
        let marker = if self.dirty { "*" } else { "" };
        format!("{}{marker} – Verso", self.comp.name)
    }

    fn apply_pending(&mut self) {
        let (edits, changes) = self.pending.iter().map(Command::kind).fold(
            (false, false),
            |(edits, changes), kind| {
                (
                    edits || kind == CommandKind::Edit,
                    changes || kind != CommandKind::Play,
                )
            },
        );
        if edits {
            self.remember_for_undo();
        }
        if changes {
            self.dirty = true;
        }
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
        // Keep selections valid after layers, groups or columns change.
        let (layers, groups) = (self.comp.layers.len(), self.comp.groups.len());
        self.selected_layers.retain(|&l| l < layers);
        self.selected_group = self.selected_group.filter(|&g| g < groups);
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
        // A handle of its own, so `self` stays free for the passes below.
        let render_state = self.render_state.clone();
        let device = &render_state.device;
        let queue = &render_state.queue;
        let mut encoder = device.create_command_encoder(&Default::default());
        let time = self.start.elapsed().as_secs_f32();
        let dt = self.frame_dt;

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
        let (samples, rate) = self
            .controls
            .audio
            .as_ref()
            .map(|audio| audio.waveform())
            .unwrap_or_default();
        self.renderer
            .milkdrop_mut()
            .set_audio(samples, if rate > 0.0 { rate } else { 44100.0 });
        let rendered = self.modulated();
        // Group images are copied only when a live slice shows a group.
        let groups_shown = self.advanced.live
            && self
                .advanced
                .setup
                .screens
                .iter()
                .flat_map(|s| &s.slices)
                .any(|s| matches!(s.input_source(), InputSource::Group(_)));
        self.renderer.keep_group_images(groups_shown);
        let views = self.media.layer_views(&self.comp);
        self.renderer
            .render(device, queue, &mut encoder, &rendered, &views, time);
        drop(views);
        self.render_hover_preview(&mut encoder, &rendered, time);
        self.render_browser_thumbs(&mut encoder, time);

        let mut new_cells = Vec::new();
        for (l, layer) in self.comp.layers.iter().enumerate() {
            for (c, clip) in layer.clips.iter().enumerate() {
                let Some(clip) = clip else { continue };
                let created = match &clip.source {
                    // Generator shaders draw with the composition renderer.
                    Source::Shader(effect) => {
                        let (view, created) = self.cell_thumbs.target(device, (l, c));
                        self.renderer.render_shader_preview(
                            device,
                            queue,
                            &mut encoder,
                            effect,
                            view,
                            CELL_THUMB,
                            time,
                        );
                        created
                    }
                    source => {
                        self.cell_thumbs
                            .render(device, queue, &mut encoder, (l, c), source, time)
                    }
                };
                if created {
                    new_cells.push((l, c));
                }
            }
        }
        let selected_source = self
            .preview_slot()
            .and_then(|(l, c)| self.comp.layers.get(l)?.clips.get(c)?.as_ref())
            .map(|clip| clip.source.clone());
        let new_monitor = selected_source.is_some_and(|source| match &source {
            Source::Shader(effect) => {
                let (view, created) = self.clip_monitor.target(device, ());
                self.renderer.render_shader_preview(
                    device,
                    queue,
                    &mut encoder,
                    effect,
                    view,
                    MONITOR,
                    time,
                );
                created
            }
            source => self
                .clip_monitor
                .render(device, queue, &mut encoder, (), source, time),
        });
        let (new_screens, screen_errors) = self.outputs.render_screens(
            device,
            queue,
            &mut encoder,
            self.advanced.live.then_some(&self.advanced.setup),
            &self.renderer,
            &rendered,
        );
        if let Some(error) = screen_errors.last() {
            self.status = error.clone();
        }
        self.outputs.capture(device, &mut encoder, &self.renderer);
        queue.submit([encoder.finish()]);
        self.outputs.deliver(device);

        let mut egui_renderer = self.render_state.renderer.write();
        self.outputs
            .register_screens(&mut egui_renderer, device, &new_screens);
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
        let (mut commands, mut toggle_output, mut tap, mut learn) =
            (Vec::new(), false, false, false);
        let (mut group, mut ungroup) = (false, false);
        let shortcut = |key, shift| {
            egui::KeyboardShortcut::new(
                if shift {
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT
                } else {
                    egui::Modifiers::COMMAND
                },
                key,
            )
        };
        let (save, save_as, open, new, undo, redo) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&shortcut(egui::Key::S, false)),
                i.consume_shortcut(&shortcut(egui::Key::S, true)),
                i.consume_shortcut(&shortcut(egui::Key::O, false)),
                i.consume_shortcut(&shortcut(egui::Key::N, false)),
                i.consume_shortcut(&shortcut(egui::Key::Z, false)),
                i.consume_shortcut(&shortcut(egui::Key::Z, true))
                    || i.consume_shortcut(&shortcut(egui::Key::Y, false)),
            )
        });
        if save || save_as {
            self.save(save_as);
        }
        if open {
            self.request(After::Open);
        }
        if new {
            self.request(After::New);
        }
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
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
            tap = i.key_pressed(egui::Key::T) && !i.modifiers.command;
            learn = i.key_pressed(egui::Key::L) && !i.modifiers.command;
            let g = i.modifiers.command && i.key_pressed(egui::Key::G);
            group = g && !i.modifiers.shift;
            ungroup = g && i.modifiers.shift;
        });
        self.pending.extend(commands);
        self.output_open ^= toggle_output;
        if tap {
            self.tap_tempo();
        }
        if learn {
            self.toggle_learn();
        }
        if group {
            self.group_selection();
        }
        if ungroup {
            self.ungroup_selection();
        }
    }

    /// Saves to the current file, or asks for one. `save_as` always asks.
    pub(crate) fn save(&mut self, save_as: bool) {
        match (&self.path, save_as) {
            (Some(path), false) => {
                let path = path.clone();
                self.save_to(&path);
            }
            _ => {
                let name = format!("{}.verso", self.comp.name);
                self.dialogs.open(DialogKind::Save, move || {
                    rfd::FileDialog::new()
                        .add_filter("Verso composition", &["verso"])
                        .set_file_name(name)
                        .save_file()
                });
            }
        }
    }

    /// The composition file text, with the current window layout in it.
    fn document_text(&mut self) -> Result<String, String> {
        self.comp.layout = self.workspace.to_text();
        self.comp.to_ron().map_err(|e| e.to_string())
    }

    fn save_to(&mut self, path: &std::path::Path) {
        let result = self
            .document_text()
            .and_then(|text| std::fs::write(path, text).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                self.status = format!("Saved {}", path.display());
                self.path = Some(path.to_owned());
                self.dirty = false;
                self.last_autosave = Instant::now();
                let _ = std::fs::remove_file(path.with_extension("verso.bak"));
                if let Some(after) = self.after_save.take() {
                    self.proceed(after);
                }
            }
            Err(e) => {
                self.status = format!("Save failed: {e}");
                self.after_save = None;
            }
        }
    }

    fn open_file(&mut self, path: PathBuf) {
        let result = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| Composition::from_ron(&text).map_err(|e| e.to_string()));
        match result {
            Ok(comp) => self.replace_composition(comp, Some(path)),
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }

    /// Runs `after`, asking first when unsaved changes would be lost.
    fn request(&mut self, after: After) {
        if self.dirty {
            self.confirm = Some(after);
        } else {
            self.proceed(after);
        }
    }

    fn proceed(&mut self, after: After) {
        match after {
            After::New => self.replace_composition(Composition::new("Untitled", 3, 8), None),
            After::Open => self.dialogs.open(DialogKind::Open, || {
                rfd::FileDialog::new()
                    .add_filter("Verso composition", &["verso"])
                    .pick_file()
            }),
            After::Quit => self.quitting = true,
        }
    }

    fn replace_composition(&mut self, comp: Composition, path: Option<PathBuf>) {
        self.status = match &path {
            Some(path) => format!("Opened {}", path.display()),
            None => String::new(),
        };
        // A file brings back the panel layout it was saved with; a new
        // composition keeps the current one.
        if let Some(layout) = comp
            .layout
            .as_deref()
            .and_then(crate::workspace::Workspace::from_text)
        {
            self.workspace = layout;
        }
        self.comp = comp;
        self.path = path;
        self.selected_clip = None;
        self.selected_layer = 0;
        self.selected_layers.clear();
        self.selected_group = None;
        self.pending.clear();
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
        self.last_autosave = Instant::now();
        self.edit_size = None;
    }

    /// Where unsaved changes are backed up: next to the composition file, or
    /// in the user directory for a composition that has no file yet.
    fn autosave_path(&self) -> Option<PathBuf> {
        match &self.path {
            Some(path) => Some(path.with_extension("verso.bak")),
            None => crate::online_ui::user_dir().map(|d| d.join("Untitled.verso.bak")),
        }
    }

    /// Writes the backup of a modified composition. Failures only go to the
    /// log: a backup must never interrupt a set.
    fn autosave(&mut self) {
        self.last_autosave = Instant::now();
        let Some(path) = self.autosave_path() else {
            return;
        };
        let result = self
            .document_text()
            .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
        match result {
            Ok(()) => log::info!("autosaved to {}", path.display()),
            Err(e) => log::warn!("autosave to {} failed: {e}", path.display()),
        }
    }

    /// Results of file dialogs that finished.
    fn poll_dialogs(&mut self) {
        for (kind, path) in self.dialogs.poll() {
            match (kind, path) {
                (DialogKind::Save, Some(path)) => self.save_to(&path),
                (DialogKind::Save, None) => self.after_save = None,
                (DialogKind::Open, Some(path)) => self.open_file(path),
                (DialogKind::AdvancedImport, Some(path)) => self.import_advanced_output(&path),
                (DialogKind::AdvancedExport, Some(path)) => self.export_advanced_output(&path),
                (DialogKind::AdvancedSvg, Some(path)) => self.import_svg(&path),
                (DialogKind::MediaFolder, Some(folder)) => self.files.open(folder),
                (DialogKind::EffectFolder, Some(folder)) => {
                    let errors = self.renderer.library_mut().load_dir(&folder);
                    self.status = match errors.first() {
                        Some(first) => format!("{} effect(s) failed. {first}", errors.len()),
                        None => format!("Loaded effects from {}", folder.display()),
                    };
                }
                (_, None) => {}
            }
        }
    }

    /// "Save changes?" question before New, Open or Quit.
    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(after) = self.confirm else { return };
        let verb = match after {
            After::New => "starting a new composition",
            After::Open => "opening another composition",
            After::Quit => "quitting",
        };
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("unsaved")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Unsaved changes");
            ui.label(format!(
                "\"{}\" has changes that are not saved. Save them before {verb}?",
                self.comp.name
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(true);
                }
                if ui.button("Don't Save").clicked() {
                    choice = Some(false);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        self.confirm = None;
                    }
                });
            });
        });
        match choice {
            Some(true) => {
                self.confirm = None;
                self.after_save = Some(after);
                self.save(false);
            }
            Some(false) => {
                self.confirm = None;
                self.proceed(after);
            }
            None => {}
        }
    }

    /// Keeps the status log and the window title current.
    fn housekeeping(&mut self, ctx: &egui::Context) {
        if self.status != self.last_status {
            self.last_status = self.status.clone();
            if !self.status.is_empty() {
                log::info!("{}", self.status);
                self.log.push_back((Instant::now(), self.status.clone()));
                if self.log.len() > LOG_LINES {
                    self.log.pop_front();
                }
            }
        }
        if self.dirty && self.last_autosave.elapsed() >= AUTOSAVE_INTERVAL {
            self.autosave();
        }
        let title = self.window_title();
        if title != self.title {
            self.title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(After::Quit);
        }
        if self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn log_window(&mut self, ctx: &egui::Context) {
        let mut open = self.log_open;
        egui::Window::new("Messages")
            .open(&mut open)
            .default_size([520.0, 300.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Clear").clicked() {
                        self.log.clear();
                    }
                    if let Some(path) = crate::logging::path() {
                        ui.label(
                            egui::RichText::new(format!("Log file: {}", path.display()))
                                .color(theme::text_dim()),
                        )
                        .on_hover_text(
                            "Every message shown here, warnings from the GPU and window \
                             layers, and any crash.",
                        );
                    }
                });
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let now = Instant::now();
                        for (when, text) in &self.log {
                            let age = now.duration_since(*when).as_secs();
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("{age:>4}s ago"))
                                        .monospace()
                                        .color(theme::text_dim()),
                                );
                                ui.label(text);
                            });
                        }
                    });
            });
        self.log_open = open;
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.label(egui::RichText::new("VERSO").strong().color(theme::accent()));
            ui.add_space(6.0);
            ui.menu_button("Composition", |ui| {
                if ui
                    .add(egui::Button::new("New").shortcut_text("Ctrl+N"))
                    .clicked()
                {
                    self.request(After::New);
                    ui.close();
                }
                if ui
                    .add(egui::Button::new("Open…").shortcut_text("Ctrl+O"))
                    .clicked()
                {
                    self.request(After::Open);
                    ui.close();
                }
                if ui
                    .add(egui::Button::new("Save").shortcut_text("Ctrl+S"))
                    .clicked()
                {
                    self.save(false);
                    ui.close();
                }
                if ui
                    .add(egui::Button::new("Save As…").shortcut_text("Ctrl+Shift+S"))
                    .clicked()
                {
                    self.save(true);
                    ui.close();
                }
                if let Some(path) = &self.path {
                    ui.label(
                        egui::RichText::new(path.display().to_string())
                            .small()
                            .color(theme::text_dim()),
                    );
                }
                ui.separator();
                if ui.button("Clear All Layers (Esc)").clicked() {
                    self.send(Command::ClearAll);
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(
                        !self.undo.is_empty(),
                        egui::Button::new("Undo").shortcut_text("Ctrl+Z"),
                    )
                    .on_disabled_hover_text("Undo covers clips, layers, columns, groups, effect chains and mappings; not fader moves.")
                    .clicked()
                {
                    self.undo();
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !self.redo.is_empty(),
                        egui::Button::new("Redo").shortcut_text("Ctrl+Shift+Z"),
                    )
                    .clicked()
                {
                    self.redo();
                    ui.close();
                }
            });
            ui.menu_button("Layer", |ui| {
                if ui.button("New Layer").clicked() {
                    self.send(Command::AddLayer);
                }
                let name = self
                    .comp
                    .layers
                    .get(self.selected_layer)
                    .map(|l| l.name.clone())
                    .unwrap_or_default();
                if ui
                    .add_enabled(
                        self.comp.layers.len() > 1,
                        egui::Button::new(format!("Delete {name}")),
                    )
                    .on_hover_text("Deletes the selected layer and its clips (Ctrl+Z undoes)")
                    .clicked()
                {
                    self.send(Command::RemoveLayer(self.selected_layer));
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(
                        self.can_group(),
                        egui::Button::new("Group Selected Layers").shortcut_text("Ctrl+G"),
                    )
                    .on_disabled_hover_text("Ctrl/Shift-click adjacent layer headers first")
                    .clicked()
                {
                    self.group_selection();
                }
                if ui
                    .add_enabled(
                        self.current_group().is_some(),
                        egui::Button::new("Ungroup").shortcut_text("Ctrl+Shift+G"),
                    )
                    .clicked()
                {
                    self.ungroup_selection();
                }
            });
            ui.menu_button("Column", |ui| {
                if ui.button("New Column").clicked() {
                    self.send(Command::AddColumn);
                }
            });
            ui.menu_button("Output", |ui| {
                ui.checkbox(&mut self.output_open, "Output Window (F2)");
                ui.checkbox(&mut self.advanced.window_open, "Advanced Output…");
                self.outputs_menu(ui);
            });
            ui.menu_button("Window", |ui| self.window_menu(ui));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let fps = 1.0 / ui.input(|i| i.stable_dt).max(1e-3);
                if self.dialogs.busy() {
                    ui.spinner().on_hover_text("A file dialog is open");
                }
                ui.label(
                    egui::RichText::new(format!("{fps:.0} FPS"))
                        .color(theme::text_dim())
                        .monospace(),
                );
                ui.separator();
                let name = format!("{}{}", self.comp.name, if self.dirty { " *" } else { "" });
                ui.label(egui::RichText::new(name).color(theme::text_dim()))
                    .on_hover_text(if self.dirty {
                        "Unsaved changes (Ctrl+S)"
                    } else {
                        "Saved"
                    });
                ui.separator();
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    self.control_bar(ui);
                });
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
        let now = Instant::now();
        self.frame_dt = (now - self.last_frame).as_secs_f64();
        self.last_frame = now;
        self.handle_keys(ctx);
        self.poll_controls(self.frame_dt);
        self.poll_mcp();
        self.run_scripts(self.frame_dt);
        self.apply_pending();
        self.render();
        self.finish_mcp_captures();
        ctx.request_repaint();
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.mcp_raw_input(raw_input);
    }

    fn on_exit(&mut self) {
        self.workspace.save();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let bar = egui::Frame::NONE
            .fill(theme::bg_header())
            .inner_margin(egui::Margin::symmetric(6, 3));
        egui::Panel::top("menu")
            .frame(bar)
            .show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").frame(bar).show(ui, |ui| {
            ui.horizontal(|ui| {
                let age = self
                    .log
                    .back()
                    .map(|(when, _)| when.elapsed().as_secs_f32())
                    .unwrap_or(f32::MAX);
                let color = if age < 8.0 { theme::text() } else { theme::text_dim() };
                if ui
                    .add(egui::Label::new(egui::RichText::new(&self.status).color(color)).sense(egui::Sense::click()))
                    .on_hover_text("Click to open the message log")
                    .clicked()
                {
                    self.log_open = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(
                            "1–9 column · Esc clear · F2 output · drag sources onto slots · right-click slot",
                        )
                        .color(theme::text_dim()),
                    );
                });
            });
        });
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::NONE.fill(theme::bg_deep()))
            .show(ui, |ui| self.workspace(ui));

        let ctx = ui.ctx().clone();
        self.poll_dialogs();
        self.handle_dropped_files(&ctx);
        self.confirm_window(&ctx);
        self.log_window(&ctx);
        self.housekeeping(&ctx);
        self.control_window(&ctx);
        self.scripts_window(&ctx);
        self.advanced_output_window(&ctx);
        theme::settings_window(&ctx, &mut self.colors_open);
        self.output_window(&ctx);
        self.screen_windows(&ctx);
        self.mcp_after_ui(&ctx);
    }
}
