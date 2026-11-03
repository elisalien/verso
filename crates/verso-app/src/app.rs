use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use eframe::egui::{self, Color32, TextureId};
use eframe::{egui_wgpu, wgpu};
use verso_core::output::InputSource;
use verso_core::{Clip, Command, CommandKind, Composition, IndexChanges, Source};
use verso_render::{Renderer, Thumbnails};

use crate::dialogs::{FileDialogs, FileRequest};
use crate::history::History;
use crate::media::Media;
use crate::prefs::{Prefs, recovery};
use crate::theme;

mod mcp;

/// Status messages kept in the log.
const LOG_LINES: usize = 200;

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
    /// Name of the browser source hovered last frame: panels drawn before the
    /// browser caption the preview with it.
    pub(crate) last_hover_name: Option<String>,
    pub(crate) monitor_texture: Option<TextureId>,
    pub(crate) media: Media,
    /// Layers whose clip was launched again since the last media update.
    restarted: Vec<usize>,
    pub(crate) files: crate::panels::FileBrowser,
    pub(crate) start: Instant,
    last_frame: Instant,
    /// Seconds since the previous frame.
    frame_dt: f64,
    path: Option<PathBuf>,
    /// Last recovery copy, or last save / open, after which a copy is pointless.
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
    /// Title bar drawn by Verso or by the system.
    pub(crate) window: crate::title_bar::WindowSettings,
    pub(crate) output_open: bool,
    pub(crate) status: String,
    pub(crate) controls: crate::control_ui::Controls,
    pub(crate) scripts: crate::scripts_ui::ScriptsState,
    pub(crate) outputs: crate::outputs::Outputs,
    pub(crate) advanced: crate::advanced_output_ui::AdvancedOutput,
    pub(crate) online: crate::online_ui::OnlineState,
    pub(crate) milk: crate::milkdrop_ui::MilkState,
    /// MilkDrop Settings: values for all clips and shared options.
    pub(crate) milk_prefs: crate::milkdrop_settings::MilkPrefs,
    /// MCP server, when started with `--mcp`.
    mcp: mcp::McpState,
    /// Commands collected while drawing, applied at the end of the frame,
    /// with whether each is a user edit (undoable) rather than a live action.
    pending: Vec<(Command, bool)>,
    /// Whether the composition changed since it was saved or opened.
    pub(crate) dirty: bool,
    pub(crate) history: History,
    pub(crate) prefs: Prefs,
    /// While on, only live actions and performance controls change the
    /// composition: no accidental edits during a show.
    pub(crate) live_lock: bool,
    pub(crate) clipboard: Option<Clip>,
    /// Modal question: unsaved changes, crash recovery, missing media.
    pub(crate) dialog: Option<Dialog>,
    pub(crate) prefs_open: bool,
    /// Set once the user agreed to close with unsaved changes.
    quit_confirmed: bool,
    /// Native file dialogs, polled every frame.
    pub(crate) file_dialogs: FileDialogs,
    gpu: crate::logging::GpuState,
    /// Status messages with the time they appeared, newest last.
    pub(crate) log: VecDeque<(Instant, String)>,
    last_status: String,
    pub(crate) log_open: bool,
    title: String,
    /// Resolution being typed or dragged, applied when the edit ends.
    pub(crate) edit_size: Option<(u32, u32)>,
    /// Prints every command to stderr (`VERSO_TRACE=1`), for debugging.
    trace: bool,
}

/// Keyboard shortcuts, handled after input is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    Undo,
    Redo,
    Save,
    SaveAs,
    Open,
    New,
    DuplicateLayer,
    LiveLock,
    ClearAll,
    EscapeHint,
    Output,
    Tap,
    Learn,
    Copy,
    Cut,
    Paste,
    ClearCell,
    Group,
    Ungroup,
}

/// Commands a performer may still send while the live lock is on.
fn allowed_when_locked(command: &Command) -> bool {
    use verso_core::EffectEdit;
    command.kind() == CommandKind::Play
        || matches!(
            command,
            Command::SetLayerOpacity { .. }
                | Command::SetLayerBypass { .. }
                | Command::SetLayerSolo { .. }
                | Command::SetLayerFader { .. }
                | Command::SetGroupOpacity { .. }
                | Command::SetGroupBypass { .. }
                | Command::SetMasterOpacity(_)
                | Command::SetCrossfader(_)
                | Command::SetBpm(_)
                | Command::ResetLaunchCount { .. }
                | Command::EditEffects {
                    edit: EffectEdit::SetParam { .. } | EffectEdit::SetBypass { .. },
                    ..
                }
        )
}

/// What to do once the user decided about unsaved changes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AfterSave {
    New,
    Open,
    OpenPath(PathBuf),
    Quit,
}

/// A modal question.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Dialog {
    /// Unsaved changes would be lost by `AfterSave`.
    Unsaved(AfterSave),
    /// The previous run crashed with unsaved work.
    Recover,
    /// Media files of the opened composition are missing.
    Missing(usize),
}

/// Media files the composition uses that do not exist.
fn missing_media(comp: &Composition) -> Vec<PathBuf> {
    let mut missing: Vec<PathBuf> = comp
        .layers
        .iter()
        .flat_map(|l| l.clips.iter().flatten())
        .filter_map(|c| c.source.path())
        .filter(|p| !p.exists())
        .map(|p| p.to_owned())
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

/// Finds files by name under `dir`, a few levels deep.
fn find_files(
    dir: &std::path::Path,
    names: &[std::ffi::OsString],
) -> HashMap<std::ffi::OsString, PathBuf> {
    let mut found = HashMap::new();
    let mut stack = vec![(dir.to_owned(), 0)];
    let mut visited = 0;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > 50_000 {
                return found;
            }
            let path = entry.path();
            if path.is_dir() {
                if depth < 5 {
                    stack.push((path, depth + 1));
                }
            } else if let Some(name) = path.file_name()
                && names.iter().any(|n| n == name)
            {
                found.entry(name.to_owned()).or_insert(path);
            }
        }
    }
    found
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
        let gpu = crate::logging::watch_gpu(device);
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
        let prefs = Prefs::load();
        cc.egui_ctx.set_zoom_factor(prefs.ui_scale);
        let crashed = recovery::start();
        let reopen = (prefs.reopen_last && !crashed)
            .then(|| prefs.recent.first().cloned())
            .flatten()
            .filter(|p| p.is_file());
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
            last_hover_name: None,
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
            window: crate::title_bar::WindowSettings::load(),
            output_open: false,
            status: effect_errors
                .first()
                .or(milk_errors.first())
                .cloned()
                .unwrap_or_default(),
            pending: Vec::new(),
            dirty: false,
            history: History::default(),
            prefs,
            live_lock: false,
            clipboard: None,
            dialog: crashed.then_some(Dialog::Recover),
            prefs_open: false,
            quit_confirmed: false,
            file_dialogs: FileDialogs::default(),
            gpu,
            log: VecDeque::new(),
            last_status: String::new(),
            log_open: false,
            title: String::new(),
            edit_size: None,
            trace: std::env::var_os("VERSO_TRACE").is_some(),
            controls: Default::default(),
            scripts: Default::default(),
            outputs: Default::default(),
            advanced: Default::default(),
            online: Default::default(),
            milk: Default::default(),
            milk_prefs: crate::milkdrop_settings::MilkPrefs::load(),
            mcp,
        };
        if let Some(message) = mcp_message {
            app.status = message;
        }
        if !ffmpeg {
            app.status = "FFmpeg not found: videos will not play. Install FFmpeg (ffmpeg and ffprobe on PATH) or set VERSO_FFMPEG and VERSO_FFPROBE.".into();
        }
        if let Some(path) = reopen {
            app.open_path(path);
        }
        Ok(app)
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
        // Panels drawn before the one under the pointer see last frame's
        // hover, which is what the preview renderer drew.
        let target = self.hover_target.or(self.hover_preview.shown())?;
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
            HoverTarget::Clip(l, c) => self
                .comp
                .layers
                .get(l)?
                .clips
                .get(c)?
                .as_ref()?
                .name
                .clone(),
            HoverTarget::Source => self
                .hover_source
                .as_ref()
                .map(|(name, _)| name.clone())
                .or_else(|| self.last_hover_name.clone())?,
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

    /// Shows `message` in the status bar; it also goes to the message log
    /// and the log file.
    pub(crate) fn problem(&mut self, message: impl Into<String>) {
        let message = message.into();
        log::warn!("{message}");
        self.status = message;
    }

    /// Queues a change made in the interface; it applies at the end of the
    /// frame and can be undone (except live actions such as launching).
    pub(crate) fn send(&mut self, command: Command) {
        if self.live_lock && !allowed_when_locked(&command) {
            self.status = "Live lock is on: unlock (Ctrl+Shift+L) to edit".into();
            return;
        }
        self.pending.push((command, true));
    }

    /// Queues a live change from a controller, script or shortcut. Never
    /// recorded for undo and allowed under the live lock.
    pub(crate) fn send_live(&mut self, command: Command) {
        self.pending.push((command, false));
    }

    /// Text of the window title bar: composition name and unsaved marker.
    fn window_title(&self) -> String {
        let marker = if self.dirty { "*" } else { "" };
        format!("{}{marker} – Verso", self.comp.name)
    }

    fn apply_pending(&mut self) {
        for (command, edit) in std::mem::take(&mut self.pending) {
            if self.trace {
                eprintln!("command (edit: {edit}): {command:?}");
            }
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
            let changes = self.comp.index_changes(&command);
            let kind = command.kind();
            let undoable = edit && kind != CommandKind::Play;
            let before = undoable.then(|| self.history.snapshot(&self.comp, &command));
            match self.comp.apply(command) {
                Ok(()) => {
                    if let Some(Some(step)) = before {
                        self.history.push(step);
                    }
                    if kind != CommandKind::Play {
                        self.dirty = true;
                    }
                    if let Some(changes) = changes {
                        self.follow(&changes);
                    }
                }
                Err(e) => self.status = e.to_string(),
            }
        }
        self.fix_selection();
    }

    /// Moves state kept per layer or column outside the composition.
    fn follow(&mut self, changes: &IndexChanges) {
        self.renderer.follow(&self.render_state.device, changes);
        self.media.follow(changes);
        let map_layer = |l: usize| changes.layers.get(l).copied().flatten();
        let map_column = |c: usize| changes.columns.get(c).copied().flatten();
        if let Some(l) = map_layer(self.selected_layer) {
            self.selected_layer = l;
        }
        self.selected_clip = self
            .selected_clip
            .and_then(|(l, c)| Some((map_layer(l)?, map_column(c)?)));
    }

    pub(crate) fn undo(&mut self) {
        let label = self.history.undo_label().map(str::to_owned);
        if let Some(restored) = self.history.undo(&self.comp) {
            self.restore(restored);
            self.status = format!("Undo {}", label.unwrap_or_default());
        }
    }

    pub(crate) fn redo(&mut self) {
        let label = self.history.redo_label().map(str::to_owned);
        if let Some(restored) = self.history.redo(&self.comp) {
            self.restore(restored);
            self.status = format!("Redo {}", label.unwrap_or_default());
        }
    }

    /// Replaces the composition with a version from the history, keeping
    /// what is playing now: undo never launches or stops clips.
    fn restore(&mut self, mut restored: Composition) {
        for (layer, current) in restored.layers.iter_mut().zip(&self.comp.layers) {
            if layer.clips.len() == current.clips.len() {
                layer.active = current
                    .active
                    .filter(|&c| layer.clips.get(c).is_some_and(Option::is_some));
                layer.launches = current.launches.clone();
            }
        }
        self.comp = restored;
        self.dirty = true;
        self.fix_selection();
    }

    fn fix_selection(&mut self) {
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
        let milk = self.renderer.milkdrop_mut();
        milk.settings = self.milk_prefs.render_settings();
        milk.set_audio(samples, if rate > 0.0 { rate } else { 44100.0 });
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
        let outgoing = self.media.outgoing_views();
        self.renderer.render_with_outgoing(
            device,
            queue,
            &mut encoder,
            &rendered,
            &views,
            &outgoing,
            time,
        );
        drop((views, outgoing));
        for error in self.renderer.take_errors() {
            log::warn!("{error}");
            self.status = error;
        }
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
        self.outputs.highlight = self.advanced.highlighted_slice();
        self.outputs.patterns = self.advanced.active_patterns();
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
        drop(egui_renderer);
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.dialog.is_some() || self.file_dialogs.is_open() {
            return;
        }
        use egui::{Key, Modifiers};
        let digits = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        let columns = self.comp.columns.len();
        let mut live = Vec::new();
        let mut actions = Vec::new();
        ctx.input_mut(|i| {
            let mut shortcut = |i: &mut egui::InputState, modifiers, key, action: Shortcut| {
                if i.consume_key(modifiers, key) {
                    actions.push(action);
                }
            };
            let command = Modifiers::COMMAND;
            let shift_command = Modifiers::COMMAND | Modifiers::SHIFT;
            shortcut(i, shift_command, Key::Z, Shortcut::Redo);
            shortcut(i, command, Key::Y, Shortcut::Redo);
            shortcut(i, command, Key::Z, Shortcut::Undo);
            shortcut(i, shift_command, Key::S, Shortcut::SaveAs);
            shortcut(i, command, Key::S, Shortcut::Save);
            shortcut(i, command, Key::O, Shortcut::Open);
            shortcut(i, command, Key::N, Shortcut::New);
            shortcut(i, command, Key::D, Shortcut::DuplicateLayer);
            shortcut(i, shift_command, Key::L, Shortcut::LiveLock);
            shortcut(i, Modifiers::SHIFT, Key::Escape, Shortcut::ClearAll);
            shortcut(i, shift_command, Key::G, Shortcut::Ungroup);
            shortcut(i, command, Key::G, Shortcut::Group);
            for event in &i.events {
                match event {
                    egui::Event::Copy => actions.push(Shortcut::Copy),
                    egui::Event::Cut => actions.push(Shortcut::Cut),
                    egui::Event::Paste(_) => actions.push(Shortcut::Paste),
                    _ => {}
                }
            }
            if i.modifiers.is_none() || i.modifiers == Modifiers::SHIFT {
                // 1–9 launch columns 1–9; with Shift, columns 10–18.
                let page = if i.modifiers.shift { 9 } else { 0 };
                for (n, key) in digits.iter().enumerate() {
                    if i.key_pressed(*key) && page + n < columns {
                        live.push(Command::TriggerColumn(page + n));
                    }
                }
            }
            if i.modifiers.is_none() {
                for (key, action) in [
                    (Key::F2, Shortcut::Output),
                    (Key::T, Shortcut::Tap),
                    (Key::L, Shortcut::Learn),
                    (Key::Delete, Shortcut::ClearCell),
                    (Key::Backspace, Shortcut::ClearCell),
                    (Key::Escape, Shortcut::EscapeHint),
                ] {
                    if i.key_pressed(key) {
                        actions.push(action);
                    }
                }
            }
            if i.modifiers.is_none() && i.key_pressed(Key::Space) {
                live.push(Command::GoNextCue);
            }
        });
        for command in live {
            self.send_live(command);
        }
        for action in actions {
            self.shortcut(action);
        }
    }

    fn shortcut(&mut self, action: Shortcut) {
        match action {
            Shortcut::Undo => self.undo(),
            Shortcut::Redo => self.redo(),
            Shortcut::Save => self.save(false),
            Shortcut::SaveAs => self.save(true),
            Shortcut::Open => self.confirm_unsaved(AfterSave::Open),
            Shortcut::New => self.confirm_unsaved(AfterSave::New),
            Shortcut::DuplicateLayer => self.send(Command::DuplicateLayer(self.selected_layer)),
            Shortcut::LiveLock => {
                self.live_lock = !self.live_lock;
                self.status = if self.live_lock {
                    "Live lock on: editing is disabled".into()
                } else {
                    "Live lock off".into()
                };
            }
            Shortcut::ClearAll => self.send_live(Command::ClearAll),
            Shortcut::EscapeHint => {
                self.status = "Shift+Esc clears all layers".into();
            }
            Shortcut::Output => self.output_open ^= true,
            Shortcut::Tap => self.tap_tempo(),
            Shortcut::Learn => self.toggle_learn(),
            Shortcut::Copy => self.copy_clip(),
            Shortcut::Cut => {
                self.copy_clip();
                self.clear_selected_cell();
            }
            Shortcut::Paste => self.paste_clip(),
            Shortcut::ClearCell => self.clear_selected_cell(),
            Shortcut::Group => self.group_selection(),
            Shortcut::Ungroup => self.ungroup_selection(),
        }
    }

    pub(crate) fn copy_clip(&mut self) {
        let clip = self
            .selected_clip
            .and_then(|(l, c)| self.comp.layers.get(l)?.clips.get(c)?.clone());
        if let Some(clip) = clip {
            self.status = format!("Copied {}", clip.name);
            self.clipboard = Some(clip);
        }
    }

    pub(crate) fn paste_clip(&mut self) {
        if let (Some((layer, column)), Some(clip)) = (self.selected_clip, self.clipboard.clone()) {
            self.send(Command::SetClip {
                layer,
                column,
                clip: Some(clip),
            });
        }
    }

    pub(crate) fn clear_selected_cell(&mut self) {
        if let Some((layer, column)) = self.selected_clip {
            self.send(Command::SetClip {
                layer,
                column,
                clip: None,
            });
        }
    }

    /// Runs `then`, first asking what to do with unsaved changes.
    pub(crate) fn confirm_unsaved(&mut self, then: AfterSave) {
        if self.dirty {
            self.dialog = Some(Dialog::Unsaved(then));
        } else {
            self.after_save(then);
        }
    }

    fn after_save(&mut self, then: AfterSave) {
        match then {
            AfterSave::New => {
                self.replace_composition(Composition::new("Untitled", 3, 8), None);
            }
            AfterSave::Open => self.open(),
            AfterSave::OpenPath(path) => self.open_path(path),
            AfterSave::Quit => self.quit_confirmed = true,
        }
    }

    /// Saves to the current file, or asks for one; `then` runs once saved.
    fn save_then(&mut self, save_as: bool, then: Option<AfterSave>) {
        match (&self.path, save_as) {
            (Some(path), false) => {
                let path = path.clone();
                self.save_to(path);
                if let Some(then) = then.filter(|_| !self.dirty) {
                    self.after_save(then);
                }
            }
            _ => self.file_request(FileRequest::SaveAs(then)),
        }
    }

    fn save(&mut self, save_as: bool) {
        self.save_then(save_as, None);
    }

    pub(crate) fn file_request(&mut self, request: FileRequest) {
        self.file_dialogs.start(request, &self.comp.name);
    }

    /// The composition file text, with the current window layout in it.
    fn document_text(&mut self) -> Result<String, String> {
        self.comp.layout = self.workspace.to_text();
        self.comp.to_ron().map_err(|e| e.to_string())
    }

    /// Acts on a file dialog that just closed.
    fn file_chosen(&mut self, request: FileRequest, path: Option<PathBuf>) {
        let Some(path) = path else { return };
        match request {
            FileRequest::SaveAs(then) => {
                self.save_to(path);
                if let Some(then) = then.filter(|_| !self.dirty) {
                    self.after_save(then);
                }
            }
            FileRequest::Open => self.open_path(path),
            FileRequest::Relink => self.relink_media(&path),
            FileRequest::FilesFolder => self.files.open(path),
            FileRequest::AdvancedImport => self.import_advanced_output(&path),
            FileRequest::AdvancedExport(_) => self.export_advanced_output(&path),
            FileRequest::AdvancedSvg => self.import_svg(&path),
            FileRequest::EffectsFolder => {
                let errors = self.renderer.library_mut().load_dir(&path);
                self.status = match errors.first() {
                    Some(first) => format!("{} effect(s) failed. {first}", errors.len()),
                    None => format!("Loaded effects from {}", path.display()),
                };
            }
        }
    }

    fn save_to(&mut self, path: PathBuf) {
        let result = self
            .document_text()
            .and_then(|text| write_atomic(&path, text.as_bytes()).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                self.status = format!("Saved {}", path.display());
                self.dirty = false;
                self.last_autosave = Instant::now();
                recovery::discard();
                self.prefs.add_recent(&path);
                self.path = Some(path);
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    fn open(&mut self) {
        self.file_request(FileRequest::Open);
    }

    pub(crate) fn open_path(&mut self, path: PathBuf) {
        let result = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| Composition::from_ron_with_fixes(&text).map_err(|e| e.to_string()));
        match result {
            Ok((comp, fixes)) => {
                self.prefs.add_recent(&path);
                self.replace_composition(comp, Some(path));
                let missing = missing_media(&self.comp).len();
                if missing > 0 {
                    self.dialog = Some(Dialog::Missing(missing));
                }
                if !fixes.is_empty() {
                    let status = format!("{} — {} problem(s) repaired", self.status, fixes.len());
                    for fix in fixes {
                        self.problem(format!("Repaired: {fix}"));
                    }
                    self.status = status;
                }
            }
            Err(e) => self.status = format!("Open failed: {e}"),
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
        self.history.clear();
        self.dirty = false;
        self.last_autosave = Instant::now();
        self.edit_size = None;
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
        if let Some((request, path)) = self.file_dialogs.poll() {
            self.file_chosen(request, path);
        }
        if let Some(reason) = self.gpu.lost() {
            let status = format!("GPU lost ({reason}): save and restart Verso");
            if self.status != status {
                self.status = status;
            }
        }
        self.autosave();
        let title = self.window_title();
        if title != self.title {
            self.title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty && !self.quit_confirmed {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.dialog = Some(Dialog::Unsaved(AfterSave::Quit));
        }
        if self.quit_confirmed {
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
        self.history.clear();
        self.dirty = false;
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let custom_title_bar = self.window.custom_title_bar;
        if custom_title_bar {
            crate::title_bar::drag_area(ui);
        }
        egui::MenuBar::new().ui(ui, |ui| {
            ui.label(egui::RichText::new("VERSO").strong().color(theme::accent()));
            ui.add_space(6.0);
            ui.menu_button("Composition", |ui| {
                if menu_item(ui, "New", "Ctrl+N").clicked() {
                    self.confirm_unsaved(AfterSave::New);
                }
                if menu_item(ui, "Open…", "Ctrl+O").clicked() {
                    self.confirm_unsaved(AfterSave::Open);
                }
                ui.add_enabled_ui(!self.prefs.recent.is_empty(), |ui| {
                    ui.menu_button("Open Recent", |ui| {
                        for path in self.prefs.recent.clone() {
                            let name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            if ui
                                .button(name)
                                .on_hover_text(path.display().to_string())
                                .clicked()
                            {
                                self.confirm_unsaved(AfterSave::OpenPath(path));
                            }
                        }
                        ui.separator();
                        if ui.button("Clear List").clicked() {
                            self.prefs.recent.clear();
                            self.prefs.save();
                        }
                    });
                });
                if menu_item(ui, "Save", "Ctrl+S").clicked() {
                    self.save(false);
                    ui.close();
                }
                if menu_item(ui, "Save As…", "Ctrl+Shift+S").clicked() {
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
                if ui.button("Preferences…").clicked() {
                    self.prefs_open = true;
                }
                ui.separator();
                if menu_item(ui, "Clear All Layers", "Shift+Esc").clicked() {
                    self.send_live(Command::ClearAll);
                }
            });
            ui.menu_button("Edit", |ui| {
                let undo = self.history.undo_label().map(|l| format!("Undo {l}"));
                if ui
                    .add_enabled(
                        undo.is_some(),
                        egui::Button::new(undo.unwrap_or("Undo".into())).shortcut_text("Ctrl+Z"),
                    )
                    .clicked()
                {
                    self.undo();
                }
                let redo = self.history.redo_label().map(|l| format!("Redo {l}"));
                if ui
                    .add_enabled(
                        redo.is_some(),
                        egui::Button::new(redo.unwrap_or("Redo".into()))
                            .shortcut_text("Ctrl+Shift+Z"),
                    )
                    .clicked()
                {
                    self.redo();
                }
                ui.separator();
                let has_clip = self
                    .selected_clip
                    .and_then(|(l, c)| self.comp.layers.get(l)?.clips.get(c)?.as_ref())
                    .is_some();
                if ui
                    .add_enabled(has_clip, egui::Button::new("Copy Clip").shortcut_text("Ctrl+C"))
                    .clicked()
                {
                    self.copy_clip();
                }
                let can_paste = self.clipboard.is_some() && self.selected_clip.is_some();
                if ui
                    .add_enabled(can_paste, egui::Button::new("Paste Clip").shortcut_text("Ctrl+V"))
                    .clicked()
                {
                    self.paste_clip();
                }
                if ui
                    .add_enabled(has_clip, egui::Button::new("Clear Cell").shortcut_text("Del"))
                    .clicked()
                {
                    self.clear_selected_cell();
                }
                ui.separator();
                let mut lock = self.live_lock;
                if ui
                    .checkbox(&mut lock, "Live Lock (Ctrl+Shift+L)")
                    .on_hover_text("Only launching and performance controls work: no accidental edits during a show")
                    .changed()
                {
                    self.shortcut(Shortcut::LiveLock);
                }
            });
            ui.menu_button("Layer", |ui| {
                if ui.button("New Layer").clicked() {
                    self.send(Command::AddLayer);
                }
                let layer = self.selected_layer;
                let count = self.comp.layers.len();
                ui.add_enabled_ui(layer < count, |ui| {
                    if menu_item(ui, "Duplicate Selected Layer", "Ctrl+D").clicked() {
                        self.send(Command::DuplicateLayer(layer));
                    }
                    if ui
                        .add_enabled(layer + 1 < count, egui::Button::new("Move Selected Layer Up"))
                        .clicked()
                    {
                        self.send(Command::MoveLayer {
                            from: layer,
                            to: layer + 1,
                        });
                    }
                    if ui
                        .add_enabled(layer > 0, egui::Button::new("Move Selected Layer Down"))
                        .clicked()
                    {
                        self.send(Command::MoveLayer {
                            from: layer,
                            to: layer - 1,
                        });
                    }
                    if ui
                        .add_enabled(count > 1, egui::Button::new("Delete Selected Layer"))
                        .on_hover_text("Ctrl+Z restores it")
                        .clicked()
                    {
                        self.send(Command::RemoveLayer(layer));
                    }
                });
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
                let column = self.selected_clip.map(|(_, c)| c);
                let count = self.comp.columns.len();
                ui.add_enabled_ui(column.is_some(), |ui| {
                    let c = column.unwrap_or(0);
                    if ui
                        .add_enabled(c > 0, egui::Button::new("Move Selected Column Left"))
                        .clicked()
                    {
                        self.send(Command::MoveColumn { from: c, to: c - 1 });
                    }
                    if ui
                        .add_enabled(c + 1 < count, egui::Button::new("Move Selected Column Right"))
                        .clicked()
                    {
                        self.send(Command::MoveColumn { from: c, to: c + 1 });
                    }
                    if ui
                        .add_enabled(count > 1, egui::Button::new("Delete Selected Column"))
                        .on_hover_text("Ctrl+Z restores it")
                        .clicked()
                    {
                        self.send(Command::RemoveColumn(c));
                    }
                })
                .response
                .on_disabled_hover_text("Select a cell of the column first");
            });
            ui.menu_button("Output", |ui| {
                ui.checkbox(&mut self.output_open, "Output Window (F2)");
                ui.checkbox(&mut self.advanced.window_open, "Advanced Output…");
                self.outputs_menu(ui);
            });
            ui.menu_button("Window", |ui| self.window_menu(ui));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if custom_title_bar {
                    crate::title_bar::buttons(ui);
                    ui.separator();
                }
                let fps = 1.0 / ui.input(|i| i.stable_dt).max(1e-3);
                if self.file_dialogs.is_open() {
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
                if self.live_lock {
                    ui.label(
                        egui::RichText::new(" LOCKED ")
                            .strong()
                            .color(Color32::BLACK)
                            .background_color(theme::solo()),
                    )
                    .on_hover_text("Live lock: editing disabled (Ctrl+Shift+L)");
                }
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
        self.comp.advance_cues(self.frame_dt as f32);
        self.render();
        self.finish_mcp_captures();
        ctx.request_repaint();
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.mcp_raw_input(raw_input);
    }

    fn on_exit(&mut self) {
        self.workspace.save();
        self.prefs.save();
        recovery::finish();
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
                            "1–9 column · Shift+Esc clear · F2 output · Ctrl+Z undo · right-click for more",
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
        self.handle_dropped_files(&ctx);
        self.dialog_window(&ctx);
        self.prefs_window(&ctx);
        self.log_window(&ctx);
        self.housekeeping(&ctx);
        self.control_window(&ctx);
        self.scripts_window(&ctx);
        self.advanced_output_window(&ctx);
        theme::settings_window(&ctx, &mut self.colors_open);
        self.output_window(&ctx);
        self.screen_windows(&ctx);
        self.mcp_after_ui(&ctx);
        // Last, so the resize cursor wins over the widget under the edge.
        if self.window.custom_title_bar {
            crate::title_bar::resize_edges(&ctx);
        }
    }
}

/// A menu button with its shortcut shown on the right.
fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> egui::Response {
    ui.add(egui::Button::new(label).shortcut_text(shortcut))
}

/// Seconds between recovery copies of unsaved work.
const AUTOSAVE_SECONDS: u64 = 60;

impl VersoApp {
    fn autosave(&mut self) {
        if !self.prefs.autosave
            || !self.dirty
            || self.last_autosave.elapsed().as_secs() < AUTOSAVE_SECONDS
        {
            return;
        }
        self.last_autosave = Instant::now();
        let Some(path) = recovery::autosave_path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Failures only go to the log: a recovery copy must never interrupt
        // a set.
        let result = self
            .document_text()
            .and_then(|text| write_atomic(&path, text.as_bytes()).map_err(|e| e.to_string()));
        match result {
            Ok(()) => log::info!("recovery copy written to {}", path.display()),
            Err(e) => log::warn!("recovery copy to {} failed: {e}", path.display()),
        }
    }

    fn relink_media(&mut self, dir: &std::path::Path) {
        let missing = missing_media(&self.comp);
        let names: Vec<_> = missing
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_owned())
            .collect();
        let found = find_files(dir, &names);
        let mut relinked = 0;
        for layer in &mut self.comp.layers {
            for clip in layer.clips.iter_mut().flatten() {
                let path = match &mut clip.source {
                    Source::Image { path } | Source::Video { path, .. } => path,
                    _ => continue,
                };
                if path.exists() {
                    continue;
                }
                if let Some(new) = path.file_name().and_then(|n| found.get(n)) {
                    *path = new.clone();
                    relinked += 1;
                }
            }
        }
        if relinked > 0 {
            self.dirty = true;
        }
        let still = missing_media(&self.comp).len();
        self.status = format!("Relinked {relinked} clip(s); {still} file(s) still missing");
        if still > 0 {
            self.problem(format!("{still} media file(s) still missing"));
        }
    }

    fn dialog_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("verso-dialog")).show(ctx, |ui| {
            ui.set_width(380.0);
            match &dialog {
                Dialog::Unsaved(_) => {
                    ui.heading("Unsaved changes");
                    ui.label(format!(
                        "Save the changes to “{}” first?",
                        self.comp.name
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            choice = Some(1);
                        }
                        if ui.button("Don't Save").clicked() {
                            choice = Some(2);
                        }
                        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            choice = Some(0);
                        }
                    });
                }
                Dialog::Missing(count) => {
                    ui.heading("Missing media");
                    ui.label(format!(
                        "{count} media file(s) of this composition cannot be found. Pick a folder to search them by name (sub-folders included)."
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Locate Folder…").clicked() {
                            choice = Some(1);
                        }
                        if ui.button("Skip").clicked() {
                            choice = Some(2);
                        }
                    });
                }
                Dialog::Recover => {
                    ui.heading("Recover unsaved work?");
                    ui.label("Verso did not close properly last time. A copy of the unsaved composition was kept.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Recover").clicked() {
                            choice = Some(1);
                        }
                        if ui.button("Discard").clicked() {
                            choice = Some(2);
                        }
                    });
                }
            }
        });
        let Some(choice) = choice else { return };
        self.dialog = None;
        match (dialog, choice) {
            (Dialog::Unsaved(then), 1) => self.save_then(false, Some(then)),
            (Dialog::Unsaved(then), 2) => self.after_save(then),
            (Dialog::Recover, 1) => {
                let recovered = recovery::autosave_path()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .map(|text| Composition::from_ron_with_fixes(&text));
                match recovered {
                    Some(Ok((comp, _))) => {
                        self.replace_composition(comp, None);
                        self.dirty = true;
                        self.status = "Recovered unsaved work: save it to keep it".into();
                    }
                    _ => self.status = "Could not read the recovery copy".into(),
                }
            }
            (Dialog::Recover, _) => recovery::discard(),
            (Dialog::Missing(_), 1) => self.file_request(FileRequest::Relink),
            _ => {}
        }
    }

    fn prefs_window(&mut self, ctx: &egui::Context) {
        if !self.prefs_open {
            return;
        }
        let mut open = true;
        let before = self.prefs.clone();
        egui::Window::new("Preferences")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                use crate::prefs::CellClick;
                ui.label("Clicking a clip cell");
                ui.radio_value(
                    &mut self.prefs.cell_click,
                    CellClick::Launch,
                    "Launches it (Ctrl+click only selects)",
                );
                ui.radio_value(
                    &mut self.prefs.cell_click,
                    CellClick::Select,
                    "Selects it (double-click launches)",
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Interface size");
                    ui.add(
                        egui::Slider::new(&mut self.prefs.ui_scale, 0.75..=2.0)
                            .step_by(0.05)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                    );
                });
                ui.add_space(6.0);
                ui.checkbox(
                    &mut self.prefs.autosave,
                    "Keep a recovery copy of unsaved work",
                )
                .on_hover_text("Saved every minute; offered at the next start after a crash");
                ui.checkbox(
                    &mut self.prefs.reopen_last,
                    "Open the last composition at startup",
                );
            });
        if self.prefs != before {
            if self.prefs.ui_scale != before.ui_scale {
                ctx.set_zoom_factor(self.prefs.ui_scale);
            }
            self.prefs.save();
        }
        self.prefs_open = open;
    }
}

/// Writes `data` to `path` without ever leaving a half-written file: the data
/// goes to a temporary file next to it, the previous version is kept as
/// `<name>.bak`, then the temporary file replaces `path`.
pub(crate) fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    if path.exists() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        let _ = std::fs::copy(path, PathBuf::from(bak));
    }
    std::fs::rename(&tmp, path)
}
