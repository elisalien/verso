//! Video outputs to other applications.
//!
//! - [`ndi`]: network video through the NDI® runtime, loaded at run time from
//!   the user's installation (no SDK needed to build Verso).
//! - `spout` (Windows): DirectX texture sharing with Spout receivers.
//! - `syphon` (macOS): Metal texture sharing with Syphon clients.
//!
//! Every output takes RGBA8 frames (sRGB, straight alpha) from the renderer.
//!
//! NDI® is a registered trademark of Vizrt NDI AB.

pub mod ndi;
#[cfg(windows)]
pub mod spout;
#[cfg(target_os = "macos")]
pub mod syphon;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("NDI: {0}")]
    Ndi(String),
    #[error("Spout: {0}")]
    Spout(String),
    #[error("Syphon: {0}")]
    Syphon(String),
}

pub type Result<T> = std::result::Result<T, Error>;
