//! File dialogs that do not stop the render loop.
//!
//! A native file dialog blocks the thread that opens it. The app renders on
//! the main thread, so a dialog opened there freezes the output window for
//! as long as it stays open. Dialogs run on their own thread instead and
//! report back through a channel, polled every frame.
//!
//! macOS only allows dialogs on the main thread; there they block as before.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    Open,
    Save,
    MediaFolder,
    EffectFolder,
}

pub struct Dialogs {
    sender: Sender<(DialogKind, Option<PathBuf>)>,
    receiver: Receiver<(DialogKind, Option<PathBuf>)>,
    /// Dialogs opened and not yet answered.
    pending: usize,
}

impl Default for Dialogs {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver,
            pending: 0,
        }
    }
}

impl Dialogs {
    /// Shows a dialog built by `show`; its answer comes back from [`Self::poll`].
    /// A second dialog of any kind is refused while one is open.
    pub fn open(&mut self, kind: DialogKind, show: impl FnOnce() -> Option<PathBuf> + Send + 'static) {
        if self.pending > 0 {
            return;
        }
        self.pending += 1;
        let sender = self.sender.clone();
        if cfg!(target_os = "macos") {
            let _ = sender.send((kind, show()));
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("verso-dialog".into())
            .spawn(move || {
                let _ = sender.send((kind, show()));
            });
        if spawned.is_err() {
            self.pending -= 1;
        }
    }

    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    /// Answers that arrived since the last call.
    pub fn poll(&mut self) -> Vec<(DialogKind, Option<PathBuf>)> {
        let answers: Vec<_> = self.receiver.try_iter().collect();
        self.pending = self.pending.saturating_sub(answers.len());
        answers
    }
}
