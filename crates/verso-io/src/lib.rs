//! Video outputs to other applications.
//!
//! - [`ndi`]: network video through the NDI® runtime, loaded at run time from
//!   the user's installation (no SDK needed to build Verso).
//!
//! Spout (Windows) and Syphon (macOS) texture sharing are planned for v0.4;
//! see `docs/SPEC.md`.
//!
//! Every output takes RGBA8 frames (sRGB, straight alpha) from the renderer.
//!
//! NDI® is a registered trademark of Vizrt NDI AB.

pub mod ndi;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("NDI: {0}")]
    Ndi(String),
}

pub type Result<T> = std::result::Result<T, Error>;
