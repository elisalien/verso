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

/// Shape of a MilkDrop preset blend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MilkPattern {
    /// Wipe, plasma or radial, picked at each blend as MilkDrop does.
    #[default]
    Random,
    Wipe,
    Plasma,
    Radial,
}

impl MilkPattern {
    pub const ALL: [MilkPattern; 4] = [
        MilkPattern::Random,
        MilkPattern::Wipe,
        MilkPattern::Plasma,
        MilkPattern::Radial,
    ];

    pub fn name(self) -> &'static str {
        match self {
            MilkPattern::Random => "Random",
            MilkPattern::Wipe => "Wipe",
            MilkPattern::Plasma => "Plasma",
            MilkPattern::Radial => "Radial",
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
    /// MilkDrop preset: `variant` of the preset family `family` (an id from
    /// the MilkDrop library, `<category>/<name>`).
    Milkdrop {
        family: String,
        variant: usize,
        /// Seconds of MilkDrop blend when the variant changes; 0 cuts.
        #[serde(default = "default_milk_blend")]
        blend: f32,
        /// Shape of the blend.
        #[serde(default)]
        pattern: MilkPattern,
        /// Layer whose image the preset reads as its image textures.
        #[serde(default)]
        input: Option<usize>,
        /// How much of the input layer is mixed into the feedback each frame.
        #[serde(default)]
        input_mix: f32,
    },
    /// Video file, played at `speed` (1 is normal speed).
    Video {
        path: PathBuf,
        speed: f32,
        looping: bool,
    },
}

fn default_milk_blend() -> f32 {
    Source::MILK_BLEND
}

impl Source {
    /// Default MilkDrop blend time, in seconds.
    pub const MILK_BLEND: f32 = 2.5;

    /// A MilkDrop source with default playback options.
    pub fn milkdrop(family: &str, variant: usize) -> Source {
        Source::Milkdrop {
            family: family.to_owned(),
            variant,
            blend: Source::MILK_BLEND,
            pattern: MilkPattern::Random,
            input: None,
            input_mix: 0.0,
        }
    }

    /// File the source reads, if any.
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            Source::Image { path } | Source::Video { path, .. } => Some(path),
            Source::Solid { .. }
            | Source::Generator { .. }
            | Source::Shader(_)
            | Source::Milkdrop { .. } => None,
        }
    }

    /// Whether the GPU renders this source without any decoded media.
    pub fn is_procedural(&self) -> bool {
        self.path().is_none()
    }
}
