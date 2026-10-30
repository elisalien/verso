//! Video outputs to other applications.
//!
//! - [`ndi`]: network video through the NDI® runtime, loaded at run time from
//!   the user's installation (no SDK needed to build Verso).
//! - [`spout`] (Windows): GPU texture sharing with Spout receivers.
//!
//! Every output takes RGBA8 frames (sRGB, straight alpha) from the renderer.
//!
//! NDI® is a registered trademark of Vizrt NDI AB.

pub mod ndi;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("NDI: {0}")]
    Ndi(String),
    #[error("Spout: {0}")]
    Spout(String),
}

pub type Result<T> = std::result::Result<T, Error>;
