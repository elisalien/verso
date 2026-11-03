//! Wraps the GLSL warp and comp shaders of Butterchurn presets into complete
//! GLSL 4.50 fragment shaders, translated to WGSL with naga.

/// Which of the two preset shaders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    Warp,
    Comp,
}

/// Uniform block shared by the warp and comp passes (std140, 24 vec4).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MilkUniforms {
    pub q: [[f32; 4]; 8],
    pub texsize: [f32; 4],
    pub aspect: [f32; 4],
    pub rand_frame: [f32; 4],
    pub rand_preset: [f32; 4],
    pub slow_roam_cos: [f32; 4],
    pub roam_cos: [f32; 4],
    pub slow_roam_sin: [f32; 4],
    pub roam_sin: [f32; 4],
    /// bass, mid, treb, vol
    pub audio: [f32; 4],
    /// bass_att, mid_att, treb_att, vol_att
    pub audio_att: [f32; 4],
    /// time, frame, fps, decay
    pub timing: [f32; 4],
    /// blur1..3 min, fShader
    pub blur_min: [f32; 4],
    /// blur1..3 max, gammaAdj
    pub blur_max: [f32; 4],
    /// echo_zoom, echo_alpha, echo_orientation, unused
    pub echo: [f32; 4],
    /// invert, brighten, darken, solarize
    pub flags: [f32; 4],
    /// Size of the input image and its inverse.
    pub input_size: [f32; 4],
}

const PRELUDE: &str = r#"#version 450
layout(set = 0, binding = 0, std140) uniform VersoMilk {
    vec4 _qa; vec4 _qb; vec4 _qc; vec4 _qd; vec4 _qe; vec4 _qf; vec4 _qg; vec4 _qh;
    vec4 texsize;
    vec4 aspect;
    vec4 rand_frame;
    vec4 rand_preset;
    vec4 slow_roam_cos;
    vec4 roam_cos;
    vec4 slow_roam_sin;
    vec4 roam_sin;
    vec4 verso_audio;
    vec4 verso_audio_att;
    vec4 verso_timing;
    vec4 verso_blur_min;
    vec4 verso_blur_max;
    vec4 verso_echo;
    vec4 verso_flags;
    vec4 verso_input_size;
};
layout(set = 0, binding = 1) uniform texture2D verso_t_main;
layout(set = 0, binding = 2) uniform texture2D verso_t_blur1;
layout(set = 0, binding = 3) uniform texture2D verso_t_blur2;
layout(set = 0, binding = 4) uniform texture2D verso_t_blur3;
layout(set = 0, binding = 5) uniform texture2D verso_t_noise_lq;
layout(set = 0, binding = 6) uniform texture2D verso_t_noise_lq_lite;
layout(set = 0, binding = 7) uniform texture2D verso_t_noise_mq;
layout(set = 0, binding = 8) uniform texture2D verso_t_noise_hq;
layout(set = 0, binding = 9) uniform texture3D verso_t_noisevol_lq;
layout(set = 0, binding = 10) uniform texture3D verso_t_noisevol_hq;
layout(set = 0, binding = 11) uniform sampler verso_s_main;
layout(set = 0, binding = 12) uniform sampler verso_s_fw;
layout(set = 0, binding = 13) uniform sampler verso_s_fc;
layout(set = 0, binding = 14) uniform sampler verso_s_pw;
layout(set = 0, binding = 15) uniform sampler verso_s_pc;
// Image textures of a preset read the clip's input layer (noise without one).
layout(set = 0, binding = 16) uniform texture2D verso_t_input;

#define sampler_main sampler2D(verso_t_main, verso_s_main)
#define sampler_fw_main sampler2D(verso_t_main, verso_s_fw)
#define sampler_fc_main sampler2D(verso_t_main, verso_s_fc)
#define sampler_pw_main sampler2D(verso_t_main, verso_s_pw)
#define sampler_pc_main sampler2D(verso_t_main, verso_s_pc)
#define sampler_blur1 sampler2D(verso_t_blur1, verso_s_fc)
#define sampler_blur2 sampler2D(verso_t_blur2, verso_s_fc)
#define sampler_blur3 sampler2D(verso_t_blur3, verso_s_fc)
#define sampler_noise_lq sampler2D(verso_t_noise_lq, verso_s_fw)
#define sampler_noise_lq_lite sampler2D(verso_t_noise_lq_lite, verso_s_fw)
#define sampler_noise_mq sampler2D(verso_t_noise_mq, verso_s_fw)
#define sampler_noise_hq sampler2D(verso_t_noise_hq, verso_s_fw)
#define sampler_pw_noise_lq sampler2D(verso_t_noise_lq, verso_s_pw)
#define sampler_noisevol_lq sampler3D(verso_t_noisevol_lq, verso_s_fw)
#define sampler_noisevol_hq sampler3D(verso_t_noisevol_hq, verso_s_fw)

// No mipmaps, and explicit level of detail keeps sampling legal in
// non-uniform control flow.
#define texture(s, c) textureLod(s, c, 0.0)
#define texture2D(s, c) textureLod(s, c, 0.0)
#define texture3D(s, c) textureLod(s, c, 0.0)

#define texsize_noise_lq vec4(256.0, 256.0, 1.0 / 256.0, 1.0 / 256.0)
#define texsize_noise_mq vec4(256.0, 256.0, 1.0 / 256.0, 1.0 / 256.0)
#define texsize_noise_hq vec4(256.0, 256.0, 1.0 / 256.0, 1.0 / 256.0)
#define texsize_noise_lq_lite vec4(32.0, 32.0, 1.0 / 32.0, 1.0 / 32.0)
#define texsize_noisevol_lq vec4(32.0, 32.0, 1.0 / 32.0, 1.0 / 32.0)
#define texsize_noisevol_hq vec4(32.0, 32.0, 1.0 / 32.0, 1.0 / 32.0)

const float PI = 3.141592653589793;

// Uniform values copied into globals, so presets may shadow their names.
vec2 resolution;
float time; float frame; float fps; float decay;
float bass; float mid; float treb; float vol;
float bass_att; float mid_att; float treb_att; float vol_att;
float blur1_min; float blur2_min; float blur3_min;
float blur1_max; float blur2_max; float blur3_max;
float scale1; float scale2; float scale3;
float bias1; float bias2; float bias3;
float fShader; float gammaAdj;
float echo_zoom; float echo_alpha; float echo_orientation;
int invert; int brighten; int darken; int solarize;
float q1; float q2; float q3; float q4; float q5; float q6; float q7; float q8;
float q9; float q10; float q11; float q12; float q13; float q14; float q15; float q16;
float q17; float q18; float q19; float q20; float q21; float q22; float q23; float q24;
float q25; float q26; float q27; float q28; float q29; float q30; float q31; float q32;
vec2 uv; vec2 uv_orig; float rad; float ang;
vec3 hue_shader;

vec3 lum(vec3 v) {
    return vec3(dot(v, vec3(0.32, 0.49, 0.29)));
}

void verso_globals() {
    resolution = texsize.xy;
    time = verso_timing.x; frame = verso_timing.y; fps = verso_timing.z; decay = verso_timing.w;
    bass = verso_audio.x; mid = verso_audio.y; treb = verso_audio.z; vol = verso_audio.w;
    bass_att = verso_audio_att.x; mid_att = verso_audio_att.y;
    treb_att = verso_audio_att.z; vol_att = verso_audio_att.w;
    blur1_min = verso_blur_min.x; blur2_min = verso_blur_min.y; blur3_min = verso_blur_min.z;
    blur1_max = verso_blur_max.x; blur2_max = verso_blur_max.y; blur3_max = verso_blur_max.z;
    scale1 = blur1_max - blur1_min; scale2 = blur2_max - blur2_min; scale3 = blur3_max - blur3_min;
    bias1 = blur1_min; bias2 = blur2_min; bias3 = blur3_min;
    fShader = verso_blur_min.w; gammaAdj = verso_blur_max.w;
    echo_zoom = verso_echo.x; echo_alpha = verso_echo.y; echo_orientation = verso_echo.z;
    invert = int(verso_flags.x); brighten = int(verso_flags.y);
    darken = int(verso_flags.z); solarize = int(verso_flags.w);
    q1 = _qa.x; q2 = _qa.y; q3 = _qa.z; q4 = _qa.w;
    q5 = _qb.x; q6 = _qb.y; q7 = _qb.z; q8 = _qb.w;
    q9 = _qc.x; q10 = _qc.y; q11 = _qc.z; q12 = _qc.w;
    q13 = _qd.x; q14 = _qd.y; q15 = _qd.z; q16 = _qd.w;
    q17 = _qe.x; q18 = _qe.y; q19 = _qe.z; q20 = _qe.w;
    q21 = _qf.x; q22 = _qf.y; q23 = _qf.z; q24 = _qf.w;
    q25 = _qg.x; q26 = _qg.y; q27 = _qg.z; q28 = _qg.w;
    q29 = _qh.x; q30 = _qh.y; q31 = _qh.z; q32 = _qh.w;
}
"#;

const DEFAULT_WARP: &str = "ret = texture(sampler_main, uv).rgb * decay;";

const DEFAULT_COMP: &str = r#"
float orient_horiz = mod(echo_orientation, 2.0);
float orient_x = (orient_horiz != 0.0) ? -1.0 : 1.0;
float orient_y = (echo_orientation >= 2.0) ? -1.0 : 1.0;
vec2 uv_echo = ((uv - 0.5) * (1.0 / echo_zoom) * vec2(orient_x, orient_y)) + 0.5;
ret = mix(texture(sampler_main, uv).rgb, texture(sampler_main, uv_echo).rgb, echo_alpha);
ret *= gammaAdj;
if (fShader >= 1.0) {
    ret *= hue_shader;
} else if (fShader > 0.001) {
    ret *= (1.0 - fShader) + (fShader * hue_shader);
}
if (brighten != 0) ret = sqrt(ret);
if (darken != 0) ret = ret * ret;
if (solarize != 0) ret = ret * (1.0 - ret) * 4.0;
if (invert != 0) ret = 1.0 - ret;
"#;

/// Splits Butterchurn shader text into (declarations, `shader_body` code).
fn shader_parts(text: &str) -> (&str, &str) {
    match text.find("shader_body") {
        Some(i) => {
            let after = &text[i..];
            match (after.find('{'), after.rfind('}')) {
                (Some(open), Some(close)) if close > open => (&text[..i], &after[open + 1..close]),
                _ => (&text[..i], ""),
            }
        }
        None => ("", text),
    }
}

/// Declarations with uniforms the prelude cannot provide rewritten:
/// image samplers read the clip's input, other uniforms become globals.
fn rewrite_header(header: &str) -> String {
    let mut out = String::new();
    let mut user_samplers = Vec::new();
    let mut texsizes = Vec::new();
    for line in header.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("uniform ") {
            let decl = rest.trim_end_matches(';').trim();
            let mut words = decl.split_whitespace();
            let (ty, name) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
            if ty == "sampler2D" {
                let short = name.trim_start_matches("sampler_");
                if !super::BUILTIN_SAMPLERS.contains(&short) {
                    // Image textures are not bundled: they read the input.
                    out.push_str(&format!(
                        "#define {name} sampler2D(verso_t_input, verso_s_fw)\n"
                    ));
                    user_samplers.push(short.to_owned());
                }
            } else if ty == "sampler3D" {
                // Built-in volume samplers come from the prelude.
            } else if name.starts_with("texsize_") {
                texsizes.push(name.to_owned());
                out.push_str(&format!("#define {name} verso_input_size\n"));
            } else if !ty.is_empty() && !name.is_empty() {
                out.push_str(&format!("{ty} {name};\n"));
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    // Presets often use the size of an image texture without declaring it.
    for name in user_samplers {
        let size = format!("texsize_{name}");
        if !texsizes.contains(&size) {
            out.push_str(&format!("#define {size} verso_input_size\n"));
        }
    }
    out
}

/// Rewrites single-component swizzles of indexed columns (`m[0].y`) as
/// indices (`m[0][1]`), which naga can assign to.
fn matrix_components(code: &str) -> String {
    let bytes = code.as_bytes();
    let mut out = String::with_capacity(code.len());
    let mut i = 0;
    while i < bytes.len() {
        // `[d].c` followed by a non-identifier character.
        if bytes[i] == b'['
            && i + 4 < bytes.len()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2] == b']'
            && bytes[i + 3] == b'.'
            && b"xyzw".contains(&bytes[i + 4])
            && !bytes
                .get(i + 5)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            let component = b"xyzw".iter().position(|c| *c == bytes[i + 4]).unwrap();
            out.push_str(&format!("[{}][{component}]", bytes[i + 1] as char));
            i += 5;
            continue;
        }
        let ch = code[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Complete GLSL fragment shader for `stage` from the preset text (empty for
/// MilkDrop's default shader).
pub fn build(stage: Stage, text: &str) -> String {
    let text = text.trim();
    let (header, body) = if text.is_empty() {
        (
            String::new(),
            match stage {
                Stage::Warp => DEFAULT_WARP,
                Stage::Comp => DEFAULT_COMP,
            }
            .to_owned(),
        )
    } else {
        let (header, body) = shader_parts(text);
        (rewrite_header(header), body.to_owned())
    };
    // naga cannot index a matrix column with a `uint` and then pick a
    // component; glsl-optimizer writes `m[uint(0)].x` and `m[1u].y`.
    let mut body = body;
    for i in 0..4 {
        body = body
            .replace(&format!("[uint({i})]"), &format!("[{i}]"))
            .replace(&format!("[{i}u]"), &format!("[{i}]"));
    }
    let body = matrix_components(&body);
    let io = match stage {
        Stage::Warp => {
            "layout(location = 0) in vec2 verso_uv;\nlayout(location = 1) in vec2 verso_uv_orig;\nlayout(location = 2) in vec4 verso_color;\nlayout(location = 0) out vec4 verso_frag;\n"
        }
        Stage::Comp => {
            "layout(location = 0) in vec2 verso_uv;\nlayout(location = 1) in vec4 verso_color;\nlayout(location = 0) out vec4 verso_frag;\n"
        }
    };
    let setup = match stage {
        Stage::Warp => {
            "    uv = verso_uv;\n    uv_orig = verso_uv_orig;\n    rad = length(uv_orig - 0.5);\n    ang = atan(uv_orig.x - 0.5, uv_orig.y - 0.5);\n"
        }
        Stage::Comp => {
            "    uv = verso_uv;\n    uv.y = 1.0 - uv.y;\n    uv_orig = uv;\n    rad = length(uv - 0.5);\n    ang = atan(uv.x - 0.5, uv.y - 0.5);\n    hue_shader = verso_color.rgb;\n"
        }
    };
    let finish = match stage {
        Stage::Warp => "    verso_frag = vec4(ret, 1.0) * verso_color;\n",
        // Display-referred MilkDrop colors to Verso's linear working space.
        Stage::Comp => {
            "    vec3 c = clamp(ret, 0.0, 1.0);\n    vec3 lo = c / 12.92;\n    vec3 hi = pow((c + 0.055) / 1.055, vec3(2.4));\n    verso_frag = vec4(mix(hi, lo, vec3(lessThanEqual(c, vec3(0.04045)))), verso_color.a);\n"
        }
    };
    format!(
        "{PRELUDE}{io}\n{header}\nvoid main() {{\n    verso_globals();\n{setup}    vec3 ret = vec3(0.0);\n    {{\n{body}\n    }}\n{finish}}}\n"
    )
}

/// Translates a preset shader to WGSL (entry point `main`).
pub fn translate(stage: Stage, text: &str) -> Result<String, String> {
    let glsl = build(stage, text);
    let module = naga::front::glsl::Frontend::default()
        .parse(&naga::ShaderStage::Fragment.into(), &glsl)
        .map_err(|e| e.emit_to_string(&glsl))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .map_err(|e| e.emit_to_string(&glsl))?;
    naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shaders_translate() {
        translate(Stage::Warp, "").unwrap();
        translate(Stage::Comp, "").unwrap();
    }

    #[test]
    fn uniform_block_matches_struct() {
        assert_eq!(size_of::<MilkUniforms>(), 24 * 16);
    }

    #[test]
    fn preset_shader_translates() {
        let warp = "uniform sampler2D sampler_clouds2;\nuniform vec4 texsize_clouds2;\n shader_body { \n  vec3 ret_1;\n  ret_1 = (texture (sampler_main, uv).xyz + ((texture (sampler_blur2, uv).xyz * scale2) + bias2));\n  ret_1 += texture(sampler_clouds2, uv * texsize_clouds2.zw).xyz * q1;\n  ret = ret_1 * 0.9;\n }";
        translate(Stage::Warp, warp).unwrap();
    }
}

#[cfg(test)]
mod bundled {
    use super::*;

    /// Every preset shipped in `presets/milkdrop` loads, compiles its
    /// equations and translates its shaders.
    #[test]
    fn bundled_presets_compile() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../presets/milkdrop");
        let mut library = verso_milkdrop::MilkLibrary::default();
        let errors = library.load_dir(&dir);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(library.len() > 100, "only {} families", library.len());
        for info in library.sorted() {
            let family = library.get(&info.id).unwrap();
            for variant in &family.variants {
                let preset = &variant.preset;
                verso_milkdrop::Engine::new(preset, 320, 180, 1)
                    .unwrap_or_else(|e| panic!("{}: {e}", variant.name));
                for (stage, text) in [(Stage::Warp, &preset.warp), (Stage::Comp, &preset.comp)] {
                    translate(stage, text)
                        .unwrap_or_else(|e| panic!("{} {stage:?}: {e}", variant.name));
                }
            }
        }
    }
}
