//! Receiving live video (NDI®, Spout) on a background thread.
//!
//! Each kind of input has its own loop that reads frames; what the loop
//! shares with the app (newest frame, frame count, error, stop request) is
//! the same for all of them and lives here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// One received RGBA8 frame, rows top to bottom, straight alpha.
#[derive(Clone)]
pub struct LiveFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// What a receive loop shares with its [`FrameReceiver`].
#[derive(Default)]
pub(crate) struct Shared {
    stop: AtomicBool,
    /// Newest frame and its number (1 for the first).
    latest: Mutex<(u64, Option<LiveFrame>)>,
    /// Why no frame comes in right now, if anything is wrong.
    error: Mutex<Option<String>>,
}

impl Shared {
    /// Whether the receiver was dropped: the loop must end.
    pub(crate) fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub(crate) fn set_error(&self, message: Option<String>) {
        if let Ok(mut error) = self.error.lock() {
            *error = message;
        }
    }

    /// Makes `frame` the newest one.
    pub(crate) fn publish(&self, frame: LiveFrame) {
        if let Ok(mut latest) = self.latest.lock() {
            latest.0 += 1;
            latest.1 = Some(frame);
        }
    }
}

/// Receives the frames of one live source on a background thread, keeping
/// only the newest. Stops when dropped.
pub struct FrameReceiver {
    shared: Arc<Shared>,
    /// Number of the last frame handed out by [`Self::take`].
    taken: u64,
}

impl FrameReceiver {
    /// Runs `receive` on a thread named `thread` until it returns; it must
    /// return soon after [`Shared::stopped`] turns true.
    pub(crate) fn spawn(
        thread: &str,
        receive: impl FnOnce(&Shared) + Send + 'static,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        // The handle is dropped: receivers are dropped on the render thread,
        // which must not wait for the loop to see the stop request.
        std::thread::Builder::new()
            .name(thread.into())
            .spawn(move || receive(&worker))?;
        Ok(Self { shared, taken: 0 })
    }

    /// The newest frame if it arrived since the last call.
    pub fn take(&mut self) -> Option<LiveFrame> {
        let mut latest = self.shared.latest.lock().ok()?;
        if latest.0 == self.taken {
            return None;
        }
        self.taken = latest.0;
        latest.1.take()
    }

    /// Frames received so far.
    pub fn received(&self) -> u64 {
        self.shared.latest.lock().map(|l| l.0).unwrap_or(0)
    }

    /// Why no frame comes in, if something is wrong (source not running,
    /// unsupported format, …).
    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().ok().and_then(|e| e.clone())
    }
}

impl Drop for FrameReceiver {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_frame_is_taken_once_and_the_loop_stops() {
        let (ended, wait) = std::sync::mpsc::channel();
        let mut receiver = FrameReceiver::spawn("test-recv", move |shared| {
            shared.set_error(Some("waiting".into()));
            for n in 1..=3u8 {
                shared.publish(LiveFrame {
                    width: 1,
                    height: 1,
                    data: vec![n; 4],
                });
            }
            shared.set_error(None);
            while !shared.stopped() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let _ = ended.send(());
        })
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while receiver.received() < 3 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(receiver.received(), 3);
        assert_eq!(receiver.take().unwrap().data, [3; 4]);
        assert!(receiver.take().is_none());
        drop(receiver);
        wait.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the loop did not stop");
    }
}
