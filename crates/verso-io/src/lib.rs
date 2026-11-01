//! Video outputs to other applications, and their documents.
//!
//! - [`ndi`]: network video through the NDI® runtime, loaded at run time from
//!   the user's installation (no SDK needed to build Verso).
//! - [`resolume_ao`]: Resolume Arena Advanced Output presets, imported and
//!   exported.
//! - [`svg`]: SVG drawings as polygon slices.
//!
//! Spout (Windows) and Syphon (macOS) texture sharing are planned for v0.4;
//! see `docs/SPEC.md`.
//!
//! Every output takes RGBA8 frames (sRGB, straight alpha) from the renderer.
//!
//! NDI® is a registered trademark of Vizrt NDI AB.

pub mod ndi;
pub mod resolume_ao;
pub mod svg;
pub mod xml;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("NDI: {0}")]
    Ndi(String),
    #[error("{0}")]
    Format(String),
}

pub type Result<T> = std::result::Result<T, Error>;
