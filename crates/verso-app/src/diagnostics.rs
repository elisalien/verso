//! Log file, crash reports and GPU error capture.
//!
//! Everything goes to `<user folder>/verso.log` (see
//! [`crate::online_ui::user_dir`]); a panic also writes `crash.log` with a
//! backtrace, so a crash in a release build (no console on Windows) leaves a
//! trace.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::wgpu;

/// The log is restarted when it grows past this size.
const MAX_LOG: u64 = 1024 * 1024;

fn log_path() -> Option<PathBuf> {
    crate::online_ui::user_dir().map(|d| d.join("verso.log"))
}

fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Appends a line to the log file. Never fails: logging must not stop a show.
pub fn log(message: &str) {
    eprintln!("{message}");
    let Some(path) = log_path() else { return };
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = std::fs::rename(&path, path.with_extension("old.log"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "[{}] {message}", timestamp());
    }
}

/// Writes panics to `crash.log` and the log, then runs the default hook.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let report = format!(
            "Verso {} crashed: {info}\n\n{backtrace}",
            env!("CARGO_PKG_VERSION")
        );
        log(&format!("crash: {info}"));
        if let Some(dir) = crate::online_ui::user_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("crash.log"), &report);
        }
        default(info);
    }));
}

/// GPU problems reported by wgpu outside of any error scope, and whether the
/// device was lost. Shared with wgpu's callbacks.
#[derive(Clone, Default)]
pub struct GpuErrors {
    inner: Arc<Mutex<GpuState>>,
}

#[derive(Default)]
struct GpuState {
    errors: Vec<String>,
    last: Option<String>,
    lost: Option<String>,
}

impl GpuErrors {
    /// Catches wgpu errors on `device`, which would otherwise panic, and
    /// device loss.
    pub fn watch(device: &wgpu::Device) -> Self {
        let this = Self::default();
        let errors = this.inner.clone();
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            let text = error.to_string();
            let Ok(mut state) = errors.lock() else { return };
            // The same error usually repeats every frame: log it once.
            if state.last.as_deref() != Some(text.as_str()) {
                log(&format!("GPU error: {text}"));
                state.last = Some(text.clone());
            }
            if state.errors.len() < 100 {
                state.errors.push(text);
            }
        }));
        let lost = this.inner.clone();
        device.set_device_lost_callback(move |reason, message| {
            if reason == wgpu::DeviceLostReason::Destroyed {
                return; // Normal shutdown.
            }
            log(&format!("GPU device lost ({reason:?}): {message}"));
            if let Ok(mut state) = lost.lock() {
                state.lost = Some(message);
            }
        });
        this
    }

    /// Errors since the last call.
    pub fn take(&self) -> Vec<String> {
        self.inner
            .lock()
            .map(|mut s| std::mem::take(&mut s.errors))
            .unwrap_or_default()
    }

    /// Why the GPU device was lost, if it was.
    pub fn lost(&self) -> Option<String> {
        self.inner.lock().ok().and_then(|s| s.lost.clone())
    }
}
