use serde::{Deserialize, Serialize};

use crate::{Error, Generator, Result, Source};

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
    pub master: Master,
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
            master: Master::default(),
        }
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
                *cell = Some(Clip {
                    name: kind.name().to_owned(),
                    source: Source::Generator {
                        kind,
                        color: palette[(c + l * 2) % palette.len()],
                        speed: 0.5 + 0.25 * l as f32,
                    },
                });
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
        let Some(layer) = self.layers.get(index) else {
            return 0.0;
        };
        let any_solo = self.layers.iter().any(|l| l.solo);
        if layer.bypass || (any_solo && !layer.solo) || layer.active_clip().is_none() {
            return 0.0;
        }
        layer.opacity * self.master.fader_level(layer.fader)
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
        let layer = self.layer_mut(layer)?;
        // Triggering an empty cell ejects the layer.
        layer.active = layer.clips[column].as_ref().map(|_| column);
        Ok(())
    }

    pub(crate) fn trigger_column(&mut self, column: usize) -> Result<()> {
        self.check_column(column)?;
        for layer in &mut self.layers {
            layer.active = layer.clips[column].as_ref().map(|_| column);
        }
        Ok(())
    }

    pub(crate) fn add_layer(&mut self) {
        let name = format!("Layer {}", self.layers.len() + 1);
        self.layers.push(Layer::new(name, self.columns.len()));
    }

    pub(crate) fn add_column(&mut self) {
        self.columns.push(Column {
            name: format!("Column {}", self.columns.len() + 1),
        });
        for layer in &mut self.layers {
            layer.clips.push(None);
        }
    }
}
