//! Log file and panic hook.
//!
//! Messages go to `verso.log` in the user directory (the previous run is kept
//! as `verso.log.old`), so a crash can be reported without a terminal. An
//! instance started with an MCP server writes `verso-mcp-<port>.log` instead,
//! so a test instance never replaces the log of the one in use. In
//! release builds on Windows there is no console at all. Everything from the
//! `verso*` crates is logged at `info` and above; dependencies (wgpu, winit,
//! naga…) only at `warn` and above.

use log::Log;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static PATH: OnceLock<PathBuf> = OnceLock::new();

struct FileLog {
    file: Mutex<File>,
    start: Instant,
}

impl log::Log for FileLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        let max = if m.target().starts_with("verso") {
            log::Level::Info
        } else {
            log::Level::Warn
        };
        m.level() <= max
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{:>10.3} {:<5} [{}] {}\n",
            self.start.elapsed().as_secs_f64(),
            record.level(),
            record.target(),
            record.args()
        );
        if let Ok(mut file) = self.file.lock() {
            let _ = file.write_all(line.as_bytes());
        }
        #[cfg(debug_assertions)]
        eprint!("{line}");
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}

/// Where the current log file is, once `init` succeeded.
pub fn path() -> Option<&'static PathBuf> {
    PATH.get()
}

/// Opens the log file, installs the logger and a panic hook that writes the
/// panic before the default hook runs. Does nothing when the user directory
/// is unknown or the file cannot be created: the app still starts.
pub fn init() {
    let Some(dir) = crate::online_ui::user_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let name = match instance_tag() {
        Some(tag) => format!("verso-{tag}.log"),
        None => "verso.log".to_owned(),
    };
    let path = dir.join(&name);
    let _ = std::fs::rename(&path, dir.join(format!("{name}.old")));
    let Ok(file) = File::create(&path) else {
        return;
    };
    let logger: &'static FileLog = Box::leak(Box::new(FileLog {
        file: Mutex::new(file),
        start: Instant::now(),
    }));
    if log::set_logger(logger).is_err() {
        return;
    }
    log::set_max_level(log::LevelFilter::Info);
    let _ = PATH.set(path);

    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    log::info!(
        "Verso {} starting ({} {}), unix time {epoch}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!(
            "panic: {info}
{backtrace}"
        );
        logger.flush();
        // A separate report, so the cause survives the next run's log.
        let report = format!(
            "Verso {} crashed: {info}

{backtrace}",
            env!("CARGO_PKG_VERSION")
        );
        let _ = std::fs::write(dir.join("crash.log"), report);
        default_hook(info);
    }));
}

/// `mcp-<port>` for an instance started with an MCP server, which keeps its
/// log and recovery copy apart so a test instance never replaces the files
/// of the one in use.
pub fn instance_tag() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    verso_mcp::requested_port(&args, std::env::var("VERSO_MCP_PORT").ok().as_deref())
        .map(|port| format!("mcp-{port}"))
}

/// GPU errors logged before the rest are dropped, so a broken resource used
/// every frame does not fill the log.
const GPU_ERRORS_LOGGED: usize = 20;

/// Whether the GPU device was lost, shared with wgpu's callback.
#[derive(Clone, Default)]
pub struct GpuState {
    lost: std::sync::Arc<Mutex<Option<String>>>,
}

impl GpuState {
    /// Why the GPU device was lost, if it was.
    pub fn lost(&self) -> Option<String> {
        self.lost.lock().ok().and_then(|l| l.clone())
    }
}

/// Logs GPU errors nothing else caught, and a lost device. In release builds
/// an uncaptured error is logged instead of panicking, so one bad resource
/// does not stop a show; debug builds keep the wgpu panic.
pub fn watch_gpu(device: &eframe::wgpu::Device) -> GpuState {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let state = GpuState::default();
    let lost = state.lost.clone();
    device.set_device_lost_callback(move |reason, message| {
        if reason == eframe::wgpu::DeviceLostReason::Destroyed {
            return; // Normal shutdown.
        }
        log::error!("GPU device lost ({reason:?}): {message}");
        if let Ok(mut lost) = lost.lock() {
            *lost = Some(message);
        }
    });
    if cfg!(debug_assertions) {
        return state;
    }
    let count = AtomicUsize::new(0);
    device.on_uncaptured_error(Arc::new(move |error| {
        match count.fetch_add(1, Ordering::Relaxed) {
            n if n < GPU_ERRORS_LOGGED => log::error!("GPU error: {error}"),
            GPU_ERRORS_LOGGED => log::error!("further GPU errors are not logged"),
            _ => {}
        }
    }));
    state
}
