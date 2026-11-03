//! Tool definitions and the confirmation rule for destructive actions.

use serde_json::{Value, json};

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// `properties` of the input schema.
    properties: fn() -> Value,
    required: &'static [&'static str],
    pub read_only: bool,
    /// Needs `"confirm": true` in its arguments.
    pub destructive: bool,
}

fn none() -> Value {
    json!({})
}

fn index(what: &str) -> Value {
    json!({"type": "integer", "minimum": 1, "description": format!("{what}, from 1")})
}

fn layer_column() -> Value {
    json!({"layer": index("Layer, counted from the bottom"), "column": index("Column")})
}

fn confirm() -> Value {
    json!({"type": "boolean", "description": "Must be true: the user agreed to this destructive action"})
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "status",
        description: "Instance facts: process id, version, FPS, composition name, file, unsaved changes, size, MCP port, log file, FFmpeg.",
        properties: none,
        required: &[],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "get_composition",
        description: "Layers (bottom first) with their clips, playing clip, opacity, blend, effects and group; columns; groups; master and tempo. full=true returns the whole serialized composition instead.",
        properties: || json!({"full": {"type": "boolean"}}),
        required: &[],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "get_layer",
        description: "One layer: properties, effects with parameter definitions and current values, and the OSC addresses that drive them.",
        properties: || json!({"layer": index("Layer, counted from the bottom")}),
        required: &["layer"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "get_clip",
        description: "One clip: source in full, effects with parameter definitions and values, OSC addresses, playback state.",
        properties: layer_column,
        required: &["layer", "column"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "list_sources",
        description: "Sources that can go in a clip slot: built-in generators, generator shaders and MilkDrop preset families with their variants. filter matches names and ids (case-insensitive).",
        properties: || {
            json!({
                "filter": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "description": "Most entries per kind, default 50"}
            })
        },
        required: &[],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "list_effects",
        description: "Effects (filters) of the library with their parameters and ranges.",
        properties: || json!({"filter": {"type": "string"}}),
        required: &[],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "get_log",
        description: "Recent status messages and errors of the app, and the end of its log file.",
        properties: || json!({"lines": {"type": "integer", "minimum": 1, "description": "Default 50"}}),
        required: &[],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "osc",
        description: "Sends one message of Verso's OSC address space, through the same path as a UDP OSC message. Values are 0..1 (above 0.5 presses a trigger); no value is a press. Examples: /layer/2/opacity 0.5, /layer/1/clip/3/launch, /column/2/launch, /layer/1/eject, /clear, /master, /crossfader, /bpm, /layer/1/bypass, /layer/1/solo, /group/1/opacity, /layer/1/effect/2/<param>, /layer/1/clip/2/effect/1/<param>, /composition/effect/1/bypass. The /composition/layers/N/... scheme of other VJ software works too.",
        properties: || {
            json!({
                "address": {"type": "string"},
                "value": {"type": "number"}
            })
        },
        required: &["address"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "set_clip",
        description: "Puts a source in a clip slot, like dropping it from the browser. source is one of: {\"generator\": \"Plasma\"|\"Gradient\"|\"Stripes\"|\"Rings\", \"color\"?: [r,g,b,a], \"speed\"?: n}, {\"solid\": [r,g,b,a]}, {\"milkdrop\": \"<family id>\", \"variant\"?: n (from 1)}, {\"shader\": \"<generator effect id>\"}, {\"file\": \"<image or video path>\"}, {\"raw\": <Source as JSON>}; null empties the slot.",
        properties: || {
            let mut p = layer_column();
            p["source"] = json!({"type": ["object", "null"]});
            p["name"] =
                json!({"type": "string", "description": "Clip name; defaults to the source name"});
            p
        },
        required: &["layer", "column", "source"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "set_milkdrop",
        description: "Changes a MilkDrop clip: variant (from 1), blend seconds, blend pattern, input layer (from 1, 0 for none) and input mix 0..1.",
        properties: || {
            let mut p = layer_column();
            p["variant"] = json!({"type": "integer", "minimum": 1});
            p["blend"] = json!({"type": "number", "minimum": 0});
            p["pattern"] =
                json!({"type": "string", "enum": ["Random", "Wipe", "Plasma", "Radial"]});
            p["input_layer"] = json!({"type": "integer", "minimum": 0});
            p["input_mix"] = json!({"type": "number", "minimum": 0, "maximum": 1});
            p
        },
        required: &["layer", "column"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "add_effect",
        description: "Adds a library effect at the end of a chain: the clip (layer and column), the layer (layer), the group (group) or the composition (none of them).",
        properties: || {
            json!({
                "effect": {"type": "string", "description": "Effect id from list_effects"},
                "layer": index("Layer"),
                "column": index("Column"),
                "group": index("Group")
            })
        },
        required: &["effect"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "apply_commands",
        description: "Applies raw verso_core Command values in order, in one frame, as the UI does (undo and unsaved state follow). JSON is serde's external tagging, indices from 0: \"ClearAll\", {\"TriggerClip\": {\"layer\": 0, \"column\": 1}}, {\"SetLayerBlend\": {\"layer\": 0, \"blend\": \"Screen\"}}, {\"EditEffects\": {\"target\": {\"Layer\": 0}, \"edit\": {\"SetParam\": {\"index\": 0, \"name\": \"amount\", \"value\": {\"Float\": 0.5}}}}}, \"AddLayer\", {\"RenameColumn\": {\"column\": 0, \"name\": \"Intro\"}}. RemoveLayer, RemoveColumn, RemoveGroup and RemoveScript need confirm.",
        properties: || {
            json!({
                "commands": {"type": "array", "items": {}},
                "confirm": confirm()
            })
        },
        required: &["commands"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "select",
        description: "Selects a layer, or a clip with column, as a click on its header or slot would (without launching). The properties panel and the Preview monitor follow.",
        properties: layer_column,
        required: &["layer"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "hover",
        description: "Simulates the pointer over a layer header (layer), a column header (column) or a clip slot (both), for the hover preview. Lasts seconds (default 10); no layer and no column stops it. Follow with capture target=hover.",
        properties: || {
            let mut p = layer_column();
            p["seconds"] = json!({"type": "number", "minimum": 0});
            p
        },
        required: &[],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "ui_widgets",
        description: "Widgets drawn in the Verso window last frame, from egui's accessibility tree: role, label or value, state, rectangle and center in UI points, plus the rectangle of every clip slot. filter matches labels and values (case-insensitive). Use the centers with ui_input.",
        properties: || {
            json!({
                "filter": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "description": "Most widgets, default 200"}
            })
        },
        required: &[],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "ui_input",
        description: "Real pointer and keyboard input into the Verso window, as a user would give it: hovers, clicks, drags (drag and drop), wheel, keys, text. Coordinates in UI points (see ui_widgets, or capture target=ui). The window can stay in the background. Actions: move (x, y; the pointer stays there), click, double_click, right_click, middle_click (x, y), drag (x, y to to_x, to_y), scroll (x, y, dx, dy; positive dy scrolls up), key (key: Escape, Enter, F2, Space, A, 1…), text (text), leave. modifiers: [\"ctrl\", \"shift\", \"alt\"].",
        properties: || {
            json!({
                "action": {"type": "string", "enum": ["move", "click", "double_click", "right_click", "middle_click", "drag", "scroll", "key", "text", "leave"]},
                "x": {"type": "number"}, "y": {"type": "number"},
                "to_x": {"type": "number"}, "to_y": {"type": "number"},
                "dx": {"type": "number"}, "dy": {"type": "number"},
                "key": {"type": "string"},
                "text": {"type": "string"},
                "modifiers": {"type": "array", "items": {"type": "string", "enum": ["ctrl", "shift", "alt"]}}
            })
        },
        required: &["action"],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "undo",
        description: "Undoes the last structure edit (clips, layers, columns, groups, effect chains, mappings), like Ctrl+Z.",
        properties: none,
        required: &[],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "redo",
        description: "Redoes the last undone edit.",
        properties: none,
        required: &[],
        read_only: false,
        destructive: false,
    },
    Tool {
        name: "capture",
        description: "PNG made by Verso itself, no OS screen capture. target: ui (the whole Verso window as drawn; caption gives its size in UI points for ui_input), output (the composed output), layer (the layer alone), column (the column launched), clip (that clip launched, alone), source (a source not placed anywhere, same format as set_clip; procedural sources only), hover (the hover preview; use hover or ui_input move first). layer, column, clip and source render offscreen without touching the output. frames lets that many frames render first (default 1 for output and ui, 60 otherwise: a new MilkDrop renderer often starts black, raise frames if the image is still dark).",
        properties: || {
            let mut p = layer_column();
            p["target"] = json!({"type": "string", "enum": ["ui", "output", "layer", "column", "clip", "source", "hover"]});
            p["source"] = json!({"type": "object", "description": "For target source"});
            p["frames"] = json!({"type": "integer", "minimum": 1, "maximum": 600});
            p["width"] = json!({"type": "integer", "minimum": 16, "maximum": 3840, "description": "Image width, default 640; the height follows"});
            p
        },
        required: &["target"],
        read_only: true,
        destructive: false,
    },
    Tool {
        name: "save",
        description: "Saves the composition to path, or to its current file. Destructive: overwrites a file.",
        properties: || json!({"path": {"type": "string"}, "confirm": confirm()}),
        required: &["confirm"],
        read_only: false,
        destructive: true,
    },
    Tool {
        name: "open",
        description: "Opens a .verso file, discarding unsaved changes. Destructive.",
        properties: || json!({"path": {"type": "string"}, "confirm": confirm()}),
        required: &["path", "confirm"],
        read_only: false,
        destructive: true,
    },
    Tool {
        name: "new_composition",
        description: "Starts an empty composition, discarding unsaved changes. Destructive.",
        properties: || json!({"confirm": confirm()}),
        required: &["confirm"],
        read_only: false,
        destructive: true,
    },
];

/// Commands that delete part of the document.
const DESTRUCTIVE_COMMANDS: [&str; 4] =
    ["RemoveLayer", "RemoveColumn", "RemoveGroup", "RemoveScript"];

pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

/// Tool descriptions for `tools/list`.
pub fn list() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": {
                    "type": "object",
                    "properties": (t.properties)(),
                    "required": t.required,
                },
                "annotations": {
                    "readOnlyHint": t.read_only,
                    "destructiveHint": t.destructive || t.name == "apply_commands",
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

/// Name of the `Command` variant in its serde JSON form.
fn command_name(command: &Value) -> Option<&str> {
    match command {
        Value::String(name) => Some(name),
        Value::Object(map) if map.len() == 1 => map.keys().next().map(String::as_str),
        _ => None,
    }
}

/// Refuses a destructive call that does not carry `"confirm": true`.
pub fn check(name: &str, args: &Value) -> Result<(), String> {
    let confirmed = args["confirm"].as_bool() == Some(true);
    let tool = find(name).ok_or_else(|| format!("unknown tool {name}"))?;
    if tool.destructive && !confirmed {
        return Err(format!(
            "{name} is destructive: ask the user, then call again with \"confirm\": true."
        ));
    }
    if name == "apply_commands" && !confirmed {
        let destructive: Vec<&str> = args["commands"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(command_name)
            .filter(|n| DESTRUCTIVE_COMMANDS.contains(n))
            .collect();
        if !destructive.is_empty() {
            return Err(format!(
                "{} deletes part of the composition: ask the user, then call again with \"confirm\": true.",
                destructive.join(", ")
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique() {
        for (i, tool) in TOOLS.iter().enumerate() {
            assert!(
                TOOLS[i + 1..].iter().all(|t| t.name != tool.name),
                "{}",
                tool.name
            );
        }
    }

    #[test]
    fn deleting_commands_need_confirm() {
        let args = json!({"commands": ["AddLayer", {"RemoveLayer": 2}]});
        assert!(
            check("apply_commands", &args)
                .unwrap_err()
                .contains("RemoveLayer")
        );
        let args = json!({"commands": [{"RemoveColumn": 0}], "confirm": true});
        assert!(check("apply_commands", &args).is_ok());
        let args = json!({"commands": ["ClearAll", {"TriggerColumn": 1}]});
        assert!(check("apply_commands", &args).is_ok());
    }

    #[test]
    fn file_tools_need_confirm() {
        for name in ["save", "open", "new_composition"] {
            assert!(check(name, &json!({})).is_err(), "{name}");
            assert!(check(name, &json!({"confirm": false})).is_err(), "{name}");
            assert!(check(name, &json!({"confirm": true})).is_ok(), "{name}");
        }
        assert!(check("osc", &json!({"address": "/clear"})).is_ok());
    }
}
