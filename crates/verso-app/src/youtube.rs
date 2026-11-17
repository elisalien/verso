//! Browser › YouTube: search with yt-dlp (no API key), download an H.264
//! MP4 into a cache folder, optionally convert it to HAP with FFmpeg, then
//! drag it onto a clip slot. yt-dlp and FFmpeg are programs the user
//! installs; Verso only runs them.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::process::{Child, ExitStatus};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui;
use verso_core::Source;

use crate::app::VersoApp;
use crate::grid::SourceTemplate;
use crate::theme;

/// Results asked for per search.
const RESULTS: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: String,
    /// Seconds, when known (not for live streams).
    pub duration: Option<f64>,
}

/// Where a download stands.
#[derive(Debug, Clone, PartialEq)]
enum Job {
    /// Percent downloaded.
    Downloading(f32),
    Converting,
    Ready(PathBuf),
    Failed(String),
}

enum Event {
    Results(Result<Vec<Video>, String>),
    Job(String, Job),
    /// A video's thumbnail picture, saved in the cache.
    Thumb(String, PathBuf),
}

/// How much of a video to download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Length {
    Full,
    Seconds(u32),
}

impl Length {
    const ALL: [Length; 4] = [
        Length::Seconds(30),
        Length::Seconds(60),
        Length::Seconds(300),
        Length::Full,
    ];

    fn name(self) -> String {
        match self {
            Length::Full => "Whole video".into(),
            Length::Seconds(s) if s < 60 => format!("First {s} s"),
            Length::Seconds(s) => format!("First {} min", s / 60),
        }
    }
}

pub struct YouTubeState {
    query: String,
    results: Vec<Video>,
    jobs: HashMap<String, Job>,
    busy: bool,
    message: String,
    /// Highest video height downloaded.
    max_height: u32,
    length: Length,
    /// Convert downloads to HAP for smooth playback and scrubbing.
    hap: bool,
    /// Thumbnail pictures downloaded so far, by video id.
    thumbs: HashMap<String, PathBuf>,
    sender: Sender<Event>,
    receiver: Receiver<Event>,
    /// yt-dlp and FFmpeg runs in progress, by video id, to stop them when
    /// Verso closes.
    children: Children,
}

type Children = Arc<Mutex<HashMap<String, Child>>>;

impl Drop for YouTubeState {
    fn drop(&mut self) {
        if let Ok(mut children) = self.children.lock() {
            for (_, mut child) in children.drain() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

/// Waits for `child`, passing its output lines to `line`, and returns its
/// status and error output. The child is registered in `children` meanwhile,
/// so closing Verso stops it.
fn run_tracked(
    mut child: Child,
    id: &str,
    children: &Children,
    mut line: impl FnMut(&str),
) -> Result<(ExitStatus, String), String> {
    let stdout = child.stdout.take();
    // Read on its own thread: a full error pipe would block the program
    // while its output is being read here.
    let errors = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
            text
        })
    });
    if let Ok(mut children) = children.lock() {
        children.insert(id.to_owned(), child);
    }
    if let Some(stdout) = stdout {
        for text in BufReader::new(stdout).lines().map_while(Result::ok) {
            line(&text);
        }
    }
    let error = errors.and_then(|e| e.join().ok()).unwrap_or_default();
    let child = children.lock().ok().and_then(|mut c| c.remove(id));
    let Some(mut child) = child else {
        return Err("stopped".into());
    };
    let status = child.wait().map_err(|e| e.to_string())?;
    Ok((status, error))
}

impl Default for YouTubeState {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            query: String::new(),
            results: Vec::new(),
            jobs: HashMap::new(),
            busy: false,
            message: String::new(),
            max_height: 1080,
            length: Length::Seconds(60),
            hap: false,
            thumbs: HashMap::new(),
            sender,
            receiver,
            children: Children::default(),
        }
    }
}

/// Folder holding downloaded videos.
pub fn cache_dir() -> Option<PathBuf> {
    crate::online_ui::user_dir().map(|d| d.join("youtube"))
}

/// A yt-dlp run with UTF-8 output, without a console window.
fn yt_dlp() -> std::process::Command {
    let mut cmd = verso_media::command(&verso_media::tool_path("yt-dlp"));
    if let Some(path) = readable_path() {
        cmd.env("PATH", path);
    }
    cmd.env("PYTHONIOENCODING", "utf-8").args([
        "--encoding",
        "utf-8",
        "--no-warnings",
        "--ignore-config",
    ]);
    cmd
}

/// PATH without folders that cannot be read. yt-dlp resolves every PATH
/// folder while looking for a JavaScript runtime, and stops with an error
/// on one that sits on a missing or unreadable drive.
fn readable_path() -> Option<std::ffi::OsString> {
    let path = std::env::var_os("PATH")?;
    let dirs: Vec<PathBuf> = std::env::split_paths(&path)
        .filter(|dir| dir.is_dir())
        .collect();
    std::env::join_paths(dirs).ok()
}

/// Videos in `yt-dlp --dump-json` output, one JSON object per line.
fn parse_results(output: &str) -> Vec<Video> {
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .filter_map(|v| {
            let id = v["id"].as_str()?;
            // Only plain video ids: they end up in a file name and a URL.
            if id.is_empty()
                || !id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return None;
            }
            Some(Video {
                id: id.to_owned(),
                title: v["title"].as_str().unwrap_or(id).to_owned(),
                channel: v["channel"]
                    .as_str()
                    .or(v["uploader"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
                duration: v["duration"].as_f64(),
            })
        })
        .collect()
}

fn search(query: &str) -> Result<Vec<Video>, String> {
    let output = yt_dlp()
        .args(["--flat-playlist", "--dump-json", "--"])
        .arg(format!("ytsearch{RESULTS}:{query}"))
        .output()
        .map_err(|e| format!("yt-dlp could not be run ({e}): install it and put it on PATH"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let videos = parse_results(&text);
    if videos.is_empty() && !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        let line = error
            .lines()
            .rev()
            .find(|l| l.contains("ERROR"))
            .unwrap_or("search failed");
        return Err(line.trim().to_owned());
    }
    Ok(videos)
}

/// Percent from a yt-dlp progress line (`[download]  42.0% of …`).
fn progress(line: &str) -> Option<f32> {
    let rest = line.trim().strip_prefix("[download]")?.trim_start();
    let percent = rest.split('%').next()?;
    percent.trim().parse().ok()
}

/// yt-dlp format choice: H.264 video up to `height` with AAC audio, so the
/// MP4 plays everywhere.
fn format(height: u32) -> String {
    format!(
        "bv*[vcodec^=avc1][height<={height}]+ba[ext=m4a]/b[ext=mp4][vcodec^=avc1][height<={height}]/b[ext=mp4]"
    )
}

/// Downloads `id` (and converts it to HAP when asked), reporting progress.
fn download(
    id: &str,
    height: u32,
    length: Length,
    hap: bool,
    dir: &Path,
    children: &Children,
    report: &dyn Fn(Job),
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let suffix = match length {
        Length::Full => String::new(),
        Length::Seconds(s) => format!("-{s}s"),
    };
    let stem = format!("{id}-{height}p{suffix}");
    let mp4 = dir.join(format!("{stem}.mp4"));
    if !mp4.is_file() {
        let ffmpeg = verso_media::tool_path("ffmpeg");
        let mut cmd = yt_dlp();
        cmd.args(["--newline", "--no-playlist", "-f", &format(height)])
            .args(["--merge-output-format", "mp4", "--ffmpeg-location", &ffmpeg])
            .arg("-o")
            .arg(dir.join(format!("{stem}.%(ext)s")));
        if let Length::Seconds(s) = length {
            cmd.args(["--download-sections", &format!("*0-{s}")]);
        }
        let child = cmd
            .arg("--")
            .arg(format!("https://www.youtube.com/watch?v={id}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("yt-dlp could not be run ({e}): install it and put it on PATH"))?;
        let (status, error) = run_tracked(child, id, children, |line| {
            if let Some(percent) = progress(line) {
                report(Job::Downloading(percent));
            }
        })?;
        if !status.success() || !mp4.is_file() {
            let line = error
                .lines()
                .rev()
                .find(|l| l.contains("ERROR"))
                .unwrap_or("download failed");
            return Err(line.trim().to_owned());
        }
    }
    if !hap {
        return Ok(mp4);
    }
    let mov = dir.join(format!("{stem}-hap.mov"));
    if mov.is_file() {
        return Ok(mov);
    }
    report(Job::Converting);
    let partial = dir.join(format!("{stem}-hap.part.mov"));
    // HAP needs sizes in multiples of 4. Clip sound is not played yet, so
    // the audio is left out.
    let child = verso_media::command(&verso_media::tool_path("ffmpeg"))
        .args(["-y", "-v", "error", "-i"])
        .arg(&mp4)
        .args(["-vf", "scale=trunc(iw/4)*4:trunc(ih/4)*4", "-an"])
        .args(["-c:v", "hap", "-format", "hap"])
        .arg(&partial)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("FFmpeg could not be run: {e}"))?;
    let (status, error) = run_tracked(child, id, children, |_| {})?;
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!(
            "HAP conversion failed: {}",
            error.lines().last().unwrap_or("").trim()
        ));
    }
    std::fs::rename(&partial, &mov).map_err(|e| e.to_string())?;
    Ok(mov)
}

/// `h:mm:ss` or `m:ss`.
fn duration_text(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

impl YouTubeState {
    fn search(&mut self) {
        let query = self.query.trim().to_owned();
        if query.is_empty() {
            return;
        }
        self.busy = true;
        self.message = "Searching…".into();
        let sender = self.sender.clone();
        thread::spawn(move || {
            let _ = sender.send(Event::Results(search(&query)));
        });
    }

    fn download(&mut self, video: &Video) {
        let Some(dir) = cache_dir() else {
            self.message = "No user folder to download into".into();
            return;
        };
        // One run per video: a second one would write the same file.
        if matches!(
            self.jobs.get(&video.id),
            Some(Job::Downloading(_) | Job::Converting)
        ) {
            return;
        }
        self.jobs.insert(video.id.clone(), Job::Downloading(0.0));
        let (id, height, length, hap) = (video.id.clone(), self.max_height, self.length, self.hap);
        let (sender, children) = (self.sender.clone(), self.children.clone());
        thread::spawn(move || {
            let report = |job| {
                let _ = sender.send(Event::Job(id.clone(), job));
            };
            let job = match download(&id, height, length, hap, &dir, &children, &report) {
                Ok(path) => Job::Ready(path),
                Err(e) => Job::Failed(e),
            };
            report(job);
        });
    }

    /// Downloads the thumbnail pictures of `videos` into the cache, in the
    /// background.
    fn fetch_thumbs(&self, videos: &[Video]) {
        let Some(dir) = cache_dir().map(|d| d.join("thumbs")) else {
            return;
        };
        let ids: Vec<String> = videos
            .iter()
            .map(|v| v.id.clone())
            .filter(|id| !self.thumbs.contains_key(id))
            .collect();
        let sender = self.sender.clone();
        thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            for id in ids {
                let path = dir.join(format!("{id}.jpg"));
                if !path.is_file() {
                    let url = format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg");
                    match verso_online::download(&url) {
                        Ok(bytes) if std::fs::write(&path, &bytes).is_ok() => {}
                        _ => continue,
                    }
                }
                if sender.send(Event::Thumb(id, path)).is_err() {
                    return;
                }
            }
        });
    }

    fn poll(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                Event::Results(Ok(videos)) => {
                    self.busy = false;
                    self.message = format!("{} result(s)", videos.len());
                    self.fetch_thumbs(&videos);
                    self.results = videos;
                }
                Event::Thumb(id, path) => {
                    self.thumbs.insert(id, path);
                }
                Event::Results(Err(e)) => {
                    self.busy = false;
                    self.message = e;
                }
                Event::Job(id, job) => {
                    self.jobs.insert(id, job);
                }
            }
        }
    }
}

impl VersoApp {
    pub(crate) fn youtube_browser(&mut self, ui: &mut egui::Ui) {
        self.youtube.poll();
        let yt = &mut self.youtube;
        let mut search = false;
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut yt.query)
                    .hint_text("Search YouTube…")
                    .desired_width(ui.available_width() - 64.0),
            );
            search |= field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            search |= ui
                .add_enabled(!yt.busy, egui::Button::new("Search"))
                .clicked();
        });
        if search {
            yt.search();
        }
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("youtube height")
                .selected_text(format!("{}p", yt.max_height))
                .width(60.0)
                .show_ui(ui, |ui| {
                    for height in [480, 720, 1080] {
                        ui.selectable_value(&mut yt.max_height, height, format!("{height}p"));
                    }
                })
                .response
                .on_hover_text("Highest resolution downloaded (H.264)");
            egui::ComboBox::from_id_salt("youtube length")
                .selected_text(yt.length.name())
                .width(100.0)
                .show_ui(ui, |ui| {
                    for length in Length::ALL {
                        ui.selectable_value(&mut yt.length, length, length.name());
                    }
                });
            ui.checkbox(&mut yt.hap, "HAP")
                .on_hover_text("Convert to HAP with FFmpeg: plays and scrubs smoothly on the GPU");
        });
        ui.horizontal(|ui| {
            if yt.busy {
                ui.spinner();
            }
            ui.label(
                egui::RichText::new(&yt.message)
                    .small()
                    .color(theme::text_dim()),
            );
        });
        ui.label(
            egui::RichText::new(
                "Uses yt-dlp and FFmpeg, installed separately. Use only videos you are allowed to show.",
            )
            .small()
            .color(theme::text_dim()),
        );
        ui.separator();

        let mut start = None;
        let results = yt.results.clone();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.files.thumbnails, false, "List");
            ui.selectable_value(&mut self.files.thumbnails, true, "Thumbnails");
        });
        let thumbnails = self.files.thumbnails;
        egui::ScrollArea::vertical()
            .id_salt("youtube results")
            .show(ui, |ui| {
                if thumbnails {
                    let (columns, width) = crate::source_browser::tile_grid(ui);
                    for row in results.chunks(columns) {
                        ui.horizontal(|ui| {
                            for video in row {
                                ui.vertical(|ui| {
                                    ui.set_width(width);
                                    if self.youtube_item(ui, video, Some(width)) {
                                        start = Some(video.clone());
                                    }
                                });
                            }
                        });
                    }
                } else {
                    for video in &results {
                        if self.youtube_item(ui, video, None) {
                            start = Some(video.clone());
                        }
                        ui.add_space(2.0);
                    }
                }
            });
        if let Some(video) = start {
            self.youtube.download(&video);
        }
    }

    /// One search result, as a row or (with `tile_width`) a tile. Once
    /// downloaded it drags onto a clip slot like any video file. Returns
    /// whether its download was asked for.
    fn youtube_item(&mut self, ui: &mut egui::Ui, video: &Video, tile_width: Option<f32>) -> bool {
        let thumb = self
            .youtube
            .thumbs
            .get(&video.id)
            .cloned()
            .and_then(|path| self.media.file_thumbnail(&Source::Image { path }))
            .map(|(id, (w, h))| (id, w as f32 / h.max(1) as f32));
        let job = self.youtube.jobs.get(&video.id).cloned();
        let mut info = video.channel.clone();
        if let Some(d) = video.duration {
            info = format!("{info} · {}", duration_text(d));
        }
        let url = format!("https://www.youtube.com/watch?v={}", video.id);
        let ready = match &job {
            Some(Job::Ready(path)) => Some(SourceTemplate {
                name: video.title.clone(),
                source: Source::Video {
                    path: path.clone(),
                    speed: 1.0,
                    looping: true,
                },
            }),
            _ => None,
        };
        let mut get = false;
        let response = match tile_width {
            Some(width) => {
                let response = youtube_picture(ui, video, thumb, ready.as_ref(), Some(width), &url);
                ui.horizontal(|ui| get |= youtube_status(ui, job.as_ref()));
                response
            }
            None => {
                ui.horizontal(|ui| {
                    let response = youtube_picture(ui, video, thumb, ready.as_ref(), None, &url);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&video.title).strong());
                        ui.label(egui::RichText::new(&info).small().color(theme::text_dim()));
                        ui.horizontal(|ui| get |= youtube_status(ui, job.as_ref()));
                        if let Some(Job::Failed(e)) = &job {
                            ui.label(egui::RichText::new(e).small().color(theme::warn()));
                        }
                    });
                    response
                })
                .inner
            }
        };
        match ready {
            Some(template) => self.browser_item_events(response, egui::Rect::NOTHING, template, ""),
            None => get |= response.double_clicked(),
        }
        get
    }
}

/// A result's picture, a tile with `tile_width` else a small thumbnail;
/// draggable onto a clip slot once the video is in the cache.
fn youtube_picture(
    ui: &mut egui::Ui,
    video: &Video,
    thumb: Option<(egui::TextureId, f32)>,
    ready: Option<&SourceTemplate>,
    tile_width: Option<f32>,
    url: &str,
) -> egui::Response {
    let paint = |ui: &mut egui::Ui| match tile_width {
        Some(width) => crate::source_browser::paint_tile(
            ui,
            width,
            &crate::source_browser::TileLook {
                name: &video.title,
                texture: thumb,
                accent: theme::accent(),
                outlined: ready.is_some(),
                favorite: None,
                badge: None,
            },
        ),
        None => {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(96.0, 54.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 2.0, theme::bg_deep());
            if let Some((texture, _)) = thumb {
                ui.painter().image(
                    texture,
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            if ready.is_some() || response.contains_pointer() {
                let width = if ready.is_some() { 2.0 } else { 1.0 };
                ui.painter().rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(width, theme::accent()),
                    egui::StrokeKind::Inside,
                );
            }
            response
        }
    };
    let response = match ready {
        Some(template) => {
            let id = egui::Id::unique(("verso-youtube", &video.id));
            ui.dnd_drag_source(id, template.clone(), paint).inner
        }
        None => paint(ui),
    };
    response.on_hover_text(match ready {
        Some(_) => format!("{url}\nDrag onto a clip slot"),
        None => format!("{url}\nDouble-click to download"),
    })
}

/// Download progress of a result, or its Get button. Returns whether Get
/// was clicked.
fn youtube_status(ui: &mut egui::Ui, job: Option<&Job>) -> bool {
    match job {
        Some(Job::Downloading(p)) => {
            ui.add(
                egui::ProgressBar::new(p / 100.0)
                    .desired_width(56.0)
                    .text(format!("{p:.0}%")),
            );
            false
        }
        Some(Job::Converting) => {
            ui.spinner().on_hover_text("Converting to HAP");
            false
        }
        Some(Job::Ready(_)) => {
            ui.label(
                egui::RichText::new("Ready: drag onto a slot")
                    .small()
                    .color(theme::accent()),
            );
            false
        }
        Some(Job::Failed(_)) | None => ui
            .small_button("Get")
            .on_hover_text("Download into the cache")
            .clicked(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_are_parsed_and_unsafe_ids_dropped() {
        let output = concat!(
            "Deprecated Feature: something\n",
            r#"{"id": "Pk7eBbD_dhc", "title": "Free Vj Loop 30", "channel": "NKUB", "duration": 12}"#,
            "\n",
            r#"{"id": "abc/../x", "title": "bad"}"#,
            "\n",
            r#"{"id": "live-1", "title": "Live", "uploader": "Someone", "duration": null}"#,
            "\n",
        );
        let videos = parse_results(output);
        assert_eq!(videos.len(), 2);
        assert_eq!(videos[0].title, "Free Vj Loop 30");
        assert_eq!(videos[0].duration, Some(12.0));
        assert_eq!(videos[1].channel, "Someone");
        assert_eq!(videos[1].duration, None);
    }

    #[test]
    fn progress_lines() {
        assert_eq!(
            progress("[download]  42.5% of   10.00MiB at  1.00MiB/s ETA 00:05"),
            Some(42.5)
        );
        assert_eq!(progress("[download] 100% of 10.00MiB"), Some(100.0));
        assert_eq!(progress("[Merger] Merging formats"), None);
        assert_eq!(progress("[download] Destination: x.mp4"), None);
    }

    /// Needs yt-dlp and the network: `cargo test -- --ignored live_search`.
    #[test]
    #[ignore]
    fn live_search() {
        let videos = search("free vj loop").unwrap();
        assert!(!videos.is_empty());
        assert!(
            videos
                .iter()
                .all(|v| !v.id.is_empty() && !v.title.is_empty())
        );
    }

    /// Needs yt-dlp, FFmpeg and the network; downloads into the folder in
    /// `VERSO_YOUTUBE_TEST_DIR`: `cargo test -- --ignored live_download`.
    #[test]
    #[ignore]
    fn live_download() {
        let dir = PathBuf::from(std::env::var("VERSO_YOUTUBE_TEST_DIR").unwrap());
        let seen = std::sync::Mutex::new(Vec::new());
        let report = |job| seen.lock().unwrap().push(job);
        let path = download(
            "Pk7eBbD_dhc",
            480,
            Length::Seconds(30),
            true,
            &dir,
            &Children::default(),
            &report,
        )
        .unwrap();
        assert!(path.extension().is_some_and(|e| e == "mov"));
        assert!(dir.join("Pk7eBbD_dhc-480p-30s.mp4").is_file());
        let seen = seen.into_inner().unwrap();
        assert!(seen.iter().any(|j| matches!(j, Job::Downloading(_))));
        assert!(seen.contains(&Job::Converting));
    }

    #[test]
    fn durations() {
        assert_eq!(duration_text(12.0), "0:12");
        assert_eq!(duration_text(3725.0), "1:02:05");
    }
}
