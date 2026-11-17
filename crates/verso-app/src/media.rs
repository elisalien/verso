//! Decoded media for the app: still images, video posters and one video player
//! per layer. Anything slow (file decoding, FFmpeg startup) runs on worker
//! threads so the render loop never waits on disk or process launch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui::TextureId;
use eframe::{egui_wgpu, wgpu};
use verso_core::{Composition, Source};
use verso_media::hap::HapFormat;
use verso_media::{Frame, HapPlayer, Transport, VideoPlayer};
use verso_render::{MediaFormat, MediaTexture, MediaView};

/// Poster size for video clip cells.
const POSTER: (u32, u32) = (320, 180);
/// Width of the stills of text and time clips that are not playing.
const TEXT_THUMB_WIDTH: u32 = 384;
/// Text stills rendered per frame at most, so a pasted grid of text clips
/// does not stall a frame.
const TEXT_THUMBS_PER_FRAME: usize = 4;
/// Frames a text still is kept once no clip or browser tile shows it.
const TEXT_THUMB_FRAMES: u64 = 120;

/// Shortest time between two seeks of an FFmpeg-decoded video.
const SEEK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);

/// Frames an image or poster is kept once no clip or browser tile asks for
/// it.
const STILL_FRAMES: u64 = 600;

struct Texture {
    texture: MediaTexture,
    id: TextureId,
}

thread_local! {
    /// Ids of dropped textures, still registered with egui. egui keeps the
    /// GPU texture of an id alive until it is freed, and a `Texture` is
    /// dropped in many places that cannot reach the egui renderer.
    static RETIRED: std::cell::RefCell<Vec<TextureId>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Drop for Texture {
    fn drop(&mut self) {
        let id = self.id;
        // Ignored during thread teardown: the renderer goes away with it.
        let _ = RETIRED.try_with(|retired| retired.borrow_mut().push(id));
    }
}

/// Unregisters the textures dropped since the last call.
fn free_retired(egui: &mut egui_wgpu::Renderer) {
    let retired = RETIRED.with(|retired| std::mem::take(&mut *retired.borrow_mut()));
    for id in retired {
        egui.free_texture(&id);
    }
}

fn media_format(format: HapFormat) -> MediaFormat {
    match format {
        HapFormat::Bc1 => MediaFormat::Bc1,
        HapFormat::Bc3 => MediaFormat::Bc3,
        HapFormat::YCoCgBc3 => MediaFormat::YCoCgBc3,
        HapFormat::Bc7 => MediaFormat::Bc7,
    }
}

/// A video file being played: natively for HAP when the GPU reads BC
/// textures, else through FFmpeg.
enum Player {
    Ffmpeg(VideoPlayer),
    Hap(HapPlayer),
}

impl Player {
    fn restart(&mut self) -> verso_media::Result<()> {
        match self {
            Player::Ffmpeg(p) => p.restart(),
            Player::Hap(p) => {
                p.restart();
                Ok(())
            }
        }
    }

    fn seek(&mut self, seconds: f64) -> verso_media::Result<()> {
        match self {
            Player::Ffmpeg(p) => p.seek(seconds),
            Player::Hap(p) => {
                p.seek(seconds);
                Ok(())
            }
        }
    }

    fn position(&self) -> f64 {
        match self {
            Player::Ffmpeg(p) => p.position(),
            Player::Hap(p) => p.position(),
        }
    }

    fn transport(&self) -> Transport {
        match self {
            Player::Ffmpeg(p) => p.transport(),
            Player::Hap(p) => {
                let info = p.info();
                let position = p.position();
                Transport {
                    position,
                    duration: info.duration,
                    frame: (position * info.fps).round() as u64,
                    ended: p.ended(),
                }
            }
        }
    }

    fn duration(&self) -> Option<f64> {
        match self {
            Player::Ffmpeg(p) => p.info().duration,
            Player::Hap(p) => p.info().duration,
        }
    }

    /// Advances and uploads the new frame into `texture`, if any.
    #[allow(clippy::too_many_arguments)]
    fn advance(
        &mut self,
        dt: f64,
        speed: f64,
        looping: bool,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
        texture: &mut Option<Texture>,
    ) -> verso_media::Result<()> {
        match self {
            Player::Ffmpeg(p) => {
                if let Some(frame) = p.advance(dt, speed, looping)? {
                    match texture {
                        Some(t) => t.upload(device, queue, egui, frame),
                        None => *texture = Some(Texture::new(device, queue, egui, frame)),
                    }
                }
            }
            Player::Hap(p) => {
                if let Some(frame) = p.advance(dt, speed, looping)? {
                    let format = media_format(frame.format);
                    let size = (frame.width, frame.height);
                    let t =
                        texture.get_or_insert_with(|| Texture::empty(device, egui, size, format));
                    t.upload_as(device, queue, egui, size, format, &frame.data);
                }
            }
        }
        Ok(())
    }
}

impl Texture {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
        frame: &Frame,
    ) -> Self {
        let size = (frame.width, frame.height);
        let mut texture = MediaTexture::new(device, size);
        texture.upload(device, queue, size, &frame.data);
        let id = egui.register_native_texture(device, texture.view(), wgpu::FilterMode::Linear);
        Self { texture, id }
    }

    fn empty(
        device: &wgpu::Device,
        egui: &mut egui_wgpu::Renderer,
        size: (u32, u32),
        format: MediaFormat,
    ) -> Self {
        let texture = MediaTexture::with_format(device, size, format);
        let id = egui.register_native_texture(device, texture.view(), wgpu::FilterMode::Linear);
        Self { texture, id }
    }

    fn upload_as(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
        size: (u32, u32),
        format: MediaFormat,
        data: &[u8],
    ) {
        if self.texture.upload_as(device, queue, size, format, data) {
            egui.update_egui_texture_from_wgpu_texture(
                device,
                self.texture.view(),
                wgpu::FilterMode::Linear,
                self.id,
            );
        }
    }

    /// Whether egui can show it as is (HAP Q holds YCoCg, not colors).
    fn displayable(&self) -> bool {
        self.texture.format() != MediaFormat::YCoCgBc3
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
        frame: &Frame,
    ) {
        let size = (frame.width, frame.height);
        if self.texture.upload(device, queue, size, &frame.data) {
            egui.update_egui_texture_from_wgpu_texture(
                device,
                self.texture.view(),
                wgpu::FilterMode::Linear,
                self.id,
            );
        }
    }
}

enum Still {
    Loading,
    Ready(Texture),
    Failed,
}

struct LayerPlayer {
    path: PathBuf,
    column: usize,
    player: Player,
    texture: Option<Texture>,
}

/// A video still playing while its layer fades to another clip.
struct Outgoing {
    player: LayerPlayer,
    /// Seconds of fade left.
    remaining: f64,
    speed: f64,
    looping: bool,
}

/// Whether this build receives Spout senders as clip sources (Windows).
pub const SPOUT_INPUT: bool = cfg!(windows);

/// Receiver of a live input clip (NDI® or Spout).
struct LiveReceiver(verso_io::receiver::FrameReceiver);

impl LiveReceiver {
    /// Connects to the live source of `source`; `None` for other sources.
    fn open(source: &Source) -> Option<Result<Self, String>> {
        let receiver = match source {
            Source::Ndi { name } => verso_io::ndi::receiver(name).map_err(|e| e.to_string()),
            #[cfg(windows)]
            Source::Spout { name } => verso_io::spout::receiver(name).map_err(|e| e.to_string()),
            #[cfg(not(windows))]
            Source::Spout { .. } => Err("Spout input needs Windows".into()),
            _ => return None,
        };
        Some(receiver.map(LiveReceiver))
    }

    /// The newest frame if it arrived since the last call.
    fn take(&mut self) -> Option<Frame> {
        self.0.take().map(|f| Frame {
            width: f.width,
            height: f.height,
            data: f.data,
        })
    }

    fn received(&self) -> u64 {
        self.0.received()
    }

    fn error(&self) -> Option<String> {
        self.0.error()
    }
}

/// Live NDI® or Spout® input of one layer.
struct LayerLive {
    /// The clip source it receives (an NDI or Spout source).
    source: Source,
    column: usize,
    receiver: Option<LiveReceiver>,
    error: Option<String>,
    texture: Option<Texture>,
}

/// Text source of one layer, drawn each frame it animates.
struct LayerText {
    column: usize,
    layout: Option<verso_text::Layout>,
    canvas: verso_text::Canvas,
    /// Seconds of animation since the clip was launched.
    elapsed: f32,
    /// Seconds since the clip was launched, for timers.
    launched: f64,
    /// Look of the last frame drawn, while nothing moves.
    drawn: Option<verso_core::text::TextStyle>,
    texture: Option<Texture>,
}

/// Fonts, loaded on a thread at startup since that takes a moment.
enum Fonts {
    Loading(Receiver<(verso_text::TextRenderer, Vec<verso_text::FontFamily>)>),
    Ready(Box<verso_text::TextRenderer>),
}

/// Still of a text or time source, for clip cells, the Preview and browser
/// tiles when the source is not playing.
struct TextThumb {
    texture: Texture,
    /// Text drawn: a clock's changes once a second.
    shown: String,
    /// Frame it was last wanted.
    used: u64,
}

/// Result of a worker thread.
enum Loaded {
    Still(PathBuf, verso_media::Result<Frame>),
    Player {
        layer: usize,
        path: PathBuf,
        column: usize,
        player: Box<verso_media::Result<Player>>,
    },
}

/// Threads decoding stills and posters. Each poster runs `ffprobe` and
/// `ffmpeg`, so a dropped folder of 300 clips must not start 600 processes
/// at once: requests queue up and this many run at a time.
const STILL_WORKERS: usize = 3;

/// A still or poster to decode: (path, is a video).
type StillJob = (PathBuf, bool);

pub struct Media {
    stills: HashMap<PathBuf, Still>,
    /// Update at which each still was last asked for.
    still_used: HashMap<PathBuf, u64>,
    /// Videos that could not be opened or played. They are not tried again
    /// every frame, only when their clip is launched again.
    failed_videos: std::collections::HashSet<PathBuf>,
    /// When the last seek ran, and a seek waiting for its turn: (layer,
    /// column, seconds).
    last_seek: std::time::Instant,
    pending_seek: Option<(usize, usize, f64)>,
    players: Vec<Option<LayerPlayer>>,
    /// Player being opened per layer: (path, column).
    opening: Vec<Option<(PathBuf, usize)>>,
    outgoing: Vec<Option<Outgoing>>,
    live: Vec<Option<LayerLive>>,
    text: Vec<Option<LayerText>>,
    /// Stills of text and time sources, by [`text_thumb_key`].
    text_thumbs: HashMap<u64, TextThumb>,
    /// Text and time sources of browser tiles to have stills of.
    text_thumb_requests: Vec<Source>,
    /// Updates so far, to drop stills no longer shown.
    frame: u64,
    fonts: Fonts,
    /// Font families, once the fonts are loaded.
    families: Vec<verso_text::FontFamily>,
    /// NDI sources found by the last search, and a search in progress.
    ndi_sources: Vec<String>,
    ndi_search: Option<Receiver<Result<Vec<String>, String>>>,
    /// Spout senders found by the last search.
    spout_senders: Vec<String>,
    /// The GPU reads BC textures: HAP plays without FFmpeg.
    native_hap: bool,
    sender: Sender<Loaded>,
    receiver: Receiver<Loaded>,
    /// Queue feeding the still workers.
    still_jobs: Sender<StillJob>,
}

/// Still image used for a source: the image itself, or a video's poster.
fn still_key(source: &Source) -> Option<&Path> {
    source.path()
}

/// Key of the still of a text or time source: the whole source, so the
/// still is drawn again when any of its settings changes.
fn text_thumb_key(source: &Source) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    if !matches!(source, Source::Text { .. } | Source::Time { .. }) {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{source:?}").hash(&mut hasher);
    Some(hasher.finish())
}

/// Size of the text stills for a composition of `size`.
fn text_thumb_size(size: (u32, u32)) -> (u32, u32) {
    let height = TEXT_THUMB_WIDTH as u64 * size.1.max(1) as u64 / size.0.max(1) as u64;
    (
        TEXT_THUMB_WIDTH,
        (height as u32).clamp(1, TEXT_THUMB_WIDTH * 4),
    )
}

/// Decodes still jobs until the queue closes.
fn still_worker(jobs: Arc<Mutex<Receiver<StillJob>>>, sender: Sender<Loaded>) {
    loop {
        let job = match jobs.lock() {
            Ok(queue) => queue.recv(),
            Err(_) => return,
        };
        let Ok((path, video)) = job else {
            return;
        };
        let frame = if video {
            verso_media::poster(&path, POSTER)
        } else {
            verso_media::load_image(&path)
        };
        if sender.send(Loaded::Still(path, frame)).is_err() {
            return;
        }
    }
}

impl Media {
    /// Keeps players with their layer when layers or columns move.
    pub fn follow(&mut self, changes: &verso_core::IndexChanges) {
        changes.apply_to_layers(&mut self.players);
        changes.apply_to_layers(&mut self.opening);
        changes.apply_to_layers(&mut self.live);
        changes.apply_to_layers(&mut self.text);
        changes.apply_to_layers(&mut self.outgoing);
        for text in self.text.iter_mut() {
            if let Some(t) = text {
                match changes.columns.get(t.column).copied().flatten() {
                    Some(c) => t.column = c,
                    None => *text = None,
                }
            }
        }
        for live in self.live.iter_mut() {
            if let Some(l) = live {
                match changes.columns.get(l.column).copied().flatten() {
                    Some(c) => l.column = c,
                    None => *live = None,
                }
            }
        }
        let column = |c: usize| changes.columns.get(c).copied().flatten();
        for player in self.players.iter_mut() {
            if let Some(p) = player {
                match column(p.column) {
                    Some(c) => p.column = c,
                    None => *player = None,
                }
            }
        }
        for opening in self.opening.iter_mut() {
            if let Some((_, c)) = opening {
                match column(*c) {
                    Some(new) => *c = new,
                    None => *opening = None,
                }
            }
        }
    }

    pub fn new() -> Self {
        let (sender, receiver) = channel();
        let (still_jobs, jobs) = channel();
        let jobs = Arc::new(Mutex::new(jobs));
        for index in 0..STILL_WORKERS {
            let (jobs, sender) = (jobs.clone(), sender.clone());
            let _ = thread::Builder::new()
                .name(format!("verso-still-{index}"))
                .spawn(move || still_worker(jobs, sender));
        }
        let (fonts_sender, fonts) = channel();
        let _ = thread::Builder::new()
            .name("verso-fonts".into())
            .spawn(move || {
                let mut renderer = verso_text::TextRenderer::new();
                if let Some(dir) = crate::online_ui::user_dir().map(|d| d.join("fonts")) {
                    renderer.load_fonts_dir(&dir);
                }
                let families = renderer.families();
                let _ = fonts_sender.send((renderer, families));
            });
        Self {
            stills: HashMap::new(),
            still_used: HashMap::new(),
            failed_videos: std::collections::HashSet::new(),
            last_seek: std::time::Instant::now(),
            pending_seek: None,
            players: Vec::new(),
            opening: Vec::new(),
            outgoing: Vec::new(),
            live: Vec::new(),
            text: Vec::new(),
            text_thumbs: HashMap::new(),
            text_thumb_requests: Vec::new(),
            frame: 0,
            fonts: Fonts::Loading(fonts),
            families: Vec::new(),
            ndi_sources: Vec::new(),
            ndi_search: None,
            spout_senders: Vec::new(),
            native_hap: false,
            sender,
            receiver,
            still_jobs,
        }
    }

    fn request_still(&mut self, source: &Source) {
        let Some(path) = still_key(source) else {
            return;
        };
        match self.still_used.get_mut(path) {
            Some(used) => *used = self.frame,
            None => {
                self.still_used.insert(path.to_owned(), self.frame);
            }
        }
        if self.stills.contains_key(path) {
            return;
        }
        self.stills.insert(path.to_owned(), Still::Loading);
        let video = matches!(source, Source::Video { .. });
        let _ = self.still_jobs.send((path.to_owned(), video));
    }

    /// Playback position of the clip at (`layer`, `column`), when it is the
    /// one playing on that layer.
    pub fn transport(&self, layer: usize, column: usize) -> Option<Transport> {
        self.players
            .get(layer)?
            .as_ref()
            .filter(|p| p.column == column)
            .map(|p| p.player.transport())
    }

    fn request_player(&mut self, layer: usize, path: &Path, column: usize, max: (u32, u32)) {
        let wanted = Some((path.to_owned(), column));
        if self.opening[layer] == wanted {
            return;
        }
        self.opening[layer] = wanted;
        let (path, sender, native) = (path.to_owned(), self.sender.clone(), self.native_hap);
        thread::spawn(move || {
            let player = Box::new(
                match native.then(|| HapPlayer::open(&path).ok()).flatten() {
                    Some(hap) => Ok(Player::Hap(hap)),
                    None => VideoPlayer::open(&path, max).map(Player::Ffmpeg),
                },
            );
            let _ = sender.send(Loaded::Player {
                layer,
                path,
                column,
                player,
            });
        });
    }

    /// Loads what `comp` needs, advances video by `dt` seconds and uploads new
    /// frames. `restarted` lists layers whose clip was launched again this
    /// frame. Returns error messages to show.
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
        comp: &Composition,
        restarted: &[usize],
        dt: f64,
    ) -> Vec<String> {
        let mut errors = Vec::new();
        free_retired(egui);
        if let Some((layer, column, seconds)) = self.pending_seek
            && self.last_seek.elapsed() >= SEEK_INTERVAL
        {
            self.pending_seek = None;
            errors.extend(self.seek(layer, column, seconds));
        }
        self.native_hap = device
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        self.players.resize_with(comp.layers.len(), || None);
        self.opening.resize_with(comp.layers.len(), || None);
        self.live.resize_with(comp.layers.len(), || None);
        self.text.resize_with(comp.layers.len(), || None);
        self.outgoing.resize_with(comp.layers.len(), || None);
        self.update_live(device, queue, egui, comp, &mut errors);
        self.update_text(device, queue, egui, comp, restarted, dt);
        self.update_text_thumbs(device, queue, egui, comp);
        if let Some(search) = &self.ndi_search
            && let Ok(result) = search.try_recv()
        {
            self.ndi_search = None;
            match result {
                Ok(sources) => self.ndi_sources = sources,
                Err(e) => errors.push(e),
            }
        }

        for loaded in self.receiver.try_iter().collect::<Vec<_>>() {
            match loaded {
                Loaded::Still(path, Ok(frame)) => {
                    let texture = Texture::new(device, queue, egui, &frame);
                    self.stills.insert(path, Still::Ready(texture));
                }
                Loaded::Still(path, Err(e)) => {
                    errors.push(e.to_string());
                    self.stills.insert(path, Still::Failed);
                }
                Loaded::Player {
                    layer,
                    path,
                    column,
                    player,
                } => {
                    // Ignore players for a clip that is no longer wanted.
                    // The layer may have moved while the file was opening:
                    // it is found by what it waits for; the index sent with
                    // the request only breaks a tie.
                    let wanted = Some((path.clone(), column));
                    let layer = if self.opening.get(layer) == Some(&wanted) {
                        layer
                    } else if let Some(moved) = self.opening.iter().position(|o| *o == wanted) {
                        moved
                    } else {
                        continue;
                    };
                    self.opening[layer] = None;
                    match *player {
                        Ok(player) => {
                            self.players[layer] = Some(LayerPlayer {
                                path,
                                column,
                                player,
                                texture: None,
                            })
                        }
                        Err(e) => {
                            errors.push(e.to_string());
                            self.failed_videos.insert(path);
                        }
                    }
                }
            }
        }

        for layer in &comp.layers {
            for clip in layer.clips.iter().flatten() {
                self.request_still(&clip.source);
            }
        }
        // Images and posters nothing asked for lately are dropped; ones
        // still decoding stay, so their result has somewhere to go.
        let (frame, used) = (self.frame, &mut self.still_used);
        self.stills.retain(|path, still| {
            matches!(still, Still::Loading)
                || used
                    .get(path)
                    .is_some_and(|&at| frame.saturating_sub(at) < STILL_FRAMES)
        });
        used.retain(|path, _| self.stills.contains_key(path));

        let max = (comp.width, comp.height);
        for (index, layer) in comp.layers.iter().enumerate() {
            // A player whose clip is no longer the playing one fades out.
            let replaced = self.players[index].as_ref().is_some_and(|p| {
                layer.active != Some(p.column)
                    || layer.active_clip().and_then(|c| c.source.path()) != Some(p.path.as_path())
            });
            if replaced && layer.transition > 0.0 && self.outgoing[index].is_none() {
                let speed = layer
                    .clips
                    .get(self.players[index].as_ref().map(|p| p.column).unwrap_or(0))
                    .and_then(|c| c.as_ref())
                    .map(|c| match c.source {
                        Source::Video { speed, looping, .. } => (speed as f64, looping),
                        _ => (1.0, true),
                    })
                    .unwrap_or((1.0, true));
                self.outgoing[index] = self.players[index].take().map(|player| Outgoing {
                    player,
                    remaining: layer.transition as f64,
                    speed: speed.0,
                    looping: speed.1,
                });
            }
            if let Some(out) = &mut self.outgoing[index] {
                out.remaining -= dt;
                let done = out.remaining <= 0.0
                    || out
                        .player
                        .player
                        .advance(
                            dt,
                            out.speed,
                            out.looping,
                            device,
                            queue,
                            egui,
                            &mut out.player.texture,
                        )
                        .is_err();
                if done {
                    self.outgoing[index] = None;
                }
            }
            let video =
                layer
                    .active
                    .zip(layer.active_clip())
                    .and_then(|(column, clip)| match &clip.source {
                        Source::Video {
                            path,
                            speed,
                            looping,
                        } => Some((column, path, *speed, *looping)),
                        _ => None,
                    });
            let Some((column, path, speed, looping)) = video else {
                self.players[index] = None;
                self.opening[index] = None;
                continue;
            };
            let current = self.players[index]
                .as_mut()
                .filter(|p| p.column == column && p.path == *path);
            if restarted.contains(&index) {
                self.failed_videos.remove(path);
            }
            let Some(playing) = current else {
                self.players[index] = None;
                if !self.failed_videos.contains(path) {
                    self.request_player(index, path, column, max);
                }
                continue;
            };
            if restarted.contains(&index)
                && let Err(e) = playing.player.restart()
            {
                errors.push(e.to_string());
            }
            let result = playing.player.advance(
                dt,
                speed as f64,
                looping,
                device,
                queue,
                egui,
                &mut playing.texture,
            );
            match result {
                Ok(()) => {}
                Err(e) => {
                    errors.push(e.to_string());
                    self.failed_videos.insert(path.clone());
                    self.players[index] = None;
                }
            }
        }
        errors
    }

    /// Opens, feeds and closes NDI and Spout receivers to match the playing
    /// clips.
    fn update_live(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
        comp: &Composition,
        errors: &mut Vec<String>,
    ) {
        for (index, layer) in comp.layers.iter().enumerate() {
            let wanted =
                layer
                    .active
                    .zip(layer.active_clip())
                    .and_then(|(column, clip)| match &clip.source {
                        Source::Ndi { .. } | Source::Spout { .. } => Some((column, &clip.source)),
                        _ => None,
                    });
            let Some((column, source)) = wanted else {
                self.live[index] = None;
                continue;
            };
            let current = &mut self.live[index];
            if current
                .as_ref()
                .is_none_or(|l| l.source != *source || l.column != column)
            {
                let (receiver, error) = match LiveReceiver::open(source) {
                    Some(Ok(r)) => (Some(r), None),
                    Some(Err(e)) => {
                        errors.push(e.clone());
                        (None, Some(e))
                    }
                    None => (None, None),
                };
                *current = Some(LayerLive {
                    source: source.clone(),
                    column,
                    receiver,
                    error,
                    texture: None,
                });
            }
            let Some(live) = current else { continue };
            let Some(frame) = live.receiver.as_mut().and_then(|r| r.take()) else {
                continue;
            };
            match &mut live.texture {
                Some(texture) => texture.upload(device, queue, egui, &frame),
                None => live.texture = Some(Texture::new(device, queue, egui, &frame)),
            }
        }
    }

    /// Draws the text layers that changed or move, at the composition size.
    fn update_text(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
        comp: &Composition,
        restarted: &[usize],
        dt: f64,
    ) {
        if let Fonts::Loading(receiver) = &self.fonts
            && let Ok((renderer, families)) = receiver.try_recv()
        {
            self.families = families;
            self.fonts = Fonts::Ready(Box::new(renderer));
        }
        let size = (comp.width, comp.height);
        for (index, layer) in comp.layers.iter().enumerate() {
            let wanted =
                layer
                    .active
                    .zip(layer.active_clip())
                    .and_then(|(column, clip)| match &clip.source {
                        Source::Text { text, style } => Some((column, Some(text), None, style)),
                        Source::Time {
                            mode,
                            format,
                            style,
                        } => Some((column, None, Some((*mode, *format)), style)),
                        _ => None,
                    });
            let Some((column, text, time, style)) = wanted else {
                self.text[index] = None;
                continue;
            };
            let Fonts::Ready(renderer) = &mut self.fonts else {
                continue;
            };
            let current = &mut self.text[index];
            if current
                .as_ref()
                .is_none_or(|t| t.column != column || t.canvas.size() != size)
                || restarted.contains(&index)
            {
                *current = Some(LayerText {
                    column,
                    layout: None,
                    canvas: verso_text::Canvas::new(size),
                    elapsed: 0.0,
                    launched: 0.0,
                    drawn: None,
                    texture: None,
                });
            }
            let Some(state) = current else { continue };
            state.elapsed += dt as f32 * style.speed;
            state.launched += dt;
            let shown;
            let text = match (text, time) {
                (Some(text), _) => text.as_str(),
                (None, Some((mode, format))) => {
                    shown = verso_core::text::time_text(
                        mode,
                        format,
                        state.launched,
                        verso_text::local_seconds(),
                    );
                    shown.as_str()
                }
                (None, None) => continue,
            };
            if state
                .layout
                .as_ref()
                .is_none_or(|l| !verso_text::TextRenderer::is_current(l, text, style, size))
            {
                state.layout = Some(renderer.layout(text, style, size));
                state.drawn = None;
            }
            let moving = style.animation != verso_core::text::TextAnimation::None;
            if !moving && state.drawn.as_ref() == Some(style) {
                continue;
            }
            let Some(layout) = &state.layout else {
                continue;
            };
            renderer.draw(layout, style, state.elapsed, &mut state.canvas);
            state.drawn = Some(style.clone());
            let frame = Frame {
                width: size.0,
                height: size.1,
                data: state.canvas.data.clone(),
            };
            match &mut state.texture {
                Some(texture) => texture.upload(device, queue, egui, &frame),
                None => state.texture = Some(Texture::new(device, queue, egui, &frame)),
            }
        }
    }

    /// Draws a still of each text and time clip and of the text sources
    /// asked for by the browser, for when they are not playing. A still is
    /// drawn again when its source changes, and a clock's once a second.
    fn update_text_thumbs(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
        comp: &Composition,
    ) {
        self.frame += 1;
        let Fonts::Ready(renderer) = &mut self.fonts else {
            return;
        };
        let size = text_thumb_size((comp.width, comp.height));
        let clock = verso_text::local_seconds();
        let requests = std::mem::take(&mut self.text_thumb_requests);
        let sources = comp
            .layers
            .iter()
            .flat_map(|layer| layer.clips.iter().flatten().map(|clip| &clip.source))
            .chain(requests.iter());
        let mut drawn = 0;
        for source in sources {
            let (text, style) = match source {
                Source::Text { text, style } => (text.clone(), style),
                // Timers show where they start.
                Source::Time {
                    mode,
                    format,
                    style,
                } => (
                    verso_core::text::time_text(*mode, *format, 0.0, clock),
                    style,
                ),
                _ => continue,
            };
            let Some(key) = text_thumb_key(source) else {
                continue;
            };
            if let Some(thumb) = self.text_thumbs.get_mut(&key)
                && thumb.shown == text
                && thumb.texture.texture.size() == size
            {
                thumb.used = self.frame;
                continue;
            }
            if drawn >= TEXT_THUMBS_PER_FRAME {
                continue;
            }
            drawn += 1;
            // Stills show the whole text, at rest.
            let still = verso_core::text::TextStyle {
                animation: verso_core::text::TextAnimation::None,
                ..style.clone()
            };
            let layout = renderer.layout(&text, &still, size);
            let mut canvas = verso_text::Canvas::new(size);
            renderer.draw(&layout, &still, 0.0, &mut canvas);
            let frame = Frame {
                width: size.0,
                height: size.1,
                data: canvas.data,
            };
            match self.text_thumbs.get_mut(&key) {
                Some(thumb) => {
                    thumb.texture.upload(device, queue, egui, &frame);
                    thumb.shown = text;
                    thumb.used = self.frame;
                }
                None => {
                    let texture = Texture::new(device, queue, egui, &frame);
                    self.text_thumbs.insert(
                        key,
                        TextThumb {
                            texture,
                            shown: text,
                            used: self.frame,
                        },
                    );
                }
            }
        }
        let frame = self.frame;
        self.text_thumbs
            .retain(|_, thumb| frame - thumb.used < TEXT_THUMB_FRAMES);
    }

    /// Still of a text or time source, once drawn.
    fn text_thumb(&self, source: &Source) -> Option<&Texture> {
        self.text_thumbs
            .get(&text_thumb_key(source)?)
            .map(|t| &t.texture)
    }

    /// egui texture of the still of a text or time source for a browser
    /// tile; asks for it to be drawn (or kept) at the next update.
    pub fn text_tile(&mut self, source: &Source) -> Option<TextureId> {
        text_thumb_key(source)?;
        self.text_thumb_requests.push(source.clone());
        self.text_thumb(source).map(|t| t.id)
    }

    /// Font families for text sources; empty until the fonts are loaded.
    pub fn font_families(&self) -> &[verso_text::FontFamily] {
        &self.families
    }

    /// Whether the fonts are still loading.
    pub fn fonts_loading(&self) -> bool {
        matches!(self.fonts, Fonts::Loading(_))
    }

    /// Starts looking for NDI sources on the network (takes a second or two).
    pub fn search_ndi(&mut self) {
        if self.ndi_search.is_some() {
            return;
        }
        let (sender, receiver) = channel();
        self.ndi_search = Some(receiver);
        thread::spawn(move || {
            let result = verso_io::ndi::find_sources(std::time::Duration::from_secs(2))
                .map_err(|e| e.to_string());
            let _ = sender.send(result);
        });
    }

    pub fn ndi_searching(&self) -> bool {
        self.ndi_search.is_some()
    }

    pub fn ndi_sources(&self) -> &[String] {
        &self.ndi_sources
    }

    /// Lists the Spout senders running on this computer (instant: Spout
    /// keeps the list in shared memory).
    pub fn search_spout(&mut self) {
        #[cfg(windows)]
        {
            self.spout_senders = verso_io::spout::find_senders();
        }
    }

    pub fn spout_senders(&self) -> &[String] {
        &self.spout_senders
    }

    /// State of the live input of the clip at (`layer`, `column`), for the
    /// Properties panel.
    pub fn live_state(&self, layer: usize, column: usize) -> String {
        match self.live.get(layer).and_then(|l| l.as_ref()) {
            Some(live) if live.column == column => {
                if let Some(error) = live
                    .error
                    .clone()
                    .or_else(|| live.receiver.as_ref().and_then(|r| r.error()))
                {
                    error
                } else {
                    match live.receiver.as_ref().map(|r| r.received()).unwrap_or(0) {
                        0 => "Connecting… (no frame yet)".into(),
                        n => format!("Receiving · {n} frames"),
                    }
                }
            }
            _ => "Launch the clip to connect".into(),
        }
    }

    fn still(&self, source: &Source) -> Option<&Texture> {
        match self.stills.get(still_key(source)?) {
            Some(Still::Ready(texture)) => Some(texture),
            _ => None,
        }
    }

    /// Thumbnail of an image or video file for the browser: the image
    /// itself or the video's poster, decoded in the background on first use.
    pub fn file_thumbnail(&mut self, source: &Source) -> Option<(TextureId, (u32, u32))> {
        self.request_still(source);
        self.still(source).map(|t| (t.id, t.texture.size()))
    }

    /// Whether loading the file of `source` failed.
    pub fn failed(&self, source: &Source) -> bool {
        still_key(source).is_some_and(|path| matches!(self.stills.get(path), Some(Still::Failed)))
    }

    /// Texture to show for the clip at (`layer`, `column`): the live video when
    /// it is playing, else the image or poster.
    fn texture(&self, comp: &Composition, layer: usize, column: usize) -> Option<&Texture> {
        let clip = comp.layers.get(layer)?.clips.get(column)?.as_ref()?;
        if let Source::Text { .. } | Source::Time { .. } = &clip.source {
            return self
                .text
                .get(layer)?
                .as_ref()
                .filter(|t| t.column == column)?
                .texture
                .as_ref();
        }
        if let Source::Ndi { .. } | Source::Spout { .. } = &clip.source {
            return self
                .live
                .get(layer)?
                .as_ref()
                .filter(|l| l.column == column && l.source == clip.source)?
                .texture
                .as_ref();
        }
        let live = self
            .players
            .get(layer)?
            .as_ref()
            .filter(|p| p.column == column && Some(p.path.as_path()) == clip.source.path());
        live.and_then(|p| p.texture.as_ref())
            .or_else(|| self.still(&clip.source))
    }

    /// Frame shown for the clip at (`layer`, `column`) in its cell and the
    /// preview monitor.
    fn clip_frame(&self, comp: &Composition, layer: usize, column: usize) -> Option<&Texture> {
        let clip = comp.layers.get(layer)?.clips.get(column)?.as_ref()?;
        self.texture(comp, layer, column)
            .filter(|t| t.displayable())
            .or_else(|| self.still(&clip.source))
            .or_else(|| self.text_thumb(&clip.source))
    }

    /// egui texture and size for a clip cell or the preview monitor.
    pub fn clip_texture(
        &self,
        comp: &Composition,
        layer: usize,
        column: usize,
    ) -> Option<(TextureId, (u32, u32))> {
        self.clip_frame(comp, layer, column)
            .map(|t| (t.id, t.texture.size()))
    }

    /// The frame of [`Self::clip_texture`] as a view to render the clip
    /// with its effects, and its egui texture (which changes when another
    /// frame, such as a loaded poster, replaces it).
    pub fn clip_view(
        &self,
        comp: &Composition,
        layer: usize,
        column: usize,
    ) -> Option<(MediaView<'_>, TextureId)> {
        self.clip_frame(comp, layer, column)
            .map(|t| (t.texture.media_view(), t.id))
    }

    /// Frame of each layer's clip in `column`, for previewing that column.
    pub fn column_views<'a>(
        &'a self,
        comp: &Composition,
        column: usize,
    ) -> Vec<Option<MediaView<'a>>> {
        (0..comp.layers.len())
            .map(|layer| {
                self.texture(comp, layer, column)
                    .or_else(|| {
                        let clip = comp.layers[layer].clips.get(column)?.as_ref()?;
                        self.text_thumb(&clip.source)
                    })
                    .map(|t| t.texture.media_view())
            })
            .collect()
    }

    /// Playing video of the clip at (`layer`, `column`).
    fn playing(&self, layer: usize, column: usize) -> Option<&LayerPlayer> {
        self.players
            .get(layer)?
            .as_ref()
            .filter(|p| p.column == column)
    }

    /// Position and duration in seconds of the clip's video while it plays,
    /// and whether it can play backwards (HAP read natively).
    pub fn playback(&self, layer: usize, column: usize) -> Option<(f64, Option<f64>, bool)> {
        let playing = self.playing(layer, column)?;
        Some((
            playing.player.position(),
            playing.player.duration(),
            matches!(playing.player, Player::Hap(_)),
        ))
    }

    /// Jumps the video of the clip at (`layer`, `column`) to `seconds`.
    pub fn seek(&mut self, layer: usize, column: usize, seconds: f64) -> Option<String> {
        let playing = self
            .players
            .get_mut(layer)?
            .as_mut()
            .filter(|p| p.column == column)?;
        // An FFmpeg seek starts a new FFmpeg: dragging the timeline would
        // start one per frame. Seeks closer than `SEEK_INTERVAL` wait, and
        // the last one is done by `update`.
        if matches!(playing.player, Player::Ffmpeg(_)) && self.last_seek.elapsed() < SEEK_INTERVAL {
            self.pending_seek = Some((layer, column, seconds));
            return None;
        }
        self.pending_seek = None;
        self.last_seek = std::time::Instant::now();
        playing.player.seek(seconds).err().map(|e| e.to_string())
    }

    /// Frame of the video each layer is fading out of, in layer order.
    pub fn outgoing_views(&self) -> Vec<Option<MediaView<'_>>> {
        self.outgoing
            .iter()
            .map(|o| {
                o.as_ref()?
                    .player
                    .texture
                    .as_ref()
                    .map(|t| t.texture.media_view())
            })
            .collect()
    }

    /// Frame to composite for each layer, in layer order.
    pub fn layer_views<'a>(&'a self, comp: &Composition) -> Vec<Option<MediaView<'a>>> {
        comp.layers
            .iter()
            .enumerate()
            .map(|(index, layer)| {
                let column = layer.active?;
                self.texture(comp, index, column)
                    .map(|t| t.texture.media_view())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use verso_core::{Clip, Command};

    /// Encodes a short clip with FFmpeg; `None` when FFmpeg is missing.
    fn test_clip() -> Option<PathBuf> {
        let dir = std::env::temp_dir().join("verso-app-test");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("clip.mp4");
        let ffmpeg = verso_media::tool_path("ffmpeg");
        let ok = std::process::Command::new(ffmpeg)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("testsrc=size=320x180:rate=25:duration=1")
            .args(["-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        ok.then_some(path)
    }

    #[test]
    fn plays_video_layer_and_loads_poster() {
        let Some(path) = test_clip() else {
            eprintln!("ffmpeg not available, skipping");
            return;
        };
        let instance = verso_render::instance();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let mut egui =
            egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Bgra8Unorm, Default::default());

        let mut comp = Composition::new("t", 1, 2);
        comp.width = 640;
        comp.height = 360;
        let video = Source::Video {
            path: path.clone(),
            speed: 1.0,
            looping: true,
        };
        comp.apply(Command::SetClip {
            layer: 0,
            column: 1,
            clip: Some(Clip::new("clip", video)),
        })
        .unwrap();
        comp.apply(Command::TriggerClip {
            layer: 0,
            column: 1,
        })
        .unwrap();

        let mut media = Media::new();
        let start = Instant::now();
        let mut live = false;
        while start.elapsed() < Duration::from_secs(10) {
            let errors = media.update(&device, &queue, &mut egui, &comp, &[], 0.02);
            assert!(errors.is_empty(), "{errors:?}");
            let player_frame = media.players[0]
                .as_ref()
                .and_then(|p| p.texture.as_ref())
                .map(|t| t.texture.size());
            if player_frame == Some((320, 180)) {
                live = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(live, "video never produced a frame");
        assert!(
            matches!(media.stills.get(&path), Some(Still::Ready(_))),
            "poster not loaded"
        );
        let views = media.layer_views(&comp);
        assert_eq!(views[0].map(|v| v.size), Some((320, 180)));
        assert!(media.clip_texture(&comp, 0, 1).is_some());
        assert!(media.clip_texture(&comp, 0, 0).is_none());

        // Ejecting the layer stops its player.
        comp.apply(Command::EjectLayer(0)).unwrap();
        media.update(&device, &queue, &mut egui, &comp, &[], 0.02);
        assert!(media.players[0].is_none());
    }

    #[test]
    fn idle_text_and_time_clips_get_stills() {
        let instance = verso_render::instance();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let mut egui =
            egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Bgra8Unorm, Default::default());
        let mut comp = Composition::new("t", 1, 2);
        let text = Source::Text {
            text: "HELLO".into(),
            style: Default::default(),
        };
        let time = Source::Time {
            mode: verso_core::text::TimeMode::Clock,
            format: Default::default(),
            style: Default::default(),
        };
        for (column, source) in [text, time].into_iter().enumerate() {
            comp.apply(Command::SetClip {
                layer: 0,
                column,
                clip: Some(Clip::new("t", source)),
            })
            .unwrap();
        }
        let mut media = Media::new();
        let start = Instant::now();
        while media.fonts_loading() && start.elapsed() < Duration::from_secs(60) {
            media.update(&device, &queue, &mut egui, &comp, &[], 0.02);
            std::thread::sleep(Duration::from_millis(10));
        }
        if media.font_families().is_empty() {
            eprintln!("no font on this system, skipping");
            return;
        }
        media.update(&device, &queue, &mut egui, &comp, &[], 0.02);
        // Neither clip plays, yet both show a still.
        let width = TEXT_THUMB_WIDTH;
        assert_eq!(
            media.clip_texture(&comp, 0, 0).map(|t| t.1),
            Some((width, width * 9 / 16))
        );
        assert!(media.clip_texture(&comp, 0, 1).is_some());
        assert!(media.column_views(&comp, 0)[0].is_some());
        assert!(media.layer_views(&comp)[0].is_none(), "not composited");
        // A changed source gets its own still; the old one goes in time.
        let Some(Source::Text { text, .. }) =
            comp.layers[0].clips[0].as_mut().map(|c| &mut c.source)
        else {
            unreachable!()
        };
        *text = "WORLD".into();
        media.update(&device, &queue, &mut egui, &comp, &[], 0.02);
        assert!(media.clip_texture(&comp, 0, 0).is_some());
        for _ in 0..TEXT_THUMB_FRAMES {
            media.update(&device, &queue, &mut egui, &comp, &[], 0.02);
        }
        assert_eq!(media.text_thumbs.len(), 2);
    }

    #[test]
    fn missing_file_is_reported() {
        let instance = verso_render::instance();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let mut egui =
            egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Bgra8Unorm, Default::default());
        let mut comp = Composition::new("t", 1, 1);
        let source = Source::Image {
            path: "does/not/exist.png".into(),
        };
        comp.apply(Command::SetClip {
            layer: 0,
            column: 0,
            clip: Some(Clip::new("missing", source.clone())),
        })
        .unwrap();
        let mut media = Media::new();
        let start = Instant::now();
        let mut errors = Vec::new();
        while errors.is_empty() && start.elapsed() < Duration::from_secs(5) {
            errors = media.update(&device, &queue, &mut egui, &comp, &[], 0.02);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(errors.len(), 1);
        assert!(media.failed(&source));
    }
}
