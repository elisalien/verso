//! Fine parameters of the enhanced blend modes.
//!
//! An enhanced mode declares its parameters as a small table of
//! [`BlendParamSpec`]; the renderer packs their values in table order into
//! the blend shader's uniform. A new enhanced mode only adds a table here and
//! its shader code.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::BlendMode;

/// Values the blend shader receives per pass.
pub const BLEND_PARAM_SLOTS: usize = 8;

/// How a parameter is edited.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BlendParamKind {
    /// A continuous value.
    Float,
    /// An angle in degrees.
    Angle,
    /// One of several options, stored as its index.
    Choice(&'static [&'static str]),
}

/// One parameter of an enhanced blend mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlendParamSpec {
    /// Stable identifier, saved in files and used in control addresses.
    pub name: &'static str,
    pub label: &'static str,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    pub kind: BlendParamKind,
    pub help: &'static str,
}

impl BlendParamSpec {
    const fn float(
        name: &'static str,
        label: &'static str,
        default: f32,
        min: f32,
        max: f32,
        help: &'static str,
    ) -> Self {
        Self {
            name,
            label,
            default,
            min,
            max,
            kind: BlendParamKind::Float,
            help,
        }
    }

    const fn angle(name: &'static str, label: &'static str, help: &'static str) -> Self {
        Self {
            name,
            label,
            default: 0.0,
            min: -180.0,
            max: 180.0,
            kind: BlendParamKind::Angle,
            help,
        }
    }

    const fn choice(
        name: &'static str,
        label: &'static str,
        options: &'static [&'static str],
        help: &'static str,
    ) -> Self {
        Self {
            name,
            label,
            default: 0.0,
            min: 0.0,
            max: (options.len() - 1) as f32,
            kind: BlendParamKind::Choice(options),
            help,
        }
    }

    /// `value` brought into range; choices snap to an option.
    pub fn clamp(&self, value: f32) -> f32 {
        let v = value.clamp(self.min, self.max);
        match self.kind {
            BlendParamKind::Choice(_) => v.round(),
            _ => v,
        }
    }

    pub fn is_toggle(&self) -> bool {
        matches!(self.kind, BlendParamKind::Choice(options) if options.len() == 2)
    }
}

/// Channel of the layer read as a map.
const MAP_SOURCES: &[&str] = &["Luminance", "Red", "Green", "Blue", "Alpha"];
/// What a sample read outside the image below gets.
const EDGES: &[&str] = &["Clamp", "Mirror"];

const LIQUID_DISPLACE: &[BlendParamSpec] = &[
    BlendParamSpec::float(
        "strength",
        "Strength",
        1.0,
        0.0,
        4.0,
        "Multiplies the bend; opacity fades it too",
    ),
    BlendParamSpec::float(
        "radius",
        "Scale",
        1.0,
        0.25,
        8.0,
        "Distance the slope is measured over: larger is smoother, wider bends",
    ),
    BlendParamSpec::float(
        "prism",
        "Prism",
        1.0,
        0.0,
        3.0,
        "Spread of red and blue around green; 0 bends all colors alike",
    ),
    BlendParamSpec::float(
        "smoothness",
        "Smoothness",
        0.25,
        0.0,
        2.0,
        "Softens steep edges so they do not tear the image",
    ),
    BlendParamSpec::angle("angle", "Angle", "Rotates the direction of the bend"),
    BlendParamSpec::float(
        "flow",
        "Flow",
        0.0,
        0.0,
        2.0,
        "Animated ripple of the map over time",
    ),
    BlendParamSpec::choice("map", "Map", MAP_SOURCES, "Channel of the layer that bends"),
    BlendParamSpec::choice(
        "edge",
        "Edges",
        EDGES,
        "Pixels pulled from outside the image",
    ),
];

const CHROMA_SHIFT: &[BlendParamSpec] = &[
    BlendParamSpec::float(
        "amount",
        "Amount",
        0.02,
        0.0,
        0.2,
        "Shift of red and blue at full map, in screen heights",
    ),
    BlendParamSpec::angle("angle", "Angle", "Direction of the shift"),
    BlendParamSpec::float(
        "bias",
        "Bias",
        0.0,
        0.0,
        1.0,
        "Shift applied everywhere, even where the map is dark",
    ),
    BlendParamSpec::choice(
        "map",
        "Map",
        MAP_SOURCES,
        "Channel of the layer that shifts",
    ),
    BlendParamSpec::choice(
        "edge",
        "Edges",
        EDGES,
        "Pixels pulled from outside the image",
    ),
];

const GLOW_ADD: &[BlendParamSpec] = &[
    BlendParamSpec::float(
        "threshold",
        "Threshold",
        0.5,
        0.0,
        1.0,
        "Brightness the layer starts to glow from",
    ),
    BlendParamSpec::float(
        "softness",
        "Softness",
        0.2,
        0.0,
        1.0,
        "Width of the threshold knee",
    ),
    BlendParamSpec::float(
        "radius",
        "Radius",
        4.0,
        0.0,
        32.0,
        "Glow spread in pixels at 1080 lines",
    ),
    BlendParamSpec::float(
        "intensity",
        "Intensity",
        1.5,
        0.0,
        8.0,
        "Gain of the added light",
    ),
];

const DISSOLVE_NOISE: &[BlendParamSpec] = &[
    BlendParamSpec::float(
        "grain",
        "Grain",
        2.0,
        1.0,
        64.0,
        "Noise cell size in pixels at 1080 lines",
    ),
    BlendParamSpec::float(
        "softness",
        "Softness",
        0.0,
        0.0,
        1.0,
        "0 dissolves pixel by pixel, higher fades through the noise",
    ),
    BlendParamSpec::float(
        "speed",
        "Speed",
        0.0,
        0.0,
        30.0,
        "New noise patterns per second; 0 keeps one pattern",
    ),
    BlendParamSpec::float("seed", "Seed", 0.0, 0.0, 100.0, "Picks another pattern"),
];

impl BlendMode {
    /// Fine parameters of the mode, in shader order; empty for plain modes.
    pub fn params(self) -> &'static [BlendParamSpec] {
        match self {
            BlendMode::LiquidDisplace => LIQUID_DISPLACE,
            BlendMode::ChromaShift => CHROMA_SHIFT,
            BlendMode::GlowAdd => GLOW_ADD,
            BlendMode::DissolveNoise => DISSOLVE_NOISE,
            _ => &[],
        }
    }

    pub fn param(self, name: &str) -> Option<&'static BlendParamSpec> {
        self.params().iter().find(|p| p.name == name)
    }

    /// Modes with fine parameters.
    pub fn is_enhanced(self) -> bool {
        !self.params().is_empty()
    }

    pub fn category(self) -> BlendCategory {
        match self {
            BlendMode::AlphaMask
            | BlendMode::LumaMask
            | BlendMode::InvertedLumaMask
            | BlendMode::InvertedAlphaMask => BlendCategory::Mask,
            m if m.is_enhanced() => BlendCategory::Enhanced,
            _ => BlendCategory::Normal,
        }
    }
}

/// Groups of blend modes in menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendCategory {
    Normal,
    Mask,
    Enhanced,
}

impl BlendCategory {
    pub const ALL: [BlendCategory; 3] = [
        BlendCategory::Normal,
        BlendCategory::Mask,
        BlendCategory::Enhanced,
    ];

    pub fn name(self) -> &'static str {
        match self {
            BlendCategory::Normal => "Normal",
            BlendCategory::Mask => "Mask",
            BlendCategory::Enhanced => "Enhanced",
        }
    }

    /// Modes of the category, in [`BlendMode::ALL`] order.
    pub fn modes(self) -> impl Iterator<Item = BlendMode> {
        BlendMode::ALL
            .into_iter()
            .filter(move |m| m.category() == self)
    }
}

/// Fine parameter values of a layer or group, per mode, so switching modes
/// back and forth keeps each mode's settings. Only values that differ from
/// their default are stored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlendParams(BTreeMap<BlendMode, BTreeMap<String, f32>>);

impl BlendParams {
    /// No value differs from its default.
    pub fn is_default(&self) -> bool {
        self.0.values().all(BTreeMap::is_empty)
    }

    /// Value of `name` for `mode`, its default when not set; `None` when the
    /// mode has no such parameter.
    pub fn get(&self, mode: BlendMode, name: &str) -> Option<f32> {
        let spec = mode.param(name)?;
        Some(
            self.0
                .get(&mode)
                .and_then(|values| values.get(name))
                .map_or(spec.default, |v| spec.clamp(*v)),
        )
    }

    /// Sets `name` of `mode`, clamped to its range. Returns `false` when the
    /// mode has no such parameter.
    pub fn set(&mut self, mode: BlendMode, name: &str, value: f32) -> bool {
        let Some(spec) = mode.param(name) else {
            return false;
        };
        let value = spec.clamp(value);
        let values = self.0.entry(mode).or_default();
        if value == spec.default {
            values.remove(name);
        } else {
            values.insert(name.to_owned(), value);
        }
        if values.is_empty() {
            self.0.remove(&mode);
        }
        true
    }

    /// Puts every parameter of `mode` back to its default.
    pub fn reset(&mut self, mode: BlendMode) {
        self.0.remove(&mode);
    }

    /// Whether every parameter of `mode` is at its default.
    pub fn is_mode_default(&self, mode: BlendMode) -> bool {
        mode.params()
            .iter()
            .all(|p| self.get(mode, p.name) == Some(p.default))
    }

    /// Values of `mode` in shader order.
    pub fn pack(&self, mode: BlendMode) -> [f32; BLEND_PARAM_SLOTS] {
        let mut out = [0.0; BLEND_PARAM_SLOTS];
        for (slot, spec) in out.iter_mut().zip(mode.params()) {
            *slot = self.get(mode, spec.name).unwrap_or(spec.default);
        }
        out
    }

    /// Whether every stored value is finite.
    pub fn is_finite(&self) -> bool {
        self.0
            .values()
            .flat_map(BTreeMap::values)
            .all(|v| v.is_finite())
    }

    /// Drops values that are not finite or belong to no parameter. Returns
    /// whether anything was dropped.
    pub fn repair(&mut self) -> bool {
        let mut changed = false;
        for (mode, values) in &mut self.0 {
            let before = values.len();
            values.retain(|name, v| v.is_finite() && mode.param(name).is_some());
            changed |= values.len() != before;
        }
        self.0.retain(|_, values| !values.is_empty());
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_fit_the_shader_and_have_valid_defaults() {
        for mode in BlendMode::ALL {
            let specs = mode.params();
            assert!(specs.len() <= BLEND_PARAM_SLOTS, "{}", mode.name());
            for (i, spec) in specs.iter().enumerate() {
                assert!(spec.min < spec.max, "{}", spec.name);
                assert_eq!(spec.clamp(spec.default), spec.default, "{}", spec.name);
                assert!(
                    specs[..i].iter().all(|s| s.name != spec.name),
                    "duplicate {}",
                    spec.name
                );
            }
        }
        assert!(BlendMode::LiquidDisplace.is_enhanced());
        assert!(!BlendMode::Add.is_enhanced());
        assert_eq!(BlendMode::LumaMask.category(), BlendCategory::Mask);
        let all: usize = BlendCategory::ALL.iter().map(|c| c.modes().count()).sum();
        assert_eq!(all, BlendMode::ALL.len());
    }

    #[test]
    fn values_are_kept_per_mode_and_defaults_are_not_stored() {
        let mut params = BlendParams::default();
        assert!(params.is_default());
        assert!(params.set(BlendMode::LiquidDisplace, "strength", 2.0));
        assert!(!params.set(BlendMode::LiquidDisplace, "nope", 2.0));
        assert!(!params.set(BlendMode::Add, "strength", 2.0));
        assert_eq!(params.get(BlendMode::LiquidDisplace, "strength"), Some(2.0));
        assert_eq!(params.get(BlendMode::LiquidDisplace, "prism"), Some(1.0));
        // Clamped to the range; choices snap.
        params.set(BlendMode::LiquidDisplace, "prism", 99.0);
        assert_eq!(params.get(BlendMode::LiquidDisplace, "prism"), Some(3.0));
        params.set(BlendMode::LiquidDisplace, "map", 1.6);
        assert_eq!(params.get(BlendMode::LiquidDisplace, "map"), Some(2.0));
        assert_eq!(params.pack(BlendMode::LiquidDisplace)[0], 2.0);
        assert!(params.is_mode_default(BlendMode::GlowAdd));
        // Back to the defaults: nothing left to store.
        for name in ["strength", "prism", "map"] {
            let default = BlendMode::LiquidDisplace.param(name).unwrap().default;
            params.set(BlendMode::LiquidDisplace, name, default);
        }
        assert!(params.is_default());
        assert_eq!(ron::to_string(&params).unwrap(), "{}");
    }

    #[test]
    fn serializes_by_mode_and_name() {
        let mut params = BlendParams::default();
        params.set(BlendMode::GlowAdd, "intensity", 3.0);
        let text = ron::to_string(&params).unwrap();
        assert_eq!(text, "{GlowAdd:{\"intensity\":3.0}}");
        let back: BlendParams = ron::from_str(&text).unwrap();
        assert_eq!(back, params);
    }
}
