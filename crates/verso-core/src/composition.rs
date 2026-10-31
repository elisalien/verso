use serde::{Deserialize, Serialize};

use crate::control::{Action, MidiMapping, Param};
use crate::modulation::Modulator;
use crate::{Effect, EffectTarget, Error, Generator, Result, Source};

/// How a layer is mixed onto the layers below it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendMode {
    Alpha,
    #[default]
    Add,
    Multiply,
    Screen,
    Lighten,
    Darken,
    Difference,
}

impl BlendMode {
    pub const ALL: [BlendMode; 7] = [
        BlendMode::Alpha,
        BlendMode::Add,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Lighten,
        BlendMode::Darken,
        BlendMode::Difference,
    ];

    /// Index passed to the blend shader.
    pub fn shader_index(self) -> u32 {
        match self {
            BlendMode::Alpha => 0,
            BlendMode::Add => 1,
            BlendMode::Multiply => 2,
            BlendMode::Screen => 3,
            BlendMode::Lighten => 4,
            BlendMode::Darken => 5,
            BlendMode::Difference => 6,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BlendMode::Alpha => "Alpha",
            BlendMode::Add => "Add",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Lighten => "Lighten",
            BlendMode::Darken => "Darken",
            BlendMode::Difference => "Difference",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub name: String,
    pub source: Source,
    #[serde(default)]
    pub effects: Vec<Effect>,
}

impl Clip {
    pub fn new(name: impl Into<String>, source: Source) -> Self {
        Self {
            name: name.into(),
            source,
            effects: Vec::new(),
        }
    }
}

/// Lua automation saved with the composition; see the `verso-script` crate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Script {
    pub name: String,
    pub source: String,
    pub enabled: bool,
}

/// Layers mixed together first, then onto the composition as one image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    pub blend: BlendMode,
    pub opacity: f32,
    pub bypass: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// Group containing this one.
    #[serde(default)]
    pub parent: Option<usize>,
    /// Member layers hidden in the grid.
    #[serde(default)]
    pub collapsed: bool,
}

impl Group {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            blend: BlendMode::Alpha,
            opacity: 1.0,
            bypass: false,
            solo: false,
            effects: Vec::new(),
            parent: None,
            collapsed: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
}

/// Which side of the master crossfader a layer follows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FaderSide {
    /// Ignores the crossfader.
    #[default]
    None,
    A,
    B,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub blend: BlendMode,
    pub opacity: f32,
    pub bypass: bool,
    /// When any layer is soloed, only soloed layers are output.
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub fader: FaderSide,
    /// One cell per column; `None` is an empty cell.
    pub clips: Vec<Option<Clip>>,
    /// Column of the playing clip.
    pub active: Option<usize>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// Crossfade time in seconds when the playing clip changes; 0 cuts.
    #[serde(default)]
    pub transition: f32,
    /// Index into [`Composition::groups`].
    #[serde(default)]
    pub group: Option<usize>,
    /// Times the clip of each column was launched. Belongs to the slot, not
    /// to the media: replacing the clip keeps the count.
    #[serde(default)]
    pub launches: Vec<u32>,
}

impl Layer {
    pub fn new(name: impl Into<String>, columns: usize) -> Self {
        Self {
            name: name.into(),
            blend: BlendMode::default(),
            opacity: 1.0,
            bypass: false,
            solo: false,
            fader: FaderSide::None,
            clips: vec![None; columns],
            active: None,
            effects: Vec::new(),
            transition: 0.0,
            group: None,
            launches: vec![0; columns],
        }
    }

    /// Times the slot in `column` was launched.
    pub fn launch_count(&self, column: usize) -> u32 {
        self.launches.get(column).copied().unwrap_or(0)
    }

    /// Plays the clip in `column`, counting the launch; an empty slot ejects.
    pub(crate) fn launch(&mut self, column: usize) {
        self.active = self.clips[column].as_ref().map(|_| column);
        if self.active.is_some() {
            if self.launches.len() <= column {
                self.launches.resize(column + 1, 0);
            }
            self.launches[column] += 1;
        }
    }

    /// The clip currently playing, if any.
    pub fn active_clip(&self) -> Option<&Clip> {
        self.active.and_then(|c| self.clips.get(c)?.as_ref())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Master {
    pub opacity: f32,
    /// 0 is full A, 1 is full B. Both sides are at full level at 0.5.
    #[serde(default = "Master::default_crossfader")]
    pub crossfader: f32,
}

impl Master {
    fn default_crossfader() -> f32 {
        0.5
    }

    /// Level applied to layers on `side`.
    pub fn fader_level(&self, side: FaderSide) -> f32 {
        match side {
            FaderSide::None => 1.0,
            FaderSide::A => (2.0 * (1.0 - self.crossfader)).clamp(0.0, 1.0),
            FaderSide::B => (2.0 * self.crossfader).clamp(0.0, 1.0),
        }
    }
}

impl Default for Master {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            crossfader: Self::default_crossfader(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub bpm: f32,
    pub columns: Vec<Column>,
    /// Bottom layer first.
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub groups: Vec<Group>,
    pub master: Master,
    #[serde(default)]
    pub effects: Vec<Effect>,
    #[serde(default)]
    pub modulators: Vec<Modulator>,
    #[serde(default)]
    pub midi: Vec<MidiMapping>,
    #[serde(default)]
    pub scripts: Vec<Script>,
}

impl Composition {
    pub fn new(name: impl Into<String>, layers: usize, columns: usize) -> Self {
        Self {
            name: name.into(),
            width: 1920,
            height: 1080,
            bpm: 120.0,
            columns: (1..=columns)
                .map(|i| Column {
                    name: format!("Column {i}"),
                })
                .collect(),
            layers: (1..=layers)
                .map(|i| Layer::new(format!("Layer {i}"), columns))
                .collect(),
            groups: Vec::new(),
            master: Master::default(),
            effects: Vec::new(),
            modulators: Vec::new(),
            midi: Vec::new(),
            scripts: Vec::new(),
        }
    }

    /// Like [`Self::demo`] with `layers` layers.
    pub fn demo_with_layers(layers: usize) -> Self {
        let mut comp = Self::demo();
        while comp.layers.len() < layers {
            comp.add_layer();
        }
        comp.layers.truncate(layers);
        comp
    }

    /// A small composition with generators in every cell, useful to try the app.
    pub fn demo() -> Self {
        let mut comp = Self::new("Demo", 3, 6);
        let palette = [
            [1.0, 0.25, 0.4, 1.0],
            [0.2, 0.6, 1.0, 1.0],
            [1.0, 0.8, 0.2, 1.0],
            [0.4, 1.0, 0.6, 1.0],
            [0.8, 0.3, 1.0, 1.0],
            [1.0, 1.0, 1.0, 1.0],
        ];
        for (l, layer) in comp.layers.iter_mut().enumerate() {
            for (c, cell) in layer.clips.iter_mut().enumerate() {
                let kind = Generator::ALL[(l + c) % Generator::ALL.len()];
                *cell = Some(Clip::new(
                    kind.name(),
                    Source::Generator {
                        kind,
                        color: palette[(c + l * 2) % palette.len()],
                        speed: 0.5 + 0.25 * l as f32,
                    },
                ));
            }
        }
        comp.layers[0].blend = BlendMode::Alpha;
        comp.layers[1].blend = BlendMode::Screen;
        comp.layers[2].opacity = 0.5;
        comp
    }

    /// Final opacity of a layer in the output: its opacity, crossfader, bypass and
    /// solo combined. Zero when the layer is not playing.
    pub fn layer_gain(&self, index: usize) -> f32 {
        match self.layers.get(index) {
            Some(layer) if layer.active_clip().is_some() => self.layer_level(index),
            _ => 0.0,
        }
    }

    /// Like [`Self::layer_gain`], but whether a clip plays is ignored. Used to
    /// fade out a clip that was just ejected.
    pub fn layer_level(&self, index: usize) -> f32 {
        let Some(layer) = self.layers.get(index) else {
            return 0.0;
        };
        if layer.bypass || (self.any_solo() && !self.soloed(index)) {
            return 0.0;
        }
        layer.opacity * self.master.fader_level(layer.fader)
    }

    /// Opacity of a group onto the composition.
    pub fn group_gain(&self, index: usize) -> f32 {
        match self.groups.get(index) {
            Some(group) if !group.bypass => group.opacity,
            _ => 0.0,
        }
    }

    pub fn effects(&self, target: EffectTarget) -> Option<&Vec<Effect>> {
        match target {
            EffectTarget::Composition => Some(&self.effects),
            EffectTarget::Group(g) => self.groups.get(g).map(|g| &g.effects),
            EffectTarget::Layer(l) => self.layers.get(l).map(|l| &l.effects),
            EffectTarget::Clip { layer, column } => self
                .layers
                .get(layer)?
                .clips
                .get(column)?
                .as_ref()
                .map(|c| &c.effects),
        }
    }

    pub(crate) fn effects_mut(&mut self, target: EffectTarget) -> Result<&mut Vec<Effect>> {
        Ok(match target {
            EffectTarget::Composition => &mut self.effects,
            EffectTarget::Group(g) => &mut self.groups.get_mut(g).ok_or(Error::NoGroup(g))?.effects,
            EffectTarget::Layer(l) => &mut self.layer_mut(l)?.effects,
            EffectTarget::Clip { layer, column } => {
                &mut self
                    .layer_mut(layer)?
                    .clips
                    .get_mut(column)
                    .ok_or(Error::NoColumn(column))?
                    .as_mut()
                    .ok_or(Error::NoClip)?
                    .effects
            }
        })
    }

    pub fn group_mut(&mut self, group: usize) -> Result<&mut Group> {
        self.groups.get_mut(group).ok_or(Error::NoGroup(group))
    }

    pub fn layer_mut(&mut self, layer: usize) -> Result<&mut Layer> {
        self.layers.get_mut(layer).ok_or(Error::NoLayer(layer))
    }

    fn check_column(&self, column: usize) -> Result<()> {
        if column < self.columns.len() {
            Ok(())
        } else {
            Err(Error::NoColumn(column))
        }
    }

    pub fn to_ron(&self) -> Result<String> {
        Ok(ron::ser::to_string_pretty(
            self,
            ron::ser::PrettyConfig::default(),
        )?)
    }

    pub fn from_ron(text: &str) -> Result<Self> {
        Ok(ron::from_str(text)?)
    }

    pub(crate) fn trigger_clip(&mut self, layer: usize, column: usize) -> Result<()> {
        self.check_column(column)?;
        self.layer_mut(layer)?.launch(column);
        Ok(())
    }

    pub(crate) fn trigger_column(&mut self, column: usize) -> Result<()> {
        self.check_column(column)?;
        for layer in &mut self.layers {
            layer.launch(column);
        }
        Ok(())
    }

    pub(crate) fn add_layer(&mut self) {
        let name = format!("Layer {}", self.layers.len() + 1);
        self.layers.push(Layer::new(name, self.columns.len()));
    }

    /// Deletes `layer`. References to it (modulators, MIDI mappings) are
    /// dropped and references to the layers above move down. Groups left
    /// without layers are dissolved.
    pub(crate) fn remove_layer(&mut self, layer: usize) -> Result<()> {
        if layer >= self.layers.len() {
            return Err(Error::NoLayer(layer));
        }
        if self.layers.len() == 1 {
            return Err(Error::LastLayer);
        }
        self.layers.remove(layer);
        self.prune(
            |p| p.without_layer(layer),
            |a| a.without_layer(layer),
        );
        while let Some(empty) = (0..self.groups.len()).find(|&g| self.group_layers(g).next().is_none())
        {
            self.ungroup(empty)?;
        }
        Ok(())
    }

    /// Deletes `column` and the clips in it on every layer. A layer playing
    /// that column is ejected.
    pub(crate) fn remove_column(&mut self, column: usize) -> Result<()> {
        self.check_column(column)?;
        if self.columns.len() == 1 {
            return Err(Error::LastColumn);
        }
        self.columns.remove(column);
        for layer in &mut self.layers {
            if column < layer.clips.len() {
                layer.clips.remove(column);
            }
            if column < layer.launches.len() {
                layer.launches.remove(column);
            }
            layer.active = match layer.active {
                Some(c) if c == column => None,
                Some(c) if c > column => Some(c - 1),
                other => other,
            };
        }
        self.prune(
            |p| p.without_column(column),
            |a| a.without_column(column),
        );
        Ok(())
    }

    /// Rewrites or drops every modulator and MIDI mapping after an index
    /// change: `param` and `action` return the new target, or `None` to drop.
    pub(crate) fn prune(
        &mut self,
        param: impl Fn(&Param) -> Option<Param>,
        action: impl Fn(&Action) -> Option<Action>,
    ) {
        self.modulators.retain_mut(|m| match param(&m.param) {
            Some(p) => {
                m.param = p;
                true
            }
            None => false,
        });
        self.midi.retain_mut(|m| match action(&m.action) {
            Some(a) => {
                m.action = a;
                true
            }
            None => false,
        });
    }

    pub(crate) fn add_column(&mut self) {
        self.columns.push(Column {
            name: format!("Column {}", self.columns.len() + 1),
        });
        for layer in &mut self.layers {
            layer.clips.push(None);
            layer.launches.resize(layer.clips.len(), 0);
        }
    }
}
