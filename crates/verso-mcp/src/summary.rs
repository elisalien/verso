//! JSON views of the composition for tools, and sources from tool arguments.
//! Indices are 1-based, as in OSC addresses.

use std::path::PathBuf;

use serde_json::{Value, json};
use verso_core::control::{Action, Param};
use verso_core::{Composition, Effect, EffectTarget, Generator, Layer, Source};

fn one(index: usize) -> usize {
    index + 1
}

/// Short description of a source.
pub fn source(source: &Source) -> Value {
    match source {
        Source::Solid { color } => json!({"kind": "solid", "color": color}),
        Source::Generator { kind, color, speed } => {
            json!({"kind": "generator", "name": kind.name(), "color": color, "speed": speed})
        }
        Source::Image { path } => json!({"kind": "image", "path": path}),
        Source::Shader(effect) => json!({"kind": "shader", "id": effect.id, "name": effect.name}),
        Source::Milkdrop {
            family,
            variant,
            blend,
            pattern,
            input,
            input_mix,
        } => json!({
            "kind": "milkdrop",
            "family": family,
            "variant": one(*variant),
            "blend": blend,
            "pattern": pattern.name(),
            "input_layer": input.map(one),
            "input_mix": input_mix,
        }),
        Source::Video {
            path,
            speed,
            looping,
        } => json!({"kind": "video", "path": path, "speed": speed, "looping": looping}),
    }
}

/// Effects of a chain, without parameter definitions.
pub fn effects(chain: &[Effect]) -> Value {
    chain
        .iter()
        .enumerate()
        .map(|(i, e)| {
            json!({
                "index": one(i),
                "id": e.id,
                "name": e.name,
                "bypass": e.bypass,
                "changed_params": e.params,
            })
        })
        .collect()
}

fn layer(index: usize, layer: &Layer) -> Value {
    let clips: Vec<Value> = layer
        .clips
        .iter()
        .enumerate()
        .filter_map(|(c, clip)| {
            let clip = clip.as_ref()?;
            Some(json!({
                "column": one(c),
                "name": clip.name,
                "source": source(&clip.source),
                "effects": clip.effects.len(),
                "playing": layer.active == Some(c),
            }))
        })
        .collect();
    json!({
        "layer": one(index),
        "name": layer.name,
        "opacity": layer.opacity,
        "blend": layer.blend.name(),
        "bypass": layer.bypass,
        "solo": layer.solo,
        "fader": format!("{:?}", layer.fader),
        "transition": layer.transition,
        "group": layer.group.map(one),
        "playing_column": layer.active.map(one),
        "effects": effects(&layer.effects),
        "clips": clips,
    })
}

/// The composition at a glance: what an agent needs to address things.
pub fn composition(comp: &Composition) -> Value {
    json!({
        "name": comp.name,
        "size": [comp.width, comp.height],
        "bpm": comp.bpm,
        "master": {"opacity": comp.master.opacity, "crossfader": comp.master.crossfader},
        "columns": comp.columns.iter().enumerate()
            .map(|(i, c)| json!({"column": one(i), "name": c.name}))
            .collect::<Vec<_>>(),
        "layers": comp.layers.iter().enumerate()
            .map(|(i, l)| layer(i, l))
            .collect::<Vec<_>>(),
        "groups": comp.groups.iter().enumerate()
            .map(|(i, g)| json!({
                "group": one(i),
                "name": g.name,
                "opacity": g.opacity,
                "blend": g.blend.name(),
                "bypass": g.bypass,
                "solo": g.solo,
                "parent": g.parent.map(one),
                "effects": effects(&g.effects),
            }))
            .collect::<Vec<_>>(),
        "effects": effects(&comp.effects),
        "modulators": comp.modulators.len(),
        "midi_mappings": comp.midi.len(),
        "scripts": comp.scripts.iter().map(|s| json!({"name": s.name, "enabled": s.enabled})).collect::<Vec<_>>(),
        "cues": {
            "current": comp.cues.current.map(one),
            "fading": comp.cue_run.as_ref().map(|r| r.progress() < 1.0).unwrap_or(false),
            "list": comp.cues.cues.iter().enumerate()
                .map(|(i, c)| json!({
                    "cue": one(i),
                    "name": c.name,
                    "fade": c.fade,
                    "follow": format!("{:?}", c.follow),
                }))
                .collect::<Vec<_>>(),
        },
    })
}

/// OSC address of an effect parameter, so an agent can drive it with `osc`.
pub fn effect_address(target: EffectTarget, index: usize, name: &str) -> String {
    Action::Set(Param::Effect {
        target,
        index,
        name: name.to_owned(),
    })
    .address()
}

/// A source asked for by a tool, before the app resolves what only it knows.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceRequest {
    Ready(Source),
    /// Image or video file; the app checks the kind.
    File(PathBuf),
    /// Generator shader id from the effect library.
    Shader(String),
}

fn color(value: &Value) -> Result<Option<[f32; 4]>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let parts: Vec<f32> = value
        .as_array()
        .ok_or("color must be [r, g, b] or [r, g, b, a] in 0..1")?
        .iter()
        .map(|v| v.as_f64().map(|v| v as f32))
        .collect::<Option<_>>()
        .ok_or("color components must be numbers")?;
    match parts.as_slice() {
        [r, g, b] => Ok(Some([*r, *g, *b, 1.0])),
        [r, g, b, a] => Ok(Some([*r, *g, *b, *a])),
        _ => Err("color must have 3 or 4 components".into()),
    }
}

/// Reads the `source` argument of `set_clip`.
pub fn parse_source(value: &Value) -> Result<SourceRequest, String> {
    let white = [1.0, 1.0, 1.0, 1.0];
    if let Some(name) = value.get("generator") {
        let name = name.as_str().ok_or("generator must be a name")?;
        let kind = Generator::ALL
            .into_iter()
            .find(|g| g.name().eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                let names: Vec<_> = Generator::ALL.iter().map(|g| g.name()).collect();
                format!("unknown generator {name}; one of {}", names.join(", "))
            })?;
        return Ok(SourceRequest::Ready(Source::Generator {
            kind,
            color: color(&value["color"])?.unwrap_or(white),
            speed: value["speed"].as_f64().unwrap_or(1.0) as f32,
        }));
    }
    if let Some(c) = value.get("solid") {
        return Ok(SourceRequest::Ready(Source::Solid {
            color: color(c)?.ok_or("solid needs a color")?,
        }));
    }
    if let Some(family) = value.get("milkdrop") {
        let family = family.as_str().ok_or("milkdrop must be a family id")?;
        let variant = value["variant"].as_u64().unwrap_or(1).max(1) as usize - 1;
        return Ok(SourceRequest::Ready(Source::milkdrop(family, variant)));
    }
    if let Some(id) = value.get("shader") {
        return Ok(SourceRequest::Shader(
            id.as_str().ok_or("shader must be an effect id")?.to_owned(),
        ));
    }
    if let Some(path) = value.get("file") {
        return Ok(SourceRequest::File(PathBuf::from(
            path.as_str().ok_or("file must be a path")?,
        )));
    }
    if let Some(raw) = value.get("raw") {
        return serde_json::from_value(raw.clone())
            .map(SourceRequest::Ready)
            .map_err(|e| format!("raw is not a Source: {e}"));
    }
    Err("source needs one of generator, solid, milkdrop, shader, file or raw".into())
}

/// A 1-based index argument, as a 0-based index.
pub fn index_arg(args: &Value, name: &str) -> Result<usize, String> {
    args[name]
        .as_u64()
        .and_then(|i| (i as usize).checked_sub(1))
        .ok_or_else(|| format!("{name} must be an integer from 1"))
}

/// Like [`index_arg`], `None` when absent.
pub fn optional_index(args: &Value, name: &str) -> Result<Option<usize>, String> {
    if args.get(name).is_none_or(Value::is_null) {
        return Ok(None);
    }
    index_arg(args, name).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use verso_core::{Clip, Command};

    #[test]
    fn summary_uses_one_based_indices() {
        let mut comp = Composition::new("t", 2, 3);
        comp.apply(Command::SetClip {
            layer: 1,
            column: 2,
            clip: Some(Clip::new("m", Source::milkdrop("cat/name", 0))),
        })
        .unwrap();
        comp.apply(Command::TriggerClip {
            layer: 1,
            column: 2,
        })
        .unwrap();
        let summary = composition(&comp);
        let layer = &summary["layers"][1];
        assert_eq!(layer["layer"], 2);
        assert_eq!(layer["playing_column"], 3);
        assert_eq!(layer["clips"][0]["column"], 3);
        assert_eq!(layer["clips"][0]["source"]["kind"], "milkdrop");
        assert_eq!(layer["clips"][0]["source"]["variant"], 1);
        assert_eq!(summary["columns"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn sources_parse() {
        assert_eq!(
            parse_source(&json!({"generator": "plasma", "speed": 2})).unwrap(),
            SourceRequest::Ready(Source::Generator {
                kind: Generator::Plasma,
                color: [1.0; 4],
                speed: 2.0
            })
        );
        assert_eq!(
            parse_source(&json!({"solid": [1, 0, 0]})).unwrap(),
            SourceRequest::Ready(Source::Solid {
                color: [1.0, 0.0, 0.0, 1.0]
            })
        );
        assert_eq!(
            parse_source(&json!({"milkdrop": "a/b", "variant": 3})).unwrap(),
            SourceRequest::Ready(Source::milkdrop("a/b", 2))
        );
        assert_eq!(
            parse_source(&json!({"file": "x.mp4"})).unwrap(),
            SourceRequest::File("x.mp4".into())
        );
        let raw = serde_json::to_value(Source::Solid { color: [0.5; 4] }).unwrap();
        assert_eq!(
            parse_source(&json!({"raw": raw})).unwrap(),
            SourceRequest::Ready(Source::Solid { color: [0.5; 4] })
        );
        assert!(parse_source(&json!({"generator": "nope"})).is_err());
        assert!(parse_source(&json!({})).is_err());
    }

    #[test]
    fn index_arguments_start_at_one() {
        let args = json!({"layer": 1, "column": 0});
        assert_eq!(index_arg(&args, "layer"), Ok(0));
        assert!(index_arg(&args, "column").is_err());
        assert_eq!(optional_index(&args, "group"), Ok(None));
    }

    #[test]
    fn effect_addresses_match_osc() {
        assert_eq!(
            effect_address(
                EffectTarget::Clip {
                    layer: 0,
                    column: 1
                },
                0,
                "radius"
            ),
            "/layer/1/clip/2/effect/1/radius"
        );
    }
}
