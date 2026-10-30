//! Effect definitions: Verso's WGSL effect format, bundled effects and ISF import.
//!
//! # WGSL effect format
//!
//! A `.wgsl` file with a comment header and an `effect` function:
//!
//! ```wgsl
//! // name: Invert
//! // category: Color
//! // param: amount float 0 1 1          (min max default)
//! // param: tint color 1 0 0 1          (default RGBA)
//! // param: mirror bool true            (default)
//! // param: center point -1 1 0 0       (min max default-x default-y)
//!
//! fn effect(uv: vec2<f32>) -> vec4<f32> {
//!     return sample(uv) * param_amount();
//! }
//! ```
//!
//! Colors are premultiplied. Verso provides `sample(uv)`, `resolution()`,
//! `premultiply(c)`, `unpremultiply(c)`, `verso.time`, `verso.aspect` and one
//! `param_<name>()` function per parameter.
//!
//! # ISF
//!
//! Single-pass ISF filters with one image input are translated to GLSL 4.50 and
//! compiled with naga. Multi-pass and persistent buffers are not supported yet.

use std::collections::HashMap;
use std::path::Path;

use verso_core::{Effect, ParamValue};

/// Scalars available for parameters: 8 `vec4` slots.
pub(crate) const PARAM_SLOTS: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub enum ParamKind {
    Float {
        min: f32,
        max: f32,
        default: f32,
    },
    Color {
        default: [f32; 4],
    },
    Bool {
        default: bool,
    },
    Point {
        min: f32,
        max: f32,
        default: [f32; 2],
    },
}

impl ParamKind {
    pub fn default_value(&self) -> ParamValue {
        match *self {
            ParamKind::Float { default, .. } => ParamValue::Float(default),
            ParamKind::Color { default } => ParamValue::Color(default),
            ParamKind::Bool { default } => ParamValue::Bool(default),
            ParamKind::Point { default, .. } => ParamValue::Point(default),
        }
    }

    /// Scalars used, and whether they must start a `vec4`.
    fn footprint(&self) -> (usize, bool) {
        match self {
            ParamKind::Color { .. } => (4, true),
            ParamKind::Point { .. } => (2, true),
            ParamKind::Float { .. } | ParamKind::Bool { .. } => (1, false),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamDef {
    pub name: String,
    pub kind: ParamKind,
    /// First scalar slot in the parameter array.
    slot: usize,
}

impl ParamDef {
    /// Shader expression reading this parameter.
    fn wgsl_access(&self) -> (String, &'static str) {
        let (v, c) = (self.slot / 4, ["x", "y", "z", "w"][self.slot % 4]);
        match self.kind {
            ParamKind::Float { .. } => (format!("verso.values[{v}].{c}"), "f32"),
            ParamKind::Bool { .. } => (format!("verso.values[{v}].{c} > 0.5"), "bool"),
            ParamKind::Color { .. } => (format!("verso.values[{v}]"), "vec4<f32>"),
            ParamKind::Point { .. } => (format!("verso.values[{v}].xy"), "vec2<f32>"),
        }
    }

    fn glsl_access(&self, kind: &str) -> String {
        let (v, c) = (self.slot / 4, ["x", "y", "z", "w"][self.slot % 4]);
        match (&self.kind, kind) {
            (ParamKind::Float { .. }, "long") => format!("int(verso_values[{v}].{c})"),
            (ParamKind::Float { .. }, _) => format!("verso_values[{v}].{c}"),
            (ParamKind::Bool { .. }, _) => format!("(verso_values[{v}].{c} > 0.5)"),
            (ParamKind::Color { .. }, _) => format!("verso_values[{v}]"),
            (ParamKind::Point { .. }, _) => format!("verso_values[{v}].xy"),
        }
    }
}

/// A compiled-to-WGSL effect, ready for the renderer.
#[derive(Debug, Clone)]
pub struct EffectDef {
    pub id: String,
    pub name: String,
    pub category: String,
    pub params: Vec<ParamDef>,
    /// Fragment module: bindings `0` uniform, `1` input texture, `2` sampler,
    /// input `@location(0)` UV (top-left origin).
    pub(crate) fragment: String,
    pub(crate) entry: &'static str,
}

impl EffectDef {
    pub fn param(&self, name: &str) -> Option<&ParamDef> {
        self.params.iter().find(|p| p.name == name)
    }

    /// Current value of a parameter of `effect`, or its default.
    pub fn value(&self, effect: &Effect, param: &ParamDef) -> ParamValue {
        match (effect.params.get(&param.name), &param.kind) {
            (Some(v @ ParamValue::Float(_)), ParamKind::Float { .. })
            | (Some(v @ ParamValue::Color(_)), ParamKind::Color { .. })
            | (Some(v @ ParamValue::Bool(_)), ParamKind::Bool { .. })
            | (Some(v @ ParamValue::Point(_)), ParamKind::Point { .. }) => *v,
            _ => param.kind.default_value(),
        }
    }

    /// Parameter values packed in shader slot order.
    pub(crate) fn pack(&self, effect: &Effect) -> [f32; PARAM_SLOTS] {
        let mut values = [0.0; PARAM_SLOTS];
        for param in &self.params {
            let s = param.slot;
            match self.value(effect, param) {
                ParamValue::Float(v) => values[s] = v,
                ParamValue::Bool(v) => values[s] = if v { 1.0 } else { 0.0 },
                ParamValue::Color(c) => values[s..s + 4].copy_from_slice(&c),
                ParamValue::Point(p) => values[s..s + 2].copy_from_slice(&p),
            }
        }
        values
    }
}

/// Assigns shader slots: vector parameters first, on `vec4` boundaries, then
/// scalars in the remaining space.
fn assign_slots(params: &mut [ParamDef]) -> Result<(), String> {
    let mut used = [false; PARAM_SLOTS];
    let mut order: Vec<usize> = (0..params.len()).collect();
    order.sort_by_key(|&i| !params[i].kind.footprint().1);
    for i in order {
        let (size, aligned) = params[i].kind.footprint();
        let step = if aligned { 4 } else { 1 };
        let slot = (0..=PARAM_SLOTS - size)
            .step_by(step)
            .find(|&s| used[s..s + size].iter().all(|u| !u))
            .ok_or_else(|| format!("too many parameters (more than {PARAM_SLOTS} values)"))?;
        used[slot..slot + size].iter_mut().for_each(|u| *u = true);
        params[i].slot = slot;
    }
    Ok(())
}

fn validate_wgsl(source: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| e.emit_to_string(source))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .map_err(|e| e.emit_to_string(source))?;
    Ok(())
}

const WGSL_PRELUDE: &str = "
struct VersoEffect {
    time: f32,
    aspect: f32,
    width: f32,
    height: f32,
    values: array<vec4<f32>, 8>,
}

@group(0) @binding(0) var<uniform> verso: VersoEffect;
@group(0) @binding(1) var verso_input: texture_2d<f32>;
@group(0) @binding(2) var verso_sampler: sampler;

fn sample(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(verso_input, verso_sampler, uv, 0.0);
}

fn resolution() -> vec2<f32> {
    return vec2<f32>(verso.width, verso.height);
}

fn unpremultiply(c: vec4<f32>) -> vec4<f32> {
    if c.a <= 0.0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(c.rgb / c.a, c.a);
}

fn premultiply(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}
";

/// Parses a Verso WGSL effect.
pub fn parse_wgsl(id: &str, source: &str) -> Result<EffectDef, String> {
    let mut name = None;
    let mut category = "Other".to_owned();
    let mut params = Vec::new();
    for line in source.lines() {
        let Some(comment) = line.trim().strip_prefix("//") else {
            continue;
        };
        let Some((key, value)) = comment.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "name" => name = Some(value.to_owned()),
            "category" => category = value.to_owned(),
            "param" => params.push(parse_param(value)?),
            _ => {}
        }
    }
    let name = name.ok_or("missing `// name:` header")?;
    assign_slots(&mut params)?;

    let mut fragment = String::from(WGSL_PRELUDE);
    for param in &params {
        let (access, ty) = param.wgsl_access();
        fragment.push_str(&format!(
            "\nfn param_{}() -> {ty} {{\n    return {access};\n}}\n",
            param.name
        ));
    }
    fragment.push('\n');
    fragment.push_str(source);
    fragment.push_str(
        "\n@fragment\nfn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {\n    return effect(uv);\n}\n",
    );
    validate_wgsl(&fragment)?;
    Ok(EffectDef {
        id: id.to_owned(),
        name,
        category,
        params,
        fragment,
        entry: "fs_main",
    })
}

fn parse_param(spec: &str) -> Result<ParamDef, String> {
    let words: Vec<&str> = spec.split_whitespace().collect();
    let bad = || format!("bad parameter `{spec}`");
    let num = |i: usize| -> Result<f32, String> {
        words.get(i).and_then(|w| w.parse().ok()).ok_or_else(bad)
    };
    let name = *words.first().ok_or_else(bad)?;
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("bad parameter name `{name}`"));
    }
    let kind = match *words.get(1).ok_or_else(bad)? {
        "float" => ParamKind::Float {
            min: num(2)?,
            max: num(3)?,
            default: num(4)?,
        },
        "color" => ParamKind::Color {
            default: [num(2)?, num(3)?, num(4)?, num(5)?],
        },
        "bool" => ParamKind::Bool {
            default: words.get(2) == Some(&"true"),
        },
        "point" => ParamKind::Point {
            min: num(2)?,
            max: num(3)?,
            default: [num(4)?, num(5)?],
        },
        other => return Err(format!("unknown parameter type `{other}`")),
    };
    Ok(ParamDef {
        name: name.to_owned(),
        kind,
        slot: 0,
    })
}

const ISF_PRELUDE: &str = "#version 450
layout(set = 0, binding = 0, std140) uniform VersoEffect {
    float verso_time;
    float verso_aspect;
    vec2 RENDERSIZE;
    vec4 verso_values[8];
};
layout(set = 0, binding = 1) uniform texture2D verso_input;
layout(set = 0, binding = 2) uniform sampler verso_sampler;
layout(location = 0) in vec2 verso_uv;
layout(location = 0) out vec4 verso_frag_color;

// ISF works with straight alpha and a bottom-left origin.
vec4 verso_sample(vec2 c) {
    vec4 s = textureLod(sampler2D(verso_input, verso_sampler), vec2(c.x, 1.0 - c.y), 0.0);
    return s.a > 0.0 ? vec4(s.rgb / s.a, s.a) : vec4(0.0);
}

#define TIME verso_time
#define TIMEDELTA (1.0 / 60.0)
#define FRAMEINDEX int(verso_time * 60.0)
#define PASSINDEX 0
#define isf_FragNormCoord vec2(verso_uv.x, 1.0 - verso_uv.y)
#define gl_FragColor verso_frag_color
#define IMG_NORM_PIXEL(image, coord) verso_sample(coord)
#define IMG_PIXEL(image, coord) verso_sample((coord) / RENDERSIZE)
#define IMG_THIS_PIXEL(image) verso_sample(isf_FragNormCoord)
#define IMG_THIS_NORM_PIXEL(image) verso_sample(isf_FragNormCoord)
#define IMG_SIZE(image) RENDERSIZE
";

fn json_num(value: Option<&serde_json::Value>, fallback: f32) -> f32 {
    value
        .and_then(|v| v.as_f64())
        .map_or(fallback, |v| v as f32)
}

/// Translates a single-pass ISF filter.
pub fn parse_isf(id: &str, fallback_name: &str, source: &str) -> Result<EffectDef, String> {
    let start = source.find("/*").ok_or("missing ISF JSON header")?;
    let end = source[start..]
        .find("*/")
        .ok_or("unterminated ISF header")?
        + start;
    let header: serde_json::Value =
        serde_json::from_str(&source[start + 2..end]).map_err(|e| format!("bad ISF JSON: {e}"))?;
    let body = &source[end + 2..];

    if header["PASSES"].as_array().is_some_and(|p| p.len() > 1) {
        return Err("multi-pass ISF is not supported yet".into());
    }
    let mut params = Vec::new();
    // ISF type name per parameter, for GLSL access.
    let mut isf_types = Vec::new();
    let mut images = 0;
    for input in header["INPUTS"].as_array().into_iter().flatten() {
        let name = input["NAME"].as_str().ok_or("ISF input without NAME")?;
        let ty = input["TYPE"].as_str().unwrap_or("");
        let kind = match ty {
            "image" => {
                images += 1;
                continue;
            }
            "float" | "long" => ParamKind::Float {
                min: json_num(input.get("MIN"), 0.0),
                max: json_num(input.get("MAX"), 1.0),
                default: json_num(input.get("DEFAULT"), 0.0),
            },
            "bool" | "event" => ParamKind::Bool {
                default: input["DEFAULT"]
                    .as_bool()
                    .or_else(|| input["DEFAULT"].as_f64().map(|v| v > 0.5))
                    .unwrap_or(false),
            },
            "color" => {
                let d = &input["DEFAULT"];
                ParamKind::Color {
                    default: std::array::from_fn(|i| json_num(d.get(i), 1.0)),
                }
            }
            "point2D" => {
                let d = &input["DEFAULT"];
                ParamKind::Point {
                    min: json_num(input["MIN"].get(0), 0.0),
                    max: json_num(input["MAX"].get(0), 1.0),
                    default: [json_num(d.get(0), 0.5), json_num(d.get(1), 0.5)],
                }
            }
            other => return Err(format!("ISF input type `{other}` is not supported")),
        };
        params.push(ParamDef {
            name: name.to_owned(),
            kind,
            slot: 0,
        });
        isf_types.push(ty);
    }
    if images > 1 {
        return Err("ISF filters with several image inputs are not supported".into());
    }
    assign_slots(&mut params)?;

    let mut glsl = String::from(ISF_PRELUDE);
    for (param, ty) in params.iter().zip(&isf_types) {
        glsl.push_str(&format!(
            "#define {} {}\n",
            param.name,
            param.glsl_access(ty)
        ));
    }
    glsl.push_str("#define main verso_isf_main\n");
    for line in body.lines() {
        // The prelude sets the version; ISF bodies often declare an older one.
        if !line.trim_start().starts_with("#version") {
            glsl.push_str(line);
            glsl.push('\n');
        }
    }
    glsl.push_str(
        "#undef main\nvoid main() {\n    verso_isf_main();\n    verso_frag_color = vec4(verso_frag_color.rgb * verso_frag_color.a, verso_frag_color.a);\n}\n",
    );

    let module = naga::front::glsl::Frontend::default()
        .parse(&naga::ShaderStage::Fragment.into(), &glsl)
        .map_err(|e| e.emit_to_string(&glsl))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .map_err(|e| e.emit_to_string(&glsl))?;
    let fragment =
        naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
            .map_err(|e| e.to_string())?;
    validate_wgsl(&fragment)?;

    let name = header["NAME"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| fallback_name.to_owned());
    let category = header["CATEGORIES"][0].as_str().unwrap_or("ISF").to_owned();
    Ok(EffectDef {
        id: id.to_owned(),
        name,
        category,
        params,
        fragment,
        entry: "main",
    })
}

const BUILTINS: &[(&str, &str)] = &[
    ("invert", include_str!("effects/invert.wgsl")),
    (
        "hue_saturation",
        include_str!("effects/hue_saturation.wgsl"),
    ),
    (
        "brightness_contrast",
        include_str!("effects/brightness_contrast.wgsl"),
    ),
    ("tint", include_str!("effects/tint.wgsl")),
    ("posterize", include_str!("effects/posterize.wgsl")),
    ("blur", include_str!("effects/blur.wgsl")),
    ("pixelate", include_str!("effects/pixelate.wgsl")),
    ("mirror", include_str!("effects/mirror.wgsl")),
    ("kaleidoscope", include_str!("effects/kaleidoscope.wgsl")),
    ("rgb_split", include_str!("effects/rgb_split.wgsl")),
    ("transform", include_str!("effects/transform.wgsl")),
];

/// All known effect definitions.
#[derive(Debug, Clone, Default)]
pub struct EffectLibrary {
    defs: Vec<EffectDef>,
    index: HashMap<String, usize>,
}

impl EffectLibrary {
    /// The bundled effects.
    pub fn builtin() -> Self {
        let mut library = Self::default();
        for (name, source) in BUILTINS {
            let def = parse_wgsl(&format!("builtin:{name}"), source)
                .unwrap_or_else(|e| panic!("bundled effect {name} is invalid: {e}"));
            library.insert(def);
        }
        library
    }

    fn insert(&mut self, def: EffectDef) {
        match self.index.get(&def.id) {
            Some(&i) => self.defs[i] = def,
            None => {
                self.index.insert(def.id.clone(), self.defs.len());
                self.defs.push(def);
            }
        }
    }

    /// Loads one `.wgsl` (Verso format) or `.fs` (ISF) file. Its id is its path.
    pub fn load_file(&mut self, path: &Path) -> Result<&EffectDef, String> {
        let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let id = path.display().to_string();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let def = match path.extension().and_then(|e| e.to_str()) {
            Some("wgsl") => parse_wgsl(&id, &source),
            Some("fs") => parse_isf(&id, &stem, &source),
            _ => Err("not an effect file (.wgsl or .fs)".into()),
        }
        .map_err(|e| format!("{}: {e}", path.display()))?;
        self.insert(def);
        Ok(&self.defs[self.index[&id]])
    }

    /// Loads every effect file of `dir`. Returns one message per file that
    /// failed.
    pub fn load_dir(&mut self, dir: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("wgsl" | "fs")))
            .collect();
        paths.sort();
        paths
            .iter()
            .filter_map(|p| self.load_file(p).err())
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&EffectDef> {
        self.index.get(id).map(|&i| &self.defs[i])
    }

    /// Definitions sorted by category, then name.
    pub fn sorted(&self) -> Vec<&EffectDef> {
        let mut all: Vec<_> = self.defs.iter().collect();
        all.sort_by(|a, b| (&a.category, &a.name).cmp(&(&b.category, &b.name)));
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_compile() {
        let library = EffectLibrary::builtin();
        assert_eq!(library.sorted().len(), BUILTINS.len());
        assert!(library.get("builtin:blur").is_some());
    }

    #[test]
    fn slots_align_vectors() {
        let def = parse_wgsl(
            "t",
            "// name: T\n// param: a float 0 1 0.5\n// param: c color 1 0 0 1\n// param: b bool true\n// param: p point -1 1 0.25 0.75\nfn effect(uv: vec2<f32>) -> vec4<f32> { return sample(uv); }",
        )
        .unwrap();
        let slot = |n: &str| def.param(n).unwrap().slot;
        assert_eq!(slot("c") % 4, 0);
        assert_eq!(slot("p") % 4, 0);
        let mut effect = Effect::new("t", "T");
        effect.params.insert("a".into(), ParamValue::Float(0.9));
        let packed = def.pack(&effect);
        assert_eq!(packed[slot("a")], 0.9);
        assert_eq!(packed[slot("b")], 1.0);
        assert_eq!(&packed[slot("c")..slot("c") + 4], &[1.0, 0.0, 0.0, 1.0]);
        assert_eq!(&packed[slot("p")..slot("p") + 2], &[0.25, 0.75]);
    }

    #[test]
    fn mismatched_value_type_uses_default() {
        let def = parse_wgsl(
            "t",
            "// name: T\n// param: a float 0 1 0.5\nfn effect(uv: vec2<f32>) -> vec4<f32> { return sample(uv); }",
        )
        .unwrap();
        let mut effect = Effect::new("t", "T");
        effect.params.insert("a".into(), ParamValue::Bool(true));
        assert_eq!(def.value(&effect, &def.params[0]), ParamValue::Float(0.5));
    }

    #[test]
    fn wgsl_errors_are_reported() {
        let err = parse_wgsl(
            "t",
            "// name: T\nfn effect(uv: vec2<f32>) -> vec4<f32> { return nope; }",
        )
        .unwrap_err();
        assert!(err.contains("nope"), "{err}");
        assert!(
            parse_wgsl("t", "fn effect() {}")
                .unwrap_err()
                .contains("name")
        );
        assert!(parse_wgsl("t", "// name: T\n// param: x vec9 1").is_err());
    }

    #[test]
    fn too_many_params_fail() {
        let mut src = String::from("// name: T\n");
        for i in 0..9 {
            src.push_str(&format!("// param: c{i} color 1 1 1 1\n"));
        }
        src.push_str("fn effect(uv: vec2<f32>) -> vec4<f32> { return sample(uv); }");
        assert!(parse_wgsl("t", &src).unwrap_err().contains("too many"));
    }

    #[test]
    fn repository_examples_load() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../effects");
        let mut library = EffectLibrary::default();
        assert_eq!(library.load_dir(&dir), Vec::<String>::new());
        assert!(library.sorted().len() >= 2);
    }

    const ISF_SAMPLE: &str = r#"/*{
    "DESCRIPTION": "Test filter",
    "CATEGORIES": ["Color Effect"],
    "INPUTS": [
        {"NAME": "inputImage", "TYPE": "image"},
        {"NAME": "amount", "TYPE": "float", "MIN": 0.0, "MAX": 1.0, "DEFAULT": 0.5},
        {"NAME": "tint", "TYPE": "color", "DEFAULT": [1.0, 0.5, 0.0, 1.0]},
        {"NAME": "flip", "TYPE": "bool", "DEFAULT": true},
        {"NAME": "center", "TYPE": "point2D", "DEFAULT": [0.5, 0.5]}
    ]
}*/
#version 120
void main() {
    vec2 uv = isf_FragNormCoord;
    if (flip) {
        uv.x = 1.0 - uv.x;
    }
    vec4 c = IMG_NORM_PIXEL(inputImage, uv + (center - 0.5));
    gl_FragColor = mix(c, c * tint, amount) + vec4(0.0 * TIME);
}
"#;

    #[test]
    fn isf_filter_translates() {
        let def = parse_isf("isf", "Sample", ISF_SAMPLE).unwrap();
        assert_eq!(def.name, "Sample");
        assert_eq!(def.category, "Color Effect");
        let names: Vec<_> = def.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["amount", "tint", "flip", "center"]);
        assert!(def.fragment.contains("fn main"));
    }

    #[test]
    fn isf_unsupported_features_are_reported() {
        let multi = ISF_SAMPLE.replace(r#""INPUTS""#, r#""PASSES": [{}, {}], "INPUTS""#);
        assert!(
            parse_isf("isf", "x", &multi)
                .unwrap_err()
                .contains("multi-pass")
        );
        let audio = ISF_SAMPLE.replace(r#""TYPE": "bool""#, r#""TYPE": "audioFFT""#);
        assert!(
            parse_isf("isf", "x", &audio)
                .unwrap_err()
                .contains("audioFFT")
        );
        assert!(
            parse_isf("isf", "x", "void main() {}")
                .unwrap_err()
                .contains("header")
        );
    }
}
