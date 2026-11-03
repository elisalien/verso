//! Native file dialogs that do not block the frame loop.
//!
//! A blocking dialog stops `update`, so the output window would freeze while
//! the user browses for a file. These dialogs are futures polled once per
//! frame instead: the show goes on while the dialog is open.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use crate::app::AfterSave;

/// What a file dialog was opened for.
#[derive(Debug, Clone, PartialEq)]
pub enum FileRequest {
    /// Save the composition under a new name, then continue with `AfterSave`.
    SaveAs(Option<AfterSave>),
    Open,
    /// Folder to search missing media in.
    Relink,
    /// Folder for Browser › Files.
    FilesFolder,
    /// Folder of effects to load.
    EffectsFolder,
    /// Resolume Advanced Output preset to import.
    AdvancedImport,
    /// File to export the Advanced Output to, with its suggested name.
    AdvancedExport(String),
    /// SVG drawing whose shapes become slices.
    AdvancedSvg,
}

type Pending = Pin<Box<dyn Future<Output = Option<rfd::FileHandle>>>>;

#[derive(Default)]
pub struct FileDialogs {
    open: Option<(FileRequest, Pending)>,
}

impl FileDialogs {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Shows the dialog for `request`; ignored while another is open.
    pub fn start(&mut self, request: FileRequest, comp_name: &str) {
        if self.open.is_some() {
            return;
        }
        let composition = |d: rfd::AsyncFileDialog| d.add_filter("Verso composition", &["verso"]);
        let future: Pending = match &request {
            FileRequest::SaveAs(_) => Box::pin(
                composition(rfd::AsyncFileDialog::new())
                    .set_file_name(format!("{comp_name}.verso"))
                    .save_file(),
            ),
            FileRequest::Open => Box::pin(composition(rfd::AsyncFileDialog::new()).pick_file()),
            FileRequest::Relink | FileRequest::FilesFolder | FileRequest::EffectsFolder => {
                Box::pin(rfd::AsyncFileDialog::new().pick_folder())
            }
            FileRequest::AdvancedImport => Box::pin(resolume_preset().pick_file()),
            FileRequest::AdvancedExport(name) => {
                Box::pin(resolume_preset().set_file_name(name).save_file())
            }
            FileRequest::AdvancedSvg => Box::pin(
                rfd::AsyncFileDialog::new()
                    .add_filter("SVG drawing", &["svg"])
                    .pick_file(),
            ),
        };
        self.open = Some((request, future));
    }

    /// The finished dialog, if any: its request and the chosen path (`None`
    /// when cancelled).
    pub fn poll(&mut self) -> Option<(FileRequest, Option<PathBuf>)> {
        let (_, future) = self.open.as_mut()?;
        // The app repaints every frame, so polling without a waker is enough.
        let mut cx = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut cx) {
            Poll::Pending => None,
            Poll::Ready(handle) => {
                let (request, _) = self.open.take()?;
                Some((request, handle.map(|h| h.path().to_owned())))
            }
        }
    }
}

/// Dialog for Resolume Advanced Output presets, opened in Resolume's preset
/// folder when it exists.
fn resolume_preset() -> rfd::AsyncFileDialog {
    let dialog =
        rfd::AsyncFileDialog::new().add_filter("Resolume Advanced Output preset", &["xml"]);
    match crate::advanced_output_ui::resolume_preset_dir() {
        Some(dir) => dialog.set_directory(dir),
        None => dialog,
    }
}
