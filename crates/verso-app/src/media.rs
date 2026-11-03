//! Decoded media for the app: still images, video posters and one video player
//! per layer. Anything slow (file decoding, FFmpeg startup) runs on worker
//! threads so the render loop never waits on disk or process launch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use eframe::egui::TextureId;
use eframe::{egui_wgpu, wgpu};
use verso_core::{Composition, Source};
use verso_media::{Frame, VideoPlayer};
use verso_render::{MediaTexture, MediaView};

/// Poster size for video clip cells.
const POSTER: (u32, u32) = (320, 180);

struct Texture {
    texture: MediaTexture,
    id: TextureId,
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
    player: VideoPlayer,
    texture: Option<Texture>,
}

/// Live NDI® input of one layer.
struct LayerLive {
    name: String,
    column: usize,
    receiver: Option<verso_io::ndi::NdiReceiver>,
    error: Option<String>,
    texture: Option<Texture>,
}

/// Result of a worker thread.
enum Loaded {
    Still(PathBuf, verso_media::Result<Frame>),
    Player {
        layer: usize,
        path: PathBuf,
        column: usize,
        player: verso_media::Result<VideoPlayer>,
    },
}

pub struct Media {
    stills: HashMap<PathBuf, Still>,
    players: Vec<Option<LayerPlayer>>,
    /// Player being opened per layer: (path, column).
    opening: Vec<Option<(PathBuf, usize)>>,
    live: Vec<Option<LayerLive>>,
    /// NDI sources found by the last search, and a search in progress.
    ndi_sources: Vec<String>,
    ndi_search: Option<Receiver<Result<Vec<String>, String>>>,
    sender: Sender<Loaded>,
    receiver: Receiver<Loaded>,
}

/// Still image used for a source: the image itself, or a video's poster.
fn still_key(source: &Source) -> Option<&Path> {
    source.path()
}

impl Media {
    /// Keeps players with their layer when layers or columns move.
    pub fn follow(&mut self, changes: &verso_core::IndexChanges) {
        changes.apply_to_layers(&mut self.players);
        changes.apply_to_layers(&mut self.opening);
        changes.apply_to_layers(&mut self.live);
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
        Self {
            stills: HashMap::new(),
            players: Vec::new(),
            opening: Vec::new(),
            live: Vec::new(),
            ndi_sources: Vec::new(),
            ndi_search: None,
            sender,
            receiver,
        }
    }

    fn request_still(&mut self, source: &Source) {
        let Some(path) = still_key(source) else {
            return;
        };
        if self.stills.contains_key(path) {
            return;
        }
        self.stills.insert(path.to_owned(), Still::Loading);
        let (path, video) = (path.to_owned(), matches!(source, Source::Video { .. }));
        let sender = self.sender.clone();
        thread::spawn(move || {
            let frame = if video {
                verso_media::poster(&path, POSTER)
            } else {
                verso_media::load_image(&path)
            };
            let _ = sender.send(Loaded::Still(path, frame));
        });
    }

    fn request_player(&mut self, layer: usize, path: &Path, column: usize, max: (u32, u32)) {
        let wanted = Some((path.to_owned(), column));
        if self.opening[layer] == wanted {
            return;
        }
        self.opening[layer] = wanted;
        let (path, sender) = (path.to_owned(), self.sender.clone());
        thread::spawn(move || {
            let player = VideoPlayer::open(&path, max);
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
        self.players.resize_with(comp.layers.len(), || None);
        self.opening.resize_with(comp.layers.len(), || None);
        self.live.resize_with(comp.layers.len(), || None);
        self.update_live(device, queue, egui, comp, &mut errors);
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
                    if layer >= self.opening.len()
                        || self.opening[layer].as_ref() != Some(&(path.clone(), column))
                    {
                        continue;
                    }
                    self.opening[layer] = None;
                    match player {
                        Ok(player) => {
                            self.players[layer] = Some(LayerPlayer {
                                path,
                                column,
                                player,
                                texture: None,
                            })
                        }
                        Err(e) => errors.push(e.to_string()),
                    }
                }
            }
        }

        for layer in &comp.layers {
            for clip in layer.clips.iter().flatten() {
                self.request_still(&clip.source);
            }
        }

        let max = (comp.width, comp.height);
        for (index, layer) in comp.layers.iter().enumerate() {
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
            let Some(playing) = current else {
                self.players[index] = None;
                self.request_player(index, path, column, max);
                continue;
            };
            if restarted.contains(&index)
                && let Err(e) = playing.player.restart()
            {
                errors.push(e.to_string());
            }
            match playing.player.advance(dt, speed as f64, looping) {
                Ok(Some(frame)) => match &mut playing.texture {
                    Some(texture) => texture.upload(device, queue, egui, frame),
                    None => playing.texture = Some(Texture::new(device, queue, egui, frame)),
                },
                Ok(None) => {}
                Err(e) => {
                    errors.push(e.to_string());
                    self.players[index] = None;
                }
            }
        }
        errors
    }

    /// Opens, feeds and closes NDI receivers to match the playing clips.
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
                        Source::Ndi { name } => Some((column, name)),
                        _ => None,
                    });
            let Some((column, name)) = wanted else {
                self.live[index] = None;
                continue;
            };
            let current = &mut self.live[index];
            if current
                .as_ref()
                .is_none_or(|l| l.name != *name || l.column != column)
            {
                let (receiver, error) = match verso_io::ndi::NdiReceiver::new(name) {
                    Ok(r) => (Some(r), None),
                    Err(e) => {
                        errors.push(e.to_string());
                        (None, Some(e.to_string()))
                    }
                };
                *current = Some(LayerLive {
                    name: name.clone(),
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
            let frame = Frame {
                width: frame.width,
                height: frame.height,
                data: frame.data,
            };
            match &mut live.texture {
                Some(texture) => texture.upload(device, queue, egui, &frame),
                None => live.texture = Some(Texture::new(device, queue, egui, &frame)),
            }
        }
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

    /// Whether loading the file of `source` failed.
    pub fn failed(&self, source: &Source) -> bool {
        still_key(source).is_some_and(|path| matches!(self.stills.get(path), Some(Still::Failed)))
    }

    /// Texture to show for the clip at (`layer`, `column`): the live video when
    /// it is playing, else the image or poster.
    fn texture(&self, comp: &Composition, layer: usize, column: usize) -> Option<&Texture> {
        let clip = comp.layers.get(layer)?.clips.get(column)?.as_ref()?;
        if let Source::Ndi { name } = &clip.source {
            return self
                .live
                .get(layer)?
                .as_ref()
                .filter(|l| l.column == column && l.name == *name)?
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

    /// egui texture and size for a clip cell or the preview monitor.
    pub fn clip_texture(
        &self,
        comp: &Composition,
        layer: usize,
        column: usize,
    ) -> Option<(TextureId, (u32, u32))> {
        self.texture(comp, layer, column)
            .map(|t| (t.id, t.texture.size()))
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
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
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
    fn missing_file_is_reported() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
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
