//! Text sources: lays text out with cosmic-text, then draws each letter at
//! its own place and opacity for the animation presets, into an RGBA frame
//! the size of the composition.
//!
//! Layout happens only when the text, its look or the frame size changes;
//! drawing reuses the rasterized glyphs, so animated text costs a copy of
//! each letter per frame.
//!
//! The Breather animation changes the weight (and width) of each letter of
//! a variable font continuously: those letters are rasterized here with
//! swash at their own point of the font's axes, where cosmic-text only
//! knows whole weights and no width.

use std::collections::HashMap;
use std::path::Path;

use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style,
    SwashCache, SwashContent, SwashImage, Weight,
};
use swash::scale::{Render, ScaleContext, Source as GlyphSource, StrikeWith};
use swash::zeno::{Angle, Format, Transform, Vector};
use swash::{NormalizedCoord, Tag};
use verso_core::text::{TextAlign, TextAnimation, TextStyle, WEIGHTS};

const WGHT: Tag = Tag::from_be_bytes(*b"wght");
const WDTH: Tag = Tag::from_be_bytes(*b"wdth");

/// Seconds since midnight, local time.
pub fn local_seconds() -> f64 {
    use chrono::Timelike;
    let now = chrono::Local::now();
    now.num_seconds_from_midnight() as f64 + now.nanosecond() as f64 / 1e9
}

/// Rasterized glyphs kept before the cache starts over.
const GLYPH_CACHE: usize = 2048;

/// Fonts and rasterized glyphs, shared by every text source.
pub struct TextRenderer {
    fonts: FontSystem,
    glyphs: SwashCache,
    /// Scaler for letters drawn at their own point of the variation axes.
    scale: ScaleContext,
    varied: HashMap<VariedKey, Option<SwashImage>>,
    /// Bytes of the images in `varied`.
    varied_bytes: usize,
}

/// A rasterized letter of a variable font at one point of its axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct VariedKey {
    key: CacheKey,
    /// Quarters of a pixel of horizontal offset.
    x_quarter: u8,
    /// Weight and width in [`AXIS_STEPS`] per unit.
    wght: i32,
    wdth: i32,
}

/// Steps per unit of axis the letters are rasterized at: half a weight
/// unit, so the outlines change continuously to the eye.
const AXIS_STEPS: f32 = 2.0;
/// Memory kept for letters rasterized at varied weights; the cache starts
/// over beyond it.
const VARIED_BUDGET: usize = 48 << 20;

/// A font family and the weights it can draw.
#[derive(Debug, Clone, PartialEq)]
pub struct FontFamily {
    pub name: String,
    /// Weights of its static faces, sorted.
    pub weights: Vec<u16>,
    /// Range of the weight axis of its variable faces.
    pub weight_axis: Option<(f32, f32)>,
    /// Whether a variable face also has a width axis.
    pub width_axis: bool,
}

impl FontFamily {
    /// Whether its weight can vary continuously (a variable font with a
    /// `wght` axis).
    pub fn is_variable(&self) -> bool {
        self.weight_axis.is_some()
    }

    /// Weights to offer: those of the static faces, and the standard ones
    /// within the weight axis.
    pub fn weight_choices(&self) -> Vec<u16> {
        let mut choices = self.weights.clone();
        if let Some((lo, hi)) = self.weight_axis {
            choices.extend(
                WEIGHTS
                    .iter()
                    .map(|(w, _)| *w)
                    .filter(|w| (lo..=hi).contains(&(*w as f32))),
            );
            for end in [lo, hi] {
                if end > 0.0 && end.fract() == 0.0 {
                    choices.push(end as u16);
                }
            }
        }
        choices.sort_unstable();
        choices.dedup();
        choices
    }
}

/// A letter placed in the frame, at rest.
#[derive(Debug, Clone, Copy)]
struct Letter {
    key: CacheKey,
    /// Glyph origin in frame pixels.
    x: i32,
    y: i32,
    /// Line of the text it is on.
    line: usize,
}

/// Laid out text, ready to draw at any time of its animation.
pub struct Layout {
    key: (String, TextStyle, (u32, u32)),
    letters: Vec<Letter>,
    /// Size of the text block in pixels.
    block: (f32, f32),
    /// Font size in pixels.
    size: f32,
}

/// Pixel rectangle `(x0, y0, x1, y1)`, ends excluded.
type Bounds = (u32, u32, u32, u32);

/// RGBA pixels, straight alpha, rows top to bottom.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// Premultiplied pixels being drawn.
    work: Vec<f32>,
    /// Pixels drawn last time: only they need clearing.
    dirty: Option<Bounds>,
}

impl Canvas {
    pub fn new(size: (u32, u32)) -> Self {
        let n = size.0 as usize * size.1 as usize * 4;
        Self {
            width: size.0,
            height: size.1,
            data: vec![0; n],
            work: vec![0.0; n],
            dirty: None,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRenderer {
    /// Loads the system fonts (takes a moment).
    pub fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
            glyphs: SwashCache::new(),
            scale: ScaleContext::new(),
            varied: HashMap::new(),
            varied_bytes: 0,
        }
    }

    /// Adds the fonts found in `dir` (`.ttf`, `.otf`, `.ttc`).
    pub fn load_fonts_dir(&mut self, dir: &Path) {
        self.fonts.db_mut().load_fonts_dir(dir);
    }

    /// Font families and their weights, sorted by name. Reads every font
    /// file: call it once the fonts are loaded, off the UI thread.
    pub fn families(&self) -> Vec<FontFamily> {
        let db = self.fonts.db();
        let mut families: HashMap<String, FontFamily> = HashMap::new();
        for face in db.faces() {
            let Some((name, _)) = face.families.first() else {
                continue;
            };
            let axes = db
                .with_face_data(face.id, |data, index| {
                    let font = swash::FontRef::from_index(data, index as usize)?;
                    let vars = font.variations();
                    let wght = vars
                        .find_by_tag(WGHT)
                        .map(|a| (a.min_value(), a.max_value()));
                    Some((wght, vars.find_by_tag(WDTH).is_some()))
                })
                .flatten();
            let family = families
                .entry(name.to_lowercase())
                .or_insert_with(|| FontFamily {
                    name: name.clone(),
                    weights: Vec::new(),
                    weight_axis: None,
                    width_axis: false,
                });
            match axes {
                Some((Some((lo, hi)), width)) if hi > lo => {
                    let (a, b) = family.weight_axis.unwrap_or((lo, hi));
                    family.weight_axis = Some((a.min(lo), b.max(hi)));
                    family.width_axis |= width;
                }
                _ => family.weights.push(face.weight.0),
            }
        }
        let mut families: Vec<FontFamily> = families.into_values().collect();
        for family in &mut families {
            family.weights.sort_unstable();
            family.weights.dedup();
        }
        families.sort_by_key(|f| f.name.to_lowercase());
        families
    }

    /// Whether `layout` still matches the text, look and frame size.
    pub fn is_current(layout: &Layout, text: &str, style: &TextStyle, size: (u32, u32)) -> bool {
        layout.key.0 == text && same_layout(&layout.key.1, style) && layout.key.2 == size
    }

    /// Lays `text` out for a frame of `size`.
    pub fn layout(&mut self, text: &str, style: &TextStyle, size: (u32, u32)) -> Layout {
        let font_px = (style.size * size.1 as f32).max(1.0);
        let metrics = Metrics::new(font_px, font_px * style.line_height.max(0.1));
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        let family = if style.font.is_empty() {
            Family::SansSerif
        } else {
            Family::Name(&style.font)
        };
        let attrs = Attrs::new()
            .family(family)
            .weight(Weight(style.weight))
            .style(if style.italic {
                Style::Italic
            } else {
                Style::Normal
            })
            .letter_spacing(style.letter_spacing);
        buffer.set_size(None, None);
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fonts, false);

        let runs: Vec<_> = buffer.layout_runs().collect();
        let width = runs.iter().map(|r| r.line_w).fold(0.0, f32::max);
        let height = runs.len() as f32 * metrics.line_height;
        let left = style.position[0] * size.0 as f32 - width / 2.0;
        let top = style.position[1] * size.1 as f32 - height / 2.0;
        let mut letters = Vec::new();
        for (line, run) in runs.iter().enumerate() {
            let indent = match style.align {
                TextAlign::Left => 0.0,
                TextAlign::Center => (width - run.line_w) / 2.0,
                TextAlign::Right => width - run.line_w,
            };
            for glyph in run.glyphs {
                let physical = glyph.physical((left + indent, top + run.line_y), 1.0);
                letters.push(Letter {
                    key: physical.cache_key,
                    x: physical.x,
                    y: physical.y,
                    line,
                });
            }
        }
        Layout {
            key: (text.to_owned(), style.clone(), size),
            letters,
            block: (width, height),
            size: font_px,
        }
    }

    /// Draws `layout` into `canvas` at `time` seconds of its animation,
    /// with the color and animation of `style`.
    pub fn draw(&mut self, layout: &Layout, style: &TextStyle, time: f32, canvas: &mut Canvas) {
        // The glyph cache never evicts, and every size is a new entry:
        // dragging a size slider would fill memory. Start over when it is
        // large; the letters on screen are rasterized again below.
        if self.glyphs.image_cache.len() > GLYPH_CACHE {
            self.glyphs.image_cache.clear();
        }
        let (w, h) = (canvas.width as i32, canvas.height as i32);
        let previous = canvas.dirty.take();
        if let Some((x0, y0, x1, y1)) = previous {
            for y in y0..y1 {
                let row = (y * canvas.width) as usize * 4;
                canvas.work[row + x0 as usize * 4..row + x1 as usize * 4].fill(0.0);
            }
        }
        let color = style.color;
        let count = layout.letters.len();
        let breaths = if style.animation == TextAnimation::Breather {
            self.breathe(layout, style, time)
        } else {
            Vec::new()
        };
        let mut bounds: Option<Bounds> = None;
        for (index, letter) in layout.letters.iter().enumerate() {
            let pose = animate(style.animation, index, count, time, layout, canvas.width);
            if pose.alpha <= 0.0 {
                continue;
            }
            let (image, x) = match breaths.get(index) {
                Some(breath) => {
                    // Where the letter is now, in quarters of a pixel.
                    let at = letter.x as f32 + letter.key.x_bin.as_float() + breath.dx;
                    let quarters = (at * 4.0).round() as i64;
                    let x = quarters.div_euclid(4) as i32;
                    let key = VariedKey {
                        key: letter.key,
                        x_quarter: quarters.rem_euclid(4) as u8,
                        wght: (breath.wght * AXIS_STEPS).round() as i32,
                        wdth: (breath.wdth * AXIS_STEPS).round() as i32,
                    };
                    (self.varied_image(key, &breath.coords), x)
                }
                None => (
                    self.glyphs.get_image(&mut self.fonts, letter.key).as_ref(),
                    letter.x,
                ),
            };
            let Some(image) = image else {
                continue;
            };
            let p = image.placement;
            let gx = x + p.left + pose.dx.round() as i32;
            let gy = letter.y - p.top + pose.dy.round() as i32;
            let gw = p.width as i32;
            let (x0, y0) = (gx.max(0), gy.max(0));
            let (x1, y1) = ((gx + gw).min(w), (gy + p.height as i32).min(h));
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            let drawn = (x0 as u32, y0 as u32, x1 as u32, y1 as u32);
            bounds = Some(union(bounds, drawn));
            let alpha = pose.alpha * color[3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y - gy) * gw + (x - gx)) as usize;
                    let src = match image.content {
                        SwashContent::Mask => {
                            let a = image.data[i] as f32 / 255.0 * alpha;
                            [color[0] * a, color[1] * a, color[2] * a, a]
                        }
                        SwashContent::SubpixelMask => {
                            let a = image.data[i * 4] as f32 / 255.0 * alpha;
                            [color[0] * a, color[1] * a, color[2] * a, a]
                        }
                        SwashContent::Color => {
                            let c = &image.data[i * 4..i * 4 + 4];
                            let a = c[3] as f32 / 255.0 * alpha;
                            let channel = |v: u8| v as f32 / 255.0 * a;
                            [channel(c[0]), channel(c[1]), channel(c[2]), a]
                        }
                    };
                    let o = ((y * w + x) * 4) as usize;
                    let dst = &mut canvas.work[o..o + 4];
                    for c in 0..4 {
                        dst[c] = src[c] + dst[c] * (1.0 - src[3]);
                    }
                }
            }
        }
        // Back to straight alpha in 8 bits, over the pixels drawn now or last
        // time.
        let touched = match (previous, bounds) {
            (Some(p), b) => Some(union(b, p)),
            (None, b) => b,
        };
        if let Some((x0, y0, x1, y1)) = touched {
            for y in y0..y1 {
                for x in x0..x1 {
                    let o = ((y * canvas.width + x) * 4) as usize;
                    let p = &canvas.work[o..o + 4];
                    let a = p[3];
                    let out = &mut canvas.data[o..o + 4];
                    if a <= 0.0 {
                        out.copy_from_slice(&[0, 0, 0, 0]);
                    } else {
                        for c in 0..3 {
                            out[c] = ((p[c] / a).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                        }
                        out[3] = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    }
                }
            }
        }
        canvas.dirty = bounds;
    }

    /// Weight, width and shift of each letter of `layout` at `time` of
    /// the Breather animation. The letters of a line move apart as they
    /// widen, and the line keeps its alignment.
    fn breathe(&mut self, layout: &Layout, style: &TextStyle, time: f32) -> Vec<Breath> {
        let settings = &style.breath;
        let (lo, hi) = (
            settings.min_weight.min(settings.max_weight),
            settings.min_weight.max(settings.max_weight),
        );
        let mut breaths: Vec<Breath> = Vec::with_capacity(layout.letters.len());
        // Growth of the line so far, and per line at the end.
        let mut grown = 0.0;
        let mut line_growth = Vec::new();
        for (index, letter) in layout.letters.iter().enumerate() {
            while line_growth.len() < letter.line {
                line_growth.push(grown);
                grown = 0.0;
            }
            let phase = time * BREATHS_PER_SECOND - index as f32 * settings.spread;
            let amount = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
            let key = letter.key;
            let Some(font) = self.fonts.get_font(key.font_id, key.font_weight) else {
                breaths.push(Breath::rest(grown));
                continue;
            };
            let font = font.as_swash();
            let vars = font.variations();
            let rest_weight = f32::from(key.font_weight.0);
            let (wght, rest_wght) = match vars.find_by_tag(WGHT) {
                // `clamp` panics on a font whose axis range is backwards.
                Some(axis) if axis.min_value() <= axis.max_value() => {
                    let clamp = |w: f32| w.clamp(axis.min_value(), axis.max_value());
                    (clamp(lo + (hi - lo) * amount), clamp(rest_weight))
                }
                _ => (rest_weight, rest_weight),
            };
            let (wdth, rest_wdth) = match vars.find_by_tag(WDTH) {
                Some(axis) => {
                    let (min, default, max) =
                        (axis.min_value(), axis.default_value(), axis.max_value());
                    // Narrow when thin, normal when bold; or normal to wide
                    // for fonts that only widen.
                    let (narrow, wide) = if default > min {
                        (min, default)
                    } else {
                        (default, max)
                    };
                    let width = if settings.width {
                        narrow + (wide - narrow) * amount
                    } else {
                        default
                    };
                    (width, default)
                }
                None => (0.0, 0.0),
            };
            let coords: Vec<NormalizedCoord> = vars
                .normalized_coords([(WGHT, wght), (WDTH, wdth)])
                .collect();
            let rest: Vec<NormalizedCoord> = vars
                .normalized_coords([(WGHT, rest_wght), (WDTH, rest_wdth)])
                .collect();
            let size = f32::from_bits(key.font_size_bits);
            let advance = |coords: &[NormalizedCoord]| {
                font.glyph_metrics(coords)
                    .scale(size)
                    .advance_width(key.glyph_id)
            };
            let change = advance(&coords) - advance(&rest);
            breaths.push(Breath {
                dx: grown,
                wght,
                wdth,
                coords,
            });
            grown += change;
        }
        line_growth.push(grown);
        let keep = match style.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => 0.5,
            TextAlign::Right => 1.0,
        };
        for (breath, letter) in breaths.iter_mut().zip(&layout.letters) {
            breath.dx -= line_growth.get(letter.line).copied().unwrap_or(0.0) * keep;
        }
        breaths
    }

    /// The letter of `key` rasterized at `coords` of its font's axes.
    fn varied_image(&mut self, key: VariedKey, coords: &[NormalizedCoord]) -> Option<&SwashImage> {
        if !self.varied.contains_key(&key) {
            if self.varied_bytes > VARIED_BUDGET {
                self.varied.clear();
                self.varied_bytes = 0;
            }
            let image = self.rasterize(key, coords);
            self.varied_bytes += image.as_ref().map_or(0, |i| i.data.len()) + 64;
            self.varied.insert(key, image);
        }
        self.varied.get(&key)?.as_ref()
    }

    fn rasterize(&mut self, key: VariedKey, coords: &[NormalizedCoord]) -> Option<SwashImage> {
        let glyph = key.key;
        let font = self.fonts.get_font(glyph.font_id, glyph.font_weight)?;
        let mut scaler = self
            .scale
            .builder(font.as_swash())
            .size(f32::from_bits(glyph.font_size_bits))
            .hint(false)
            .normalized_coords(coords)
            .build();
        let skew = glyph
            .flags
            .contains(CacheKeyFlags::FAKE_ITALIC)
            .then(|| Transform::skew(Angle::from_degrees(14.0), Angle::from_degrees(0.0)));
        Render::new(&[
            GlyphSource::ColorOutline(0),
            GlyphSource::ColorBitmap(StrikeWith::BestFit),
            GlyphSource::Outline,
        ])
        .format(Format::Alpha)
        .offset(Vector::new(
            f32::from(key.x_quarter) / 4.0,
            glyph.y_bin.as_float(),
        ))
        .transform(skew)
        .render(&mut scaler, glyph.glyph_id)
    }
}

/// One letter of the Breather animation.
#[derive(Debug, Clone)]
struct Breath {
    /// Shift from its place at rest, in pixels.
    dx: f32,
    /// Weight and width it is drawn at.
    wght: f32,
    wdth: f32,
    /// The same, as the font's normalized coordinates.
    coords: Vec<NormalizedCoord>,
}

impl Breath {
    /// A letter of a font that cannot vary.
    fn rest(dx: f32) -> Self {
        Self {
            dx,
            wght: 0.0,
            wdth: 0.0,
            coords: Vec::new(),
        }
    }
}

/// Breaths per second of the Breather animation at speed 1.
const BREATHS_PER_SECOND: f32 = 0.5;

fn union(a: Option<Bounds>, b: Bounds) -> Bounds {
    match a {
        Some(a) => (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)),
        None => b,
    }
}

/// Only these fields change the layout; the others are applied when drawing.
fn same_layout(a: &TextStyle, b: &TextStyle) -> bool {
    a.font == b.font
        && a.weight == b.weight
        && a.italic == b.italic
        && a.size == b.size
        && a.align == b.align
        && a.position == b.position
        && a.letter_spacing == b.letter_spacing
        && a.line_height == b.line_height
}

/// Offset and opacity of one letter.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pose {
    dx: f32,
    dy: f32,
    alpha: f32,
}

const REST: Pose = Pose {
    dx: 0.0,
    dy: 0.0,
    alpha: 1.0,
};

/// Letters per second of the typewriter and fade animations.
const LETTERS_PER_SECOND: f32 = 12.0;
/// Seconds the full text stays before an animation starts again.
const HOLD: f32 = 2.0;
/// Seconds each letter takes to fade in.
const LETTER_FADE: f32 = 0.4;

fn hash(a: u64, b: u64) -> f32 {
    let mut x = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// Where letter `index` of `count` is at `time` seconds of `animation`.
fn animate(
    animation: TextAnimation,
    index: usize,
    count: usize,
    time: f32,
    layout: &Layout,
    frame_width: u32,
) -> Pose {
    let i = index as f32;
    let n = count as f32;
    match animation {
        // The Breather moves letters in `breathe`.
        TextAnimation::None | TextAnimation::Breather => REST,
        TextAnimation::Typewriter => {
            let cycle = n / LETTERS_PER_SECOND + HOLD;
            let shown = (time.rem_euclid(cycle) * LETTERS_PER_SECOND).floor();
            Pose {
                alpha: if i < shown { 1.0 } else { 0.0 },
                ..REST
            }
        }
        TextAnimation::FadeLetters => {
            let cycle = n / LETTERS_PER_SECOND + LETTER_FADE + HOLD;
            let t = time.rem_euclid(cycle) - i / LETTERS_PER_SECOND;
            Pose {
                alpha: (t / LETTER_FADE).clamp(0.0, 1.0),
                ..REST
            }
        }
        TextAnimation::Scroll => {
            // From just off the right edge to just off the left edge, at a
            // quarter of the frame width per second.
            let width = frame_width as f32;
            let left = layout.key.1.position[0] * width - layout.block.0 / 2.0;
            let travel = (time * 0.25 * width).rem_euclid(width + layout.block.0);
            Pose {
                dx: width - travel - left,
                ..REST
            }
        }
        TextAnimation::Bounce => {
            let phase = time * 2.0 - i * 0.12;
            Pose {
                dy: -(phase * std::f32::consts::PI).sin().abs() * 0.3 * layout.size,
                ..REST
            }
        }
        TextAnimation::Glitch => {
            let step = (time * 12.0).floor() as i64 as u64;
            let roll = hash(index as u64, step);
            if roll < 0.06 {
                return Pose { alpha: 0.0, ..REST };
            }
            if roll < 0.3 {
                let jx = hash(index as u64 + 7919, step) - 0.5;
                let jy = hash(index as u64 + 104_729, step) - 0.5;
                return Pose {
                    dx: jx * 0.4 * layout.size,
                    dy: jy * 0.2 * layout.size,
                    alpha: 0.5 + roll,
                };
            }
            REST
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pixels more than half opaque, and their bounds.
    fn coverage(canvas: &Canvas) -> (usize, Option<Bounds>) {
        let mut count = 0;
        let mut bounds = None;
        for y in 0..canvas.height {
            for x in 0..canvas.width {
                if canvas.data[((y * canvas.width + x) * 4 + 3) as usize] > 128 {
                    count += 1;
                    bounds = Some(union(bounds, (x, y, x + 1, y + 1)));
                }
            }
        }
        (count, bounds)
    }

    fn renderer() -> Option<TextRenderer> {
        let renderer = TextRenderer::new();
        if renderer.families().is_empty() {
            eprintln!("no font on this system: skipped");
            return None;
        }
        Some(renderer)
    }

    #[test]
    fn text_is_drawn_centered() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let style = TextStyle::default();
        let size = (320, 180);
        let layout = renderer.layout("VERSO", &style, size);
        let mut canvas = Canvas::new(size);
        renderer.draw(&layout, &style, 0.0, &mut canvas);
        let (count, bounds) = coverage(&canvas);
        assert!(count > 200, "{count} pixels");
        let (x0, y0, x1, y1) = bounds.unwrap();
        let (cx, cy) = ((x0 + x1) as f32 / 2.0, (y0 + y1) as f32 / 2.0);
        assert!((cx - 160.0).abs() < 12.0, "center x {cx}");
        assert!((cy - 90.0).abs() < 15.0, "center y {cy}");
    }

    #[test]
    fn typewriter_shows_more_letters_over_time() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let style = TextStyle {
            animation: TextAnimation::Typewriter,
            ..Default::default()
        };
        let size = (320, 180);
        let layout = renderer.layout("TYPEWRITER", &style, size);
        let mut canvas = Canvas::new(size);
        renderer.draw(&layout, &style, 0.0, &mut canvas);
        assert_eq!(coverage(&canvas).0, 0);
        renderer.draw(&layout, &style, 0.3, &mut canvas);
        let early = coverage(&canvas).0;
        renderer.draw(&layout, &style, 0.8, &mut canvas);
        let late = coverage(&canvas).0;
        assert!(early > 0 && late > early, "{early} then {late}");
        // Earlier pixels are cleared when the canvas is drawn again.
        renderer.draw(&layout, &style, 0.0, &mut canvas);
        assert_eq!(coverage(&canvas).0, 0);
    }

    #[test]
    fn scroll_moves_the_text_and_bounce_lifts_letters() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let size = (320, 180);
        let mut canvas = Canvas::new(size);
        let scroll = TextStyle {
            animation: TextAnimation::Scroll,
            ..Default::default()
        };
        let layout = renderer.layout("GO", &scroll, size);
        renderer.draw(&layout, &scroll, 1.0, &mut canvas);
        let a = coverage(&canvas).1.unwrap().0;
        renderer.draw(&layout, &scroll, 2.0, &mut canvas);
        let b = coverage(&canvas).1.unwrap().0;
        assert!(b < a, "scrolls left: {a} then {b}");

        let still = TextStyle::default();
        let layout = renderer.layout("HOP", &still, size);
        renderer.draw(&layout, &still, 0.25, &mut canvas);
        let rest = coverage(&canvas).1.unwrap().1;
        let bounce = TextStyle {
            animation: TextAnimation::Bounce,
            ..Default::default()
        };
        renderer.draw(&layout, &bounce, 0.25, &mut canvas);
        let up = coverage(&canvas).1.unwrap().1;
        assert!(up < rest, "bounces up: {rest} then {up}");
    }

    /// A variable font with a weight axis, Bahnschrift if there is one.
    fn variable_family(renderer: &TextRenderer) -> Option<FontFamily> {
        let families = renderer.families();
        let pick = families
            .iter()
            .find(|f| f.name == "Bahnschrift" && f.is_variable())
            .or_else(|| families.iter().find(|f| f.is_variable()))
            .cloned();
        if pick.is_none() {
            eprintln!("no variable font on this system: skipped");
        }
        pick
    }

    #[test]
    fn families_list_their_weights() {
        let Some(renderer) = renderer() else {
            return;
        };
        let families = renderer.families();
        assert!(families.iter().all(|f| !f.weight_choices().is_empty()));
        let names: Vec<_> = families.iter().map(|f| f.name.to_lowercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names, sorted, "sorted, one entry per family");
        let variable = FontFamily {
            name: "V".into(),
            weights: vec![],
            weight_axis: Some((300.0, 700.0)),
            width_axis: true,
        };
        assert_eq!(variable.weight_choices(), vec![300, 400, 500, 600, 700]);
    }

    #[test]
    fn weight_thickens_the_letters() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let Some(family) = variable_family(&renderer) else {
            return;
        };
        let (lo, hi) = family.weight_axis.unwrap();
        let size = (320, 180);
        let ink = |renderer: &mut TextRenderer, weight: f32| {
            let style = TextStyle {
                font: family.name.clone(),
                weight: weight as u16,
                ..Default::default()
            };
            let layout = renderer.layout("WEIGHT", &style, size);
            let mut canvas = Canvas::new(size);
            renderer.draw(&layout, &style, 0.0, &mut canvas);
            coverage(&canvas).0
        };
        let (thin, bold) = (ink(&mut renderer, lo), ink(&mut renderer, hi));
        assert!(bold > thin * 5 / 4, "{} {thin} then {bold}", family.name);
    }

    #[test]
    fn breather_varies_the_weight_continuously() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let Some(family) = variable_family(&renderer) else {
            return;
        };
        let (lo, hi) = family.weight_axis.unwrap();
        let mut style = TextStyle {
            font: family.name.clone(),
            animation: TextAnimation::Breather,
            ..Default::default()
        };
        style.breath.min_weight = lo;
        style.breath.max_weight = hi;
        style.breath.spread = 0.0;
        let size = (320, 180);
        let layout = renderer.layout("BREATHE", &style, size);
        let mut canvas = Canvas::new(size);
        // A breath takes two seconds: thinnest at 0, boldest at 1.
        let mut ink = |time: f32| {
            renderer.draw(&layout, &style, time, &mut canvas);
            coverage(&canvas)
        };
        let (thin, thin_bounds) = ink(0.0);
        let (half, _) = ink(0.5);
        let (bold, bold_bounds) = ink(1.0);
        assert!(thin < half && half < bold, "{thin} {half} {bold}");
        // The line widens around its center as the letters grow.
        let (t, b) = (thin_bounds.unwrap(), bold_bounds.unwrap());
        assert!(b.2 - b.0 >= t.2 - t.0, "{t:?} then {b:?}");
        let center = |b: Bounds| (b.0 + b.2) as f32 / 2.0;
        assert!((center(t) - center(b)).abs() < 8.0, "{t:?} then {b:?}");
        // Back to the start after a whole breath.
        assert_eq!(ink(2.0).0, thin);
    }

    #[test]
    fn layout_is_kept_until_the_look_changes() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let style = TextStyle::default();
        let layout = renderer.layout("A", &style, (64, 64));
        let recolored = TextStyle {
            color: [1.0, 0.0, 0.0, 1.0],
            animation: TextAnimation::Glitch,
            ..style.clone()
        };
        assert!(TextRenderer::is_current(&layout, "A", &recolored, (64, 64)));
        let bigger = TextStyle {
            size: 0.3,
            ..style.clone()
        };
        assert!(!TextRenderer::is_current(&layout, "A", &bigger, (64, 64)));
        assert!(!TextRenderer::is_current(&layout, "B", &style, (64, 64)));
    }
}
