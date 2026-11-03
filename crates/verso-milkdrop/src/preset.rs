//! MilkDrop presets in the Butterchurn JSON format, and preset families.
//!
//! A Butterchurn preset keeps the original EEL code (`*_eqs_eel`) and the
//! warp and comp shaders already translated from HLSL to GLSL.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Named numeric values of a preset, shape or wave.
pub type Values = BTreeMap<String, f64>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Shape {
    #[serde(rename = "baseVals", default)]
    pub base_vals: Values,
    #[serde(default)]
    pub init_eqs_eel: String,
    #[serde(default)]
    pub frame_eqs_eel: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Wave {
    #[serde(rename = "baseVals", default)]
    pub base_vals: Values,
    #[serde(default)]
    pub init_eqs_eel: String,
    #[serde(default)]
    pub frame_eqs_eel: String,
    #[serde(default)]
    pub point_eqs_eel: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Preset {
    #[serde(default)]
    pub version: u32,
    #[serde(rename = "baseVals", default)]
    pub base_vals: Values,
    #[serde(default)]
    pub shapes: Vec<Shape>,
    #[serde(default)]
    pub waves: Vec<Wave>,
    #[serde(default)]
    pub init_eqs_eel: String,
    #[serde(default)]
    pub frame_eqs_eel: String,
    #[serde(default)]
    pub pixel_eqs_eel: String,
    /// GLSL warp shader (`shader_body { ... }`); empty means the default.
    #[serde(default)]
    pub warp: String,
    /// GLSL composite shader; empty means the default.
    #[serde(default)]
    pub comp: String,
}

impl Preset {
    pub fn from_json(json: &str) -> Result<Preset, String> {
        serde_json::from_str(json).map_err(|e| format!("bad preset JSON: {e}"))
    }

    /// Image textures the shaders sample besides the built-in ones.
    pub fn user_textures(&self) -> Vec<String> {
        let mut found = Vec::new();
        for shader in [&self.warp, &self.comp] {
            for line in shader.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("uniform sampler2D sampler_") {
                    let name = rest.trim_end_matches(';').trim().to_owned();
                    if !BUILTIN_SAMPLERS.contains(&name.as_str()) && !found.contains(&name) {
                        found.push(name);
                    }
                }
            }
        }
        found
    }
}

/// Samplers every warp and comp shader can use without declaring them.
pub const BUILTIN_SAMPLERS: &[&str] = &[
    "main",
    "fw_main",
    "fc_main",
    "pw_main",
    "pc_main",
    "blur1",
    "blur2",
    "blur3",
    "noise_lq",
    "noise_lq_lite",
    "noise_mq",
    "noise_hq",
    "pw_noise_lq",
    "noisevol_lq",
    "noisevol_hq",
];

/// Butterchurn defaults for preset values missing from a file.
pub const PRESET_DEFAULTS: &[(&str, f64)] = &[
    ("decay", 0.98),
    ("gammaadj", 2.0),
    ("echo_zoom", 2.0),
    ("echo_alpha", 0.0),
    ("echo_orient", 0.0),
    ("red_blue", 0.0),
    ("brighten", 0.0),
    ("darken", 0.0),
    ("wrap", 1.0),
    ("darken_center", 0.0),
    ("solarize", 0.0),
    ("invert", 0.0),
    ("bmotionvectorson", 1.0),
    ("fshader", 0.0),
    ("b1n", 0.0),
    ("b2n", 0.0),
    ("b3n", 0.0),
    ("b1x", 1.0),
    ("b2x", 1.0),
    ("b3x", 1.0),
    ("b1ed", 0.25),
    ("wave_mode", 0.0),
    ("additivewave", 0.0),
    ("wave_dots", 0.0),
    ("wave_thick", 0.0),
    ("wave_a", 0.8),
    ("wave_scale", 1.0),
    ("wave_smoothing", 0.75),
    ("wave_mystery", 0.0),
    ("modwavealphabyvolume", 0.0),
    ("modwavealphastart", 0.75),
    ("modwavealphaend", 0.95),
    ("wave_r", 1.0),
    ("wave_g", 1.0),
    ("wave_b", 1.0),
    ("wave_x", 0.5),
    ("wave_y", 0.5),
    ("wave_brighten", 1.0),
    ("mv_x", 12.0),
    ("mv_y", 9.0),
    ("mv_dx", 0.0),
    ("mv_dy", 0.0),
    ("mv_l", 0.9),
    ("mv_r", 1.0),
    ("mv_g", 1.0),
    ("mv_b", 1.0),
    ("mv_a", 1.0),
    ("warpanimspeed", 1.0),
    ("warpscale", 1.0),
    ("zoomexp", 1.0),
    ("zoom", 1.0),
    ("rot", 0.0),
    ("cx", 0.5),
    ("cy", 0.5),
    ("dx", 0.0),
    ("dy", 0.0),
    ("warp", 1.0),
    ("sx", 1.0),
    ("sy", 1.0),
    ("ob_size", 0.01),
    ("ob_r", 0.0),
    ("ob_g", 0.0),
    ("ob_b", 0.0),
    ("ob_a", 0.0),
    ("ib_size", 0.01),
    ("ib_r", 0.25),
    ("ib_g", 0.25),
    ("ib_b", 0.25),
    ("ib_a", 0.0),
];

pub const SHAPE_DEFAULTS: &[(&str, f64)] = &[
    ("enabled", 0.0),
    ("sides", 4.0),
    ("additive", 0.0),
    ("thickoutline", 0.0),
    ("textured", 0.0),
    ("num_inst", 1.0),
    ("tex_zoom", 1.0),
    ("tex_ang", 0.0),
    ("x", 0.5),
    ("y", 0.5),
    ("rad", 0.1),
    ("ang", 0.0),
    ("r", 1.0),
    ("g", 0.0),
    ("b", 0.0),
    ("a", 1.0),
    ("r2", 0.0),
    ("g2", 1.0),
    ("b2", 0.0),
    ("a2", 0.0),
    ("border_r", 1.0),
    ("border_g", 1.0),
    ("border_b", 1.0),
    ("border_a", 0.1),
];

pub const WAVE_DEFAULTS: &[(&str, f64)] = &[
    ("enabled", 0.0),
    ("samples", 512.0),
    ("sep", 0.0),
    ("scaling", 1.0),
    ("smoothing", 0.5),
    ("r", 1.0),
    ("g", 1.0),
    ("b", 1.0),
    ("a", 1.0),
    ("spectrum", 0.0),
    ("usedots", 0.0),
    ("thick", 0.0),
    ("additive", 0.0),
];

/// `defaults` overridden by the matching keys of `vals`; other keys are
/// dropped, like Butterchurn does.
pub fn with_defaults(defaults: &[(&str, f64)], vals: &Values) -> Values {
    defaults
        .iter()
        .map(|(k, d)| ((*k).to_owned(), vals.get(*k).copied().unwrap_or(*d)))
        .collect()
}

/// One preset among the variations of a family.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variant {
    pub name: String,
    pub preset: Preset,
}

/// Remixes of the same preset, offered as one source with several presets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Family {
    pub name: String,
    #[serde(default)]
    pub category: String,
    /// Preset authors, in credit order.
    #[serde(default)]
    pub authors: Vec<String>,
    /// Where the presets come from (pack and folder).
    #[serde(default)]
    pub source: String,
    pub variants: Vec<Variant>,
}

impl Family {
    pub fn from_json(json: &str) -> Result<Family, String> {
        serde_json::from_str(json).map_err(|e| format!("bad preset family JSON: {e}"))
    }
}
