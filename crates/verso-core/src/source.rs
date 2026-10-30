use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Effect;

/// Built-in generative sources rendered by a WGSL shader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Generator {
    Plasma,
    Gradient,
    Stripes,
    Rings,
}

impl Generator {
    pub const ALL: [Generator; 4] = [
        Generator::Plasma,
        Generator::Gradient,
        Generator::Stripes,
        Generator::Rings,
    ];

    /// Index passed to the generator shader.
    pub fn shader_index(self) -> u32 {
        match self {
            Generator::Plasma => 0,
            Generator::Gradient => 1,
            Generator::Stripes => 2,
            Generator::Rings => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Generator::Plasma => "Plasma",
            Generator::Gradient => "Gradient",
            Generator::Stripes => "Stripes",
            Generator::Rings => "Rings",
        }
    }
}

/// What produces the pixels of a clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Source {
    /// Flat color, straight (non-premultiplied) RGBA.
    Solid { color: [f32; 4] },
    /// Generative shader tinted by `color`, animated at `speed`.
    Generator {
        kind: Generator,
        color: [f32; 4],
        speed: f32,
    },
    /// Still image file.
    Image { path: PathBuf },
    /// Generator shader from the effect library (ISF or ShaderToy without an
    /// input image), with its parameters.
    Shader(Effect),
    /// Video file, played at `speed` (1 is normal speed).
    Video {
        path: PathBuf,
        speed: f32,
        looping: bool,
    },
}

impl Source {
    /// File the source reads, if any.
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            Source::Image { path } | Source::Video { path, .. } => Some(path),
            Source::Solid { .. } | Source::Generator { .. } | Source::Shader(_) => None,
        }
    }

    /// Whether the GPU renders this source without any decoded media.
    pub fn is_procedural(&self) -> bool {
        self.path().is_none()
    }
}
