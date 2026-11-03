//! Media decoding for Verso, without any GPU dependency.
//!
//! Still images decode with the `image` crate. Video decodes in an `ffmpeg`
//! child process that writes raw RGBA frames to a pipe, so every codec FFmpeg
//! knows (HAP, H.264, H.265, ProRes, …) plays without linking libav. The
//! binaries are found on `PATH`, or through `VERSO_FFMPEG` and `VERSO_FFPROBE`.

mod player;

use std::path::Path;
use std::process::{Command, Stdio};

pub use player::VideoPlayer;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Image {
        path: String,
        source: image::ImageError,
    },
    #[error("could not run {program}: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("{program} failed: {message}")]
    Tool { program: String, message: String },
    #[error("no video stream in {0}")]
    NoVideo(String),
    #[error("{0} is not a file")]
    NotAFile(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Kind of media a file holds, from its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
}

impl MediaKind {
    pub fn of(path: &Path) -> Option<MediaKind> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "bmp" | "webp" | "tga" | "tif" | "tiff" => {
                Some(MediaKind::Image)
            }
            // GIF goes through FFmpeg so animated files play.
            "mov" | "mp4" | "m4v" | "mkv" | "webm" | "avi" | "mpg" | "mpeg" | "wmv" | "gif" => {
                Some(MediaKind::Video)
            }
            _ => None,
        }
    }
}

/// An RGBA8 image with straight alpha, sRGB encoded.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}×{})", self.width, self.height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Seconds, when the container reports it.
    pub duration: Option<f64>,
}

/// Path of an FFmpeg tool: `VERSO_<NAME>` if set, else the first match on
/// `PATH`, else the bare name.
///
/// `PATH` is split by hand: `std::env::split_paths` and `Command` treat a stray
/// `"` in one entry as an open quote and lose every entry after it.
pub fn tool_path(name: &str) -> String {
    let var = format!("VERSO_{}", name.to_ascii_uppercase());
    if let Ok(path) = std::env::var(var) {
        return path;
    }
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let separator = if cfg!(windows) { ';' } else { ':' };
    let paths = std::env::var("PATH").unwrap_or_default();
    paths
        .split(separator)
        .map(|dir| std::path::Path::new(dir.trim().trim_matches('"')).join(&file))
        .find(|candidate| candidate.is_file())
        .map(|found| found.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_owned())
}

pub(crate) fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashing up for each decoder.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Adds `path` as the input of an FFmpeg tool. Only local files are read:
/// the path must be a file, is passed with the `file:` protocol, and other
/// protocols are refused, so a composition cannot make FFmpeg reach the
/// network or read through `concat:`, `subfile:` and the like.
pub(crate) fn input(cmd: &mut Command, path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(Error::NotAFile(path.display().to_string()));
    }
    let mut arg = std::ffi::OsString::from("file:");
    arg.push(path.as_os_str());
    cmd.args(["-protocol_whitelist", "file", "-i"]).arg(arg);
    Ok(())
}

/// Runs `cmd` to completion and returns its stdout.
fn run(mut cmd: Command, program: &str) -> Result<Vec<u8>> {
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|source| Error::Spawn {
            program: program.to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(Error::Tool {
            program: program.to_owned(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(output.stdout)
}

pub fn load_image(path: &Path) -> Result<Frame> {
    let image = image::open(path)
        .map_err(|source| Error::Image {
            path: path.display().to_string(),
            source,
        })?
        .into_rgba8();
    Ok(Frame {
        width: image.width(),
        height: image.height(),
        data: image.into_raw(),
    })
}

fn parse_rate(rate: &str) -> Option<f64> {
    let (num, den) = rate.split_once('/').unwrap_or((rate, "1"));
    let (num, den): (f64, f64) = (num.trim().parse().ok()?, den.trim().parse().ok()?);
    (num > 0.0 && den > 0.0).then(|| num / den)
}

/// Reads the size, frame rate and duration of the first video stream.
pub fn probe(path: &Path) -> Result<VideoInfo> {
    let program = tool_path("ffprobe");
    let mut cmd = command(&program);
    cmd.args([
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=width,height,avg_frame_rate,r_frame_rate:format=duration",
        "-of",
        "default=noprint_wrappers=1",
    ]);
    input(&mut cmd, path)?;
    let text = String::from_utf8_lossy(&run(cmd, &program)?).into_owned();

    let mut info = VideoInfo {
        width: 0,
        height: 0,
        fps: 0.0,
        duration: None,
    };
    let mut fallback_fps = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "width" => info.width = value.parse().unwrap_or(0),
            "height" => info.height = value.parse().unwrap_or(0),
            "avg_frame_rate" => info.fps = parse_rate(value).unwrap_or(0.0),
            "r_frame_rate" => fallback_fps = parse_rate(value),
            "duration" => info.duration = value.parse().ok(),
            _ => {}
        }
    }
    if !(info.fps > 0.0 && info.fps <= 1000.0) {
        info.fps = fallback_fps
            .filter(|f| *f > 0.0 && *f <= 1000.0)
            .unwrap_or(30.0);
    }
    if info.width == 0 || info.height == 0 {
        return Err(Error::NoVideo(path.display().to_string()));
    }
    Ok(info)
}

/// Largest size with the aspect of `size` that fits in `max`, with even
/// dimensions as most scalers require.
pub fn fit_size(size: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let scale = (max.0 as f64 / size.0 as f64)
        .min(max.1 as f64 / size.1 as f64)
        .min(1.0);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(size.0 as f64 * scale), even(size.1 as f64 * scale))
}

/// Decodes the first frame of a video, scaled to fit in `max`.
pub fn poster(path: &Path, max: (u32, u32)) -> Result<Frame> {
    let info = probe(path)?;
    let (width, height) = fit_size((info.width, info.height), max);
    let program = tool_path("ffmpeg");
    let mut cmd = command(&program);
    cmd.args(["-v", "error"]);
    input(&mut cmd, path)?;
    cmd.args(["-frames:v", "1", "-vf"])
        .arg(format!("scale={width}:{height}"))
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"]);
    let data = run(cmd, &program)?;
    if data.len() != width as usize * height as usize * 4 {
        return Err(Error::NoVideo(path.display().to_string()));
    }
    Ok(Frame {
        width,
        height,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_kind_from_extension() {
        assert_eq!(MediaKind::of(Path::new("a/b.PNG")), Some(MediaKind::Image));
        assert_eq!(MediaKind::of(Path::new("clip.mov")), Some(MediaKind::Video));
        assert_eq!(MediaKind::of(Path::new("loop.gif")), Some(MediaKind::Video));
        assert_eq!(MediaKind::of(Path::new("notes.txt")), None);
        assert_eq!(MediaKind::of(Path::new("noext")), None);
    }

    #[test]
    fn fit_size_keeps_aspect_and_even_dims() {
        assert_eq!(fit_size((3840, 2160), (1920, 1080)), (1920, 1080));
        assert_eq!(fit_size((1280, 720), (1920, 1080)), (1280, 720));
        assert_eq!(fit_size((1080, 1920), (1920, 1080)), (608, 1080));
        assert_eq!(fit_size((101, 51), (1920, 1080)), (100, 50));
    }

    #[test]
    fn frame_rates() {
        assert_eq!(
            parse_rate("30000/1001").map(|r| (r * 100.0).round()),
            Some(2997.0)
        );
        assert_eq!(parse_rate("25"), Some(25.0));
        assert_eq!(parse_rate("0/0"), None);
    }

    #[test]
    fn loads_png() {
        let dir = std::env::temp_dir().join("verso-media-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pixel.png");
        image::save_buffer(
            &path,
            &[255, 0, 0, 255, 0, 255, 0, 128],
            2,
            1,
            image::ColorType::Rgba8,
        )
        .unwrap();
        let frame = load_image(&path).unwrap();
        assert_eq!((frame.width, frame.height), (2, 1));
        assert_eq!(frame.data, [255, 0, 0, 255, 0, 255, 0, 128]);
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[test]
    fn only_local_files_are_opened() {
        for url in [
            "http://127.0.0.1:9/x.mp4",
            "concat:a.mp4|b.mp4",
            "/no/such/file.mp4",
        ] {
            assert!(
                matches!(probe(Path::new(url)), Err(Error::NotAFile(_))),
                "{url}"
            );
        }
    }
}
