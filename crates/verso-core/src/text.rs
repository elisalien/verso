//! Look of text sources: font, size, placement and the per-letter
//! animation, and what time sources show. Rendered by `verso_text`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

impl TextAlign {
    pub const ALL: [TextAlign; 3] = [TextAlign::Left, TextAlign::Center, TextAlign::Right];

    pub fn name(self) -> &'static str {
        match self {
            TextAlign::Left => "Left",
            TextAlign::Center => "Center",
            TextAlign::Right => "Right",
        }
    }
}

/// How the letters move. Animations loop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAnimation {
    #[default]
    None,
    /// Letters appear one at a time.
    Typewriter,
    /// Letters fade in one after the other.
    FadeLetters,
    /// The text crosses the frame from right to left.
    Scroll,
    /// Letters hop in a wave.
    Bounce,
    /// Letters jump and flicker at random.
    Glitch,
    /// Letters grow bolder and thinner in a wave, as if the text breathed.
    /// Needs a variable font: the outlines are interpolated along its
    /// weight (and width) axes.
    Breather,
}

impl TextAnimation {
    pub const ALL: [TextAnimation; 7] = [
        TextAnimation::None,
        TextAnimation::Typewriter,
        TextAnimation::FadeLetters,
        TextAnimation::Scroll,
        TextAnimation::Bounce,
        TextAnimation::Glitch,
        TextAnimation::Breather,
    ];

    pub fn name(self) -> &'static str {
        match self {
            TextAnimation::None => "None",
            TextAnimation::Typewriter => "Typewriter",
            TextAnimation::FadeLetters => "Fade letters",
            TextAnimation::Scroll => "Scroll",
            TextAnimation::Bounce => "Bounce",
            TextAnimation::Glitch => "Glitch",
            TextAnimation::Breather => "Breather",
        }
    }
}

/// Regular weight.
pub const NORMAL_WEIGHT: u16 = 400;
/// Weight that `bold: true` stood for in older files.
pub const BOLD_WEIGHT: u16 = 700;

/// The standard weights and their names.
pub const WEIGHTS: [(u16, &str); 9] = [
    (100, "Thin"),
    (200, "Extra Light"),
    (300, "Light"),
    (400, "Regular"),
    (500, "Medium"),
    (600, "Semibold"),
    (700, "Bold"),
    (800, "Extra Bold"),
    (900, "Black"),
];

/// Name of a font weight, like "Bold 700".
pub fn weight_name(weight: u16) -> String {
    match WEIGHTS.iter().find(|(w, _)| *w == weight) {
        Some((w, name)) => format!("{name} {w}"),
        None => format!("Weight {weight}"),
    }
}

/// Settings of the Breather animation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Breath {
    /// Thinnest and boldest weights the letters reach, kept within what
    /// the font can do.
    pub min_weight: f32,
    pub max_weight: f32,
    /// Part of a breath between one letter and the next: 0 breathes all
    /// letters together, more sends a wave along the text.
    pub spread: f32,
    /// Letters also widen as they grow bolder, when the font has a width
    /// axis.
    pub width: bool,
}

impl Default for Breath {
    fn default() -> Self {
        Self {
            min_weight: 100.0,
            max_weight: 900.0,
            spread: 0.08,
            width: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "TextStyleFile", into = "TextStyleFile")]
pub struct TextStyle {
    /// Font family name; empty for the default sans-serif.
    pub font: String,
    /// Font weight, 100 (thin) to 900 (black).
    pub weight: u16,
    pub italic: bool,
    /// Font size as a fraction of the frame height.
    pub size: f32,
    /// Straight RGBA.
    pub color: [f32; 4],
    pub align: TextAlign,
    /// Center of the text, 0..1 from the top left of the frame.
    pub position: [f32; 2],
    /// Extra space between letters, in ems.
    pub letter_spacing: f32,
    /// Distance between lines, as a multiple of the size.
    pub line_height: f32,
    pub animation: TextAnimation,
    /// Speed of the animation, 1 is normal.
    pub speed: f32,
    pub breath: Breath,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font: String::new(),
            weight: NORMAL_WEIGHT,
            italic: false,
            size: 0.15,
            color: [1.0, 1.0, 1.0, 1.0],
            align: TextAlign::Center,
            position: [0.5, 0.5],
            letter_spacing: 0.0,
            line_height: 1.2,
            animation: TextAnimation::None,
            speed: 1.0,
            breath: Breath::default(),
        }
    }
}

/// [`TextStyle`] as saved: older files have `bold` instead of `weight`.
/// `bold` is still written, for older versions of Verso.
#[derive(Serialize, Deserialize)]
#[serde(default, rename = "TextStyle")]
struct TextStyleFile {
    font: String,
    bold: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    weight: Option<u16>,
    italic: bool,
    size: f32,
    color: [f32; 4],
    align: TextAlign,
    position: [f32; 2],
    letter_spacing: f32,
    line_height: f32,
    animation: TextAnimation,
    speed: f32,
    breath: Breath,
}

impl Default for TextStyleFile {
    fn default() -> Self {
        // No weight: an older file's `bold` decides.
        let mut file = Self::from(TextStyle::default());
        file.weight = None;
        file
    }
}

impl From<TextStyle> for TextStyleFile {
    fn from(s: TextStyle) -> Self {
        Self {
            font: s.font,
            bold: s.weight >= BOLD_WEIGHT,
            weight: Some(s.weight),
            italic: s.italic,
            size: s.size,
            color: s.color,
            align: s.align,
            position: s.position,
            letter_spacing: s.letter_spacing,
            line_height: s.line_height,
            animation: s.animation,
            speed: s.speed,
            breath: s.breath,
        }
    }
}

impl From<TextStyleFile> for TextStyle {
    fn from(f: TextStyleFile) -> Self {
        let weight = f
            .weight
            .unwrap_or(if f.bold { BOLD_WEIGHT } else { NORMAL_WEIGHT });
        Self {
            font: f.font,
            weight: weight.clamp(1, 1000),
            italic: f.italic,
            size: f.size,
            color: f.color,
            align: f.align,
            position: f.position,
            letter_spacing: f.letter_spacing,
            line_height: f.line_height,
            animation: f.animation,
            speed: f.speed,
            breath: f.breath,
        }
    }
}

impl TextStyle {
    /// Whether the values are usable (finite, in range).
    pub fn is_valid(&self) -> bool {
        self.size.is_finite()
            && self.size > 0.0
            && self.size <= 4.0
            && self.color.iter().all(|c| c.is_finite())
            && self.position.iter().all(|p| p.is_finite())
            && self.letter_spacing.is_finite()
            && self.line_height.is_finite()
            && self.line_height > 0.0
            && self.speed.is_finite()
            && self.breath.min_weight.is_finite()
            && self.breath.max_weight.is_finite()
            && self.breath.spread.is_finite()
    }
}

/// What a time source counts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TimeMode {
    /// Time of day on this computer.
    Clock,
    /// Time since the clip was launched.
    CountUp,
    /// Time left from `seconds` when the clip was launched, then zero.
    CountDown { seconds: f32 },
}

impl TimeMode {
    pub fn name(self) -> &'static str {
        match self {
            TimeMode::Clock => "Clock",
            TimeMode::CountUp => "Timer",
            TimeMode::CountDown { .. } => "Countdown",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeFormat {
    /// 13:45:07
    #[default]
    HoursMinutesSeconds,
    /// 13:45
    HoursMinutes,
    /// 05:07, minutes going past 59 for long timers.
    MinutesSeconds,
    /// 05:07.42
    Hundredths,
    /// 1:45:07 PM
    TwelveHour,
}

impl TimeFormat {
    pub const ALL: [TimeFormat; 5] = [
        TimeFormat::HoursMinutesSeconds,
        TimeFormat::HoursMinutes,
        TimeFormat::MinutesSeconds,
        TimeFormat::Hundredths,
        TimeFormat::TwelveHour,
    ];

    pub fn name(self) -> &'static str {
        match self {
            TimeFormat::HoursMinutesSeconds => "13:45:07",
            TimeFormat::HoursMinutes => "13:45",
            TimeFormat::MinutesSeconds => "05:07",
            TimeFormat::Hundredths => "05:07.42",
            TimeFormat::TwelveHour => "1:45:07 PM",
        }
    }
}

/// The time shown, as text: `elapsed` is the time since the clip was
/// launched and `clock` the time of day, both in seconds.
pub fn time_text(mode: TimeMode, format: TimeFormat, elapsed: f64, clock: f64) -> String {
    let seconds = match mode {
        TimeMode::Clock => clock,
        TimeMode::CountUp => elapsed,
        // Rounded up, so a countdown shows 0 only when it is over.
        TimeMode::CountDown { seconds } => {
            let left = (seconds as f64 - elapsed).max(0.0);
            match format {
                TimeFormat::Hundredths => left,
                _ => left.ceil(),
            }
        }
    }
    .max(0.0);
    let hundredths = (seconds * 100.0).floor() as u64;
    let whole = hundredths / 100;
    let (h, m, s) = (whole / 3600, whole / 60 % 60, whole % 60);
    match format {
        TimeFormat::HoursMinutesSeconds => format!("{h:02}:{m:02}:{s:02}"),
        TimeFormat::HoursMinutes => format!("{h:02}:{m:02}"),
        TimeFormat::MinutesSeconds => format!("{:02}:{s:02}", whole / 60),
        TimeFormat::Hundredths => format!("{:02}:{s:02}.{:02}", whole / 60, hundredths % 100),
        TimeFormat::TwelveHour => {
            let suffix = if h % 24 < 12 { "AM" } else { "PM" };
            let h12 = match h % 12 {
                0 => 12,
                h => h,
            };
            format!("{h12}:{m:02}:{s:02} {suffix}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        let clock = 13.0 * 3600.0 + 45.0 * 60.0 + 7.42;
        let text = |format| time_text(TimeMode::Clock, format, 0.0, clock);
        assert_eq!(text(TimeFormat::HoursMinutesSeconds), "13:45:07");
        assert_eq!(text(TimeFormat::HoursMinutes), "13:45");
        assert_eq!(text(TimeFormat::MinutesSeconds), "825:07");
        assert_eq!(text(TimeFormat::Hundredths), "825:07.42");
        assert_eq!(text(TimeFormat::TwelveHour), "1:45:07 PM");
        assert_eq!(
            time_text(TimeMode::Clock, TimeFormat::TwelveHour, 0.0, 30.0),
            "12:00:30 AM"
        );
    }

    #[test]
    fn bold_of_older_files_is_weight_700() {
        let old: TextStyle = ron::from_str(r#"(font: "Arial", bold: true)"#).unwrap();
        assert_eq!(old.weight, BOLD_WEIGHT);
        assert_eq!(old.font, "Arial");
        let plain: TextStyle = ron::from_str("()").unwrap();
        assert_eq!(plain, TextStyle::default());
        let light = TextStyle {
            weight: 300,
            animation: TextAnimation::Breather,
            ..Default::default()
        };
        let saved = ron::to_string(&light).unwrap();
        let back: TextStyle = ron::from_str(&saved).unwrap();
        assert_eq!(back, light);
        // A weight saved along with `bold` wins over it.
        let both: TextStyle = ron::from_str("(bold: true, weight: Some(500))").unwrap();
        assert_eq!(both.weight, 500);
        assert_eq!(weight_name(700), "Bold 700");
        assert_eq!(weight_name(350), "Weight 350");
    }

    #[test]
    fn timers() {
        let up = time_text(TimeMode::CountUp, TimeFormat::MinutesSeconds, 75.5, 0.0);
        assert_eq!(up, "01:15");
        let down = TimeMode::CountDown { seconds: 60.0 };
        assert_eq!(
            time_text(down, TimeFormat::MinutesSeconds, 0.2, 0.0),
            "01:00"
        );
        assert_eq!(
            time_text(down, TimeFormat::MinutesSeconds, 59.5, 0.0),
            "00:01"
        );
        assert_eq!(
            time_text(down, TimeFormat::MinutesSeconds, 90.0, 0.0),
            "00:00"
        );
        assert_eq!(
            time_text(down, TimeFormat::Hundredths, 59.5, 0.0),
            "00:00.50"
        );
    }
}
