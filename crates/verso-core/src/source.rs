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
    Noise,
    Voronoi,
    Checker,
    Tunnel,
    Starfield,
    Waves,
    Spiral,
    Metaballs,
}

impl Generator {
    pub const ALL: [Generator; 12] = [
        Generator::Plasma,
        Generator::Gradient,
        Generator::Stripes,
        Generator::Rings,
        Generator::Noise,
        Generator::Voronoi,
        Generator::Checker,
        Generator::Tunnel,
        Generator::Starfield,
        Generator::Waves,
        Generator::Spiral,
        Generator::Metaballs,
    ];

    /// Index passed to the generator shader.
    pub fn shader_index(self) -> u32 {
        match self {
            Generator::Plasma => 0,
            Generator::Gradient => 1,
            Generator::Stripes => 2,
            Generator::Rings => 3,
            Generator::Noise => 4,
            Generator::Voronoi => 5,
            Generator::Checker => 6,
            Generator::Tunnel => 7,
            Generator::Starfield => 8,
            Generator::Waves => 9,
            Generator::Spiral => 10,
            Generator::Metaballs => 11,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Generator::Plasma => "Plasma",
            Generator::Gradient => "Gradient",
            Generator::Stripes => "Stripes",
            Generator::Rings => "Rings",
            Generator::Noise => "Noise",
            Generator::Voronoi => "Voronoi",
            Generator::Checker => "Checker",
            Generator::Tunnel => "Tunnel",
            Generator::Starfield => "Starfield",
            Generator::Waves => "Waves",
            Generator::Spiral => "Spiral",
            Generator::Metaballs => "Metaballs",
        }
    }

    /// Parameters specific to this generator, in shader slot order. Their
    /// defaults reproduce the look the generator had before it had any.
    pub fn shape_params(self) -> &'static [ShapeParam] {
        const fn p(name: &'static str, default: f32, min: f32, max: f32) -> ShapeParam {
            ShapeParam {
                name,
                default,
                min,
                max,
                integer: false,
            }
        }
        const fn int(name: &'static str, default: f32, min: f32, max: f32) -> ShapeParam {
            ShapeParam {
                name,
                default,
                min,
                max,
                integer: true,
            }
        }
        match self {
            Generator::Plasma => {
                const P: &[ShapeParam] = &[
                    p("Complexity", 1.0, 0.1, 4.0),
                    p("Warp", 0.5, 0.0, 2.0),
                    p("Bands", 1.0, 0.25, 4.0),
                ];
                P
            }
            Generator::Gradient => {
                const P: &[ShapeParam] =
                    &[p("Repeats", 1.0, 0.25, 8.0), p("Radial", 0.0, 0.0, 1.0)];
                P
            }
            Generator::Stripes => {
                const P: &[ShapeParam] = &[
                    p("Count", 8.0, 1.0, 64.0),
                    p("Slant", 2.0, -16.0, 16.0),
                    p("Width", 0.5, 0.02, 0.98),
                    p("Softness", 0.0, 0.0, 0.5),
                ];
                P
            }
            Generator::Rings => {
                const P: &[ShapeParam] = &[
                    p("Count", 30.0, 1.0, 120.0),
                    p("Thickness", 0.5, 0.02, 0.98),
                    p("Hardness", 0.0, 0.0, 1.0),
                ];
                P
            }
            Generator::Noise => {
                const P: &[ShapeParam] = &[
                    int("Detail", 5.0, 1.0, 8.0),
                    p("Roughness", 0.5, 0.1, 0.9),
                    p("Frequency", 3.0, 0.5, 12.0),
                ];
                P
            }
            Generator::Voronoi => {
                const P: &[ShapeParam] = &[
                    p("Cells", 5.0, 1.0, 30.0),
                    p("Jitter", 0.4, 0.0, 0.5),
                    p("Edge width", 0.0, 0.0, 0.5),
                ];
                P
            }
            Generator::Checker => {
                const P: &[ShapeParam] = &[p("Squares", 6.0, 1.0, 40.0), p("Spin", 0.2, -2.0, 2.0)];
                P
            }
            Generator::Tunnel => {
                const P: &[ShapeParam] = &[
                    p("Depth", 0.3, 0.05, 2.0),
                    p("Rings", 2.0, 0.5, 10.0),
                    int("Ribs", 8.0, 0.0, 32.0),
                    p("Twist", 0.0, -4.0, 4.0),
                ];
                P
            }
            Generator::Starfield => {
                const P: &[ShapeParam] = &[
                    p("Density", 0.3, 0.0, 1.0),
                    p("Star size", 1.0, 0.2, 4.0),
                    int("Layers", 4.0, 1.0, 8.0),
                ];
                P
            }
            Generator::Waves => {
                const P: &[ShapeParam] = &[
                    p("Frequency", 40.0, 2.0, 200.0),
                    p("Spread", 0.3, 0.0, 1.0),
                    p("Contrast", 1.0, 0.0, 4.0),
                ];
                P
            }
            Generator::Spiral => {
                const P: &[ShapeParam] = &[
                    int("Arms", 4.0, 1.0, 16.0),
                    p("Tightness", 6.0, -20.0, 20.0),
                    p("Hardness", 0.0, 0.0, 1.0),
                ];
                P
            }
            Generator::Metaballs => {
                const P: &[ShapeParam] = &[
                    int("Count", 5.0, 1.0, 12.0),
                    p("Size", 1.0, 0.2, 4.0),
                    p("Orbit", 0.35, 0.0, 1.0),
                    p("Glow", 0.3, 0.0, 1.0),
                ];
                P
            }
        }
    }
}

/// A parameter specific to one [`Generator`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeParam {
    pub name: &'static str,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    /// Only whole values make sense (a count of arms, octaves...).
    pub integer: bool,
}

impl ShapeParam {
    /// Most specific parameters a generator has.
    pub const SLOTS: usize = 4;

    /// `value` made usable: finite, in range, whole if it must be.
    pub fn sanitize(&self, value: f32) -> f32 {
        if !value.is_finite() {
            return self.default;
        }
        let v = value.clamp(self.min, self.max);
        if self.integer { v.round() } else { v }
    }
}

/// Settings of a [`Source::Generator`]: a transform and a second color shared
/// by every kind, plus values specific to the kind. The defaults draw the
/// generator as it looked before it had parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneratorParams {
    /// Zoom: above 1 the pattern is larger.
    pub scale: f32,
    /// Rotation of the pattern, in degrees.
    pub angle: f32,
    /// Position of the pattern center, in frame heights.
    pub offset: [f32; 2],
    /// Color where the pattern is dark, straight RGBA. An alpha below 1
    /// makes the dark parts transparent.
    pub color2: [f32; 4],
    /// Values of [`Generator::shape_params`], in order; missing ones are at
    /// their default.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shape: Vec<f32>,
}

impl Default for GeneratorParams {
    fn default() -> Self {
        Self {
            scale: 1.0,
            angle: 0.0,
            offset: [0.0, 0.0],
            color2: [0.0, 0.0, 0.0, 1.0],
            shape: Vec::new(),
        }
    }
}

impl GeneratorParams {
    pub const MIN_SCALE: f32 = 0.05;
    pub const MAX_SCALE: f32 = 20.0;

    /// Value of the `index`th parameter of `kind`, sanitized; 0 past them.
    pub fn shape_value(&self, kind: Generator, index: usize) -> f32 {
        kind.shape_params().get(index).map_or(0.0, |param| {
            param.sanitize(self.shape.get(index).copied().unwrap_or(param.default))
        })
    }

    /// Every shader slot for `kind`.
    pub fn shape_values(&self, kind: Generator) -> [f32; ShapeParam::SLOTS] {
        std::array::from_fn(|i| self.shape_value(kind, i))
    }

    /// Sets the `index`th parameter of `kind`, filling earlier missing ones
    /// with their defaults.
    pub fn set_shape(&mut self, kind: Generator, index: usize, value: f32) {
        let params = kind.shape_params();
        if index >= params.len() {
            return;
        }
        while self.shape.len() <= index {
            self.shape.push(params[self.shape.len()].default);
        }
        self.shape[index] = params[index].sanitize(value);
    }

    /// Whether every value is finite.
    pub fn is_finite(&self) -> bool {
        self.scale.is_finite()
            && self.angle.is_finite()
            && self.offset.iter().all(|v| v.is_finite())
            && self.color2.iter().all(|v| v.is_finite())
            && self.shape.iter().all(|v| v.is_finite())
    }

    /// Brings every value back into a usable range for `kind`.
    pub fn repair(&mut self, kind: Generator) {
        let fix = |v: f32, default: f32, min: f32, max: f32| {
            if v.is_finite() {
                v.clamp(min, max)
            } else {
                default
            }
        };
        self.scale = fix(self.scale, 1.0, Self::MIN_SCALE, Self::MAX_SCALE);
        self.angle = fix(self.angle, 0.0, -3600.0, 3600.0);
        for v in &mut self.offset {
            *v = fix(*v, 0.0, -100.0, 100.0);
        }
        for v in &mut self.color2 {
            *v = fix(*v, 0.0, 0.0, 1.0);
        }
        let params = kind.shape_params();
        self.shape.truncate(params.len());
        for (v, param) in self.shape.iter_mut().zip(params) {
            *v = param.sanitize(*v);
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
        /// Absent in files saved before generators had parameters.
        #[serde(default)]
        params: GeneratorParams,
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
    /// Live video from an NDI® source on the network, by its full name
    /// (`MACHINE (Source)`).
    Ndi { name: String },
    /// Live video from a Spout sender on this computer (Windows), by its
    /// sender name.
    Spout { name: String },
    /// Text in a font, with an optional per-letter animation.
    Text {
        text: String,
        style: crate::text::TextStyle,
    },
    /// The time of day or a timer, drawn as text.
    Time {
        mode: crate::text::TimeMode,
        format: crate::text::TimeFormat,
        style: crate::text::TextStyle,
    },
}

fn default_milk_blend() -> f32 {
    Source::MILK_BLEND
}

impl Source {
    /// A generator with default parameters.
    pub fn generator(kind: Generator, color: [f32; 4], speed: f32) -> Source {
        Source::Generator {
            kind,
            color,
            speed,
            params: GeneratorParams::default(),
        }
    }

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
            | Source::Milkdrop { .. }
            | Source::Ndi { .. }
            | Source::Spout { .. }
            | Source::Text { .. }
            | Source::Time { .. } => None,
        }
    }

    /// Whether the GPU renders this source without any decoded media.
    pub fn is_procedural(&self) -> bool {
        matches!(
            self,
            Source::Solid { .. } | Source::Generator { .. } | Source::Shader(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_sources_round_trip() {
        for source in [
            Source::Ndi {
                name: "STUDIO (Cam)".into(),
            },
            Source::Spout {
                name: "TouchDesigner".into(),
            },
        ] {
            let text = ron::to_string(&source).unwrap();
            assert_eq!(ron::from_str::<Source>(&text).unwrap(), source);
        }
        // Files written before Spout input still load.
        let old: Source = ron::from_str(r#"Ndi(name: "STUDIO (Cam)")"#).unwrap();
        assert!(matches!(old, Source::Ndi { .. }));
        assert!(!Source::Spout { name: "x".into() }.is_procedural());
    }

    #[test]
    fn old_generator_without_params_loads_with_defaults() {
        let old = "Generator(kind: Rings, color: (1.0, 0.5, 0.25, 1.0), speed: 2.0)";
        let source: Source = ron::from_str(old).unwrap();
        assert_eq!(
            source,
            Source::generator(Generator::Rings, [1.0, 0.5, 0.25, 1.0], 2.0)
        );
        let Source::Generator { kind, params, .. } = &source else {
            unreachable!()
        };
        assert_eq!(params.shape_values(*kind), [30.0, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn generator_params_round_trip() {
        let mut params = GeneratorParams {
            scale: 2.0,
            angle: 45.0,
            offset: [0.1, -0.2],
            color2: [0.0, 0.0, 1.0, 0.5],
            shape: Vec::new(),
        };
        params.set_shape(Generator::Stripes, 2, 0.25);
        assert_eq!(params.shape, vec![8.0, 2.0, 0.25]);
        let source = Source::Generator {
            kind: Generator::Stripes,
            color: [1.0; 4],
            speed: 1.0,
            params,
        };
        let text = ron::to_string(&source).unwrap();
        assert_eq!(ron::from_str::<Source>(&text).unwrap(), source);
        // Partial params keep the other fields at their defaults.
        let partial: Source = ron::from_str(
            "Generator(kind: Plasma, color: (1, 1, 1, 1), speed: 1, params: (scale: 3))",
        )
        .unwrap();
        let Source::Generator { params, .. } = partial else {
            unreachable!()
        };
        assert_eq!(params.scale, 3.0);
        assert_eq!(params.color2, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn shape_values_are_sanitized() {
        let params = GeneratorParams {
            shape: vec![f32::NAN, 100.0, 2.6],
            ..GeneratorParams::default()
        };
        // Detail falls back to its default, roughness is clamped.
        assert_eq!(params.shape_value(Generator::Noise, 0), 5.0);
        assert_eq!(params.shape_value(Generator::Noise, 1), 0.9);
        assert_eq!(params.shape_value(Generator::Spiral, 0), 4.0);
        // Ribs are whole.
        assert_eq!(params.shape_value(Generator::Tunnel, 2), 3.0);
        for kind in Generator::ALL {
            assert!(kind.shape_params().len() <= ShapeParam::SLOTS);
        }
    }
}
