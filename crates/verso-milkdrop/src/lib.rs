//! MilkDrop presets for Verso, without the GPU: the preset model, an EEL
//! interpreter, MilkDrop audio analysis and the per-frame engine that feeds
//! the renderer. Ported from Butterchurn (MIT, Jordan Berg).

pub mod audio;
mod blend;
pub mod eel;
mod engine;
mod library;
pub mod preset;

pub use audio::AudioState;
pub use blend::BlendPattern;
pub use engine::{
    Blend, COMP_HEIGHT, COMP_WIDTH, Draw, Engine, FrameOutput, MESH_HEIGHT, MESH_WIDTH,
    ShaderValues, Vertex, blur_passes,
};
pub use library::{FamilyInfo, MilkLibrary};
pub use preset::{Family, Preset, Variant};
